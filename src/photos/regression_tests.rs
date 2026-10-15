use super::*;
use crate::{
    competitions::Competition,
    details::Score,
    scorecard::{ExtractionResult, PreparedScore},
    scoring::{ScoreFormat, ScoreValue},
    storage::SessionStore,
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

mod support {
    use crate as wodbuster_jueces;
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
}

async fn submitting_job(
    pool: &PgPool,
) -> anyhow::Result<(Arc<SessionStore>, Photo, ExtractionResult, Score)> {
    let store = Arc::new(SessionStore::new(
        pool.clone(),
        "test-photo-regressions-secret",
    ));
    let mut detail = support::fixture();
    let fields =
        json!([{"attributes":{"name":"panel$IoReps","id":"reps"},"options":[],"value":"52"}]);
    detail.categories[0].wods[0].score_format = ScoreFormat::from_fields(&fields);
    detail.categories[0].wods[0].metadata["score_fields"] = fields.clone();
    detail.categories[0].teams[0].scores[0].metadata = json!({
        "fields":fields,"save_controls":[{"name":"panel$CtlSave","id":"save"}]
    });
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Fixture")])
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    store
        .save_competition_detail("judge@example.com", &detail)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let extraction = evaluate_reading(
        &detail,
        ScorecardReading {
            category_id: Some(16907),
            wod_id: Some(64459),
            team_reference: Some("298682".into()),
            score_value: Some(ScoreValue::Repetitions { repetitions: 2 }),
            needs_retake: false,
            retake_reason: None,
        },
    );
    assert!(extraction.ready_for_form);
    store
        .save_photo(NewPhoto {
            id: Uuid::new_v4(),
            username: "judge@example.com",
            competition_id: 3910,
            session_id: Uuid::nil(),
            image: b"fixture",
            media_type: "image/png",
            width: 1,
            height: 1,
            model: "test",
        })
        .await?;
    let parsing = store.claim_photo().await?.unwrap();
    assert!(store.parsed_photo(&parsing, &extraction).await?);
    let submitting = store.claim_photo().await?.unwrap();
    assert_eq!(submitting.state, "submitting");
    let mut score = detail.categories[0].teams[0].scores[0].clone();
    score.result = Some("2reps".into());
    score.value = Some(ScoreValue::Repetitions { repetitions: 2 });
    Ok((store, submitting, extraction, score))
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn confirmed_photo_stays_busy_while_local_persistence_is_blocked(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, photo, extraction, score) = submitting_job(&pool).await?;
    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('judge@example.com', 0))")
        .execute(&mut *blocker)
        .await?;
    let id = photo.id;
    let worker_store = store.clone();
    let plan = extraction.prepared.clone().unwrap();
    let worker =
        tokio::spawn(
            async move { persist_confirmed_photo(&worker_store, &photo, &plan, &score).await },
        );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let confirmed: bool =
                sqlx::query_scalar("SELECT result IS NOT NULL FROM score_photos WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await?;
            if confirmed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    let busy = store.photo("judge@example.com", 3910, id).await?.unwrap();
    assert_eq!(busy.state, "submitting");
    assert!(busy.busy());
    for correction in [None, Some(&extraction)] {
        assert!(
            !store
                .requeue_photo(&busy, Uuid::nil(), correction, "new-model")
                .await?
        );
    }
    assert!(!worker.is_finished());
    blocker.commit().await?;
    tokio::time::timeout(Duration::from_secs(5), worker).await???;
    let done = store.photo("judge@example.com", 3910, id).await?.unwrap();
    assert_eq!(done.state, "succeeded");
    assert!(!done.busy());
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    assert_eq!(
        detail.categories[0].teams[0].scores[0].result.as_deref(),
        Some("2reps")
    );
    assert!(
        store
            .requeue_photo(&done, Uuid::nil(), Some(&extraction), "new-model")
            .await?
    );
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn stale_worker_cannot_overwrite_a_reanalysis_or_manual_correction(
    pool: PgPool,
) -> anyhow::Result<()> {
    for manual in [false, true] {
        let (store, stale, extraction, score) = submitting_job(&pool).await?;
        let plan: PreparedScore = extraction.prepared.clone().unwrap();
        persist_confirmed_photo(&store, &stale, &plan, &score).await?;
        let done = store
            .photo("judge@example.com", 3910, stale.id)
            .await?
            .unwrap();
        let mut corrected = extraction.reading.clone();
        corrected.score_value = Some(ScoreValue::Repetitions { repetitions: 7 });
        let detail = store
            .competition_detail("judge@example.com", 3910)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .unwrap();
        let extraction = evaluate_reading(&detail, corrected);
        assert!(extraction.ready_for_form);
        assert!(
            store
                .requeue_photo(
                    &done,
                    Uuid::nil(),
                    manual.then_some(&extraction),
                    "new-model"
                )
                .await?
        );
        // Una corrección vuelve al mismo estado submitting, pero tiene otra revisión.
        let claimed = store.claim_photo().await?.unwrap();
        assert_eq!(claimed.state, if manual { "submitting" } else { "parsing" });
        assert!(claimed.revision > stale.revision);
        let before = serde_json::to_value(&claimed)?;
        assert!(
            !store
                .finish_photo(&stale, "succeeded", None, Some(&score))
                .await?
        );
        assert!(!store.fail_photo(&stale, "failed", "stale error").await?);
        assert!(!store.parsed_photo(&stale, &extraction).await?);
        assert!(!store.confirm_photo(&stale, &score).await?);
        let mut outdated_score = score.clone();
        outdated_score.result = Some("999reps".into());
        persist_confirmed_photo(&store, &stale, &plan, &outdated_score).await?;
        assert_eq!(
            serde_json::to_value(
                store
                    .photo("judge@example.com", 3910, stale.id)
                    .await?
                    .unwrap()
            )?,
            before
        );
        let detail = store
            .competition_detail("judge@example.com", 3910)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .unwrap();
        assert_eq!(
            detail.categories[0].teams[0].scores[0].result.as_deref(),
            Some("2reps")
        );
        assert!(store.fail_photo(&claimed, "failed", "End test job").await?);
    }
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn recovery_preserves_remote_confirmation_and_invalidates_old_workers(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, confirmed, extraction, score) = submitting_job(&pool).await?;
    assert!(store.confirm_photo(&confirmed, &score).await?);
    let (_, uncertain, _, _) = submitting_job(&pool).await?;
    store.recover_photos().await?;
    let recovered = store
        .photo("judge@example.com", 3910, confirmed.id)
        .await?
        .unwrap();
    assert_eq!(recovered.state, "saved_local_failed");
    assert!(!recovered.busy());
    assert!(
        recovered
            .error
            .as_deref()
            .unwrap()
            .contains("no repitas el envío")
    );
    let saved: sqlx::types::Json<Score> =
        sqlx::query_scalar("SELECT result FROM score_photos WHERE id=$1")
            .bind(confirmed.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved.0, score);
    assert_eq!(
        store
            .photo("judge@example.com", 3910, uncertain.id)
            .await?
            .unwrap()
            .state,
        "unconfirmed"
    );
    assert!(store.claim_photo().await?.is_none());
    assert!(
        !store
            .finish_photo(&confirmed, "succeeded", None, Some(&score))
            .await?
    );
    assert!(!store.fail_photo(&uncertain, "failed", "stale").await?);

    // Un análisis recuperado puede volver al mismo estado; su worker anterior no lo posee.
    assert!(
        store
            .requeue_photo(&recovered, Uuid::nil(), None, "test")
            .await?
    );
    let old_parser = store.claim_photo().await?.unwrap();
    store.recover_photos().await?;
    let new_parser = store.claim_photo().await?.unwrap();
    assert_eq!(new_parser.state, "parsing");
    assert!(new_parser.revision > old_parser.revision);
    assert!(!store.parsed_photo(&old_parser, &extraction).await?);
    assert!(store.parsed_photo(&new_parser, &extraction).await?);
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn failed_local_copy_keeps_remote_confirmation_without_resubmitting(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, photo, extraction, score) = submitting_job(&pool).await?;
    sqlx::query("DELETE FROM competition_scores WHERE username='judge@example.com'")
        .execute(&pool)
        .await?;
    persist_confirmed_photo(
        &store,
        &photo,
        extraction.prepared.as_ref().unwrap(),
        &score,
    )
    .await?;
    let done = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(done.state, "saved_local_failed");
    assert!(!done.busy());
    assert!(
        done.error
            .as_deref()
            .unwrap()
            .contains("no repitas el envío")
    );
    let saved: sqlx::types::Json<Score> =
        sqlx::query_scalar("SELECT result FROM score_photos WHERE id=$1")
            .bind(photo.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(saved.0, score);
    assert!(store.claim_photo().await?.is_none());
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn recovery_during_local_persistence_prevents_stale_cache_and_photo_updates(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, photo, extraction, score) = submitting_job(&pool).await?;
    let id = photo.id;
    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('judge@example.com', 0))")
        .execute(&mut *blocker)
        .await?;
    let worker_store = store.clone();
    let plan = extraction.prepared.clone().unwrap();
    let worker =
        tokio::spawn(
            async move { persist_confirmed_photo(&worker_store, &photo, &plan, &score).await },
        );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let confirmed: bool =
                sqlx::query_scalar("SELECT result IS NOT NULL FROM score_photos WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await?;
            if confirmed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    // El nuevo worker recupera la confirmación y el usuario solicita otra revisión.
    store.recover_photos().await?;
    let recovered = store.photo("judge@example.com", 3910, id).await?.unwrap();
    assert_eq!(recovered.state, "saved_local_failed");
    let mut corrected = extraction.reading.clone();
    corrected.score_value = Some(ScoreValue::Repetitions { repetitions: 7 });
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    let extraction = evaluate_reading(&detail, corrected);
    assert!(extraction.ready_for_form);
    assert!(
        store
            .requeue_photo(&recovered, Uuid::nil(), Some(&extraction), "test")
            .await?
    );
    let before = store.photo("judge@example.com", 3910, id).await?.unwrap();
    assert_eq!(before.state, "parsed");
    assert_eq!(
        before.reading.as_ref().unwrap()["score_value"]["repetitions"],
        7
    );
    blocker.commit().await?;
    tokio::time::timeout(Duration::from_secs(5), worker).await???;
    assert_eq!(
        serde_json::to_value(store.photo("judge@example.com", 3910, id).await?.unwrap())?,
        serde_json::to_value(before)?
    );
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    assert_eq!(
        detail.categories[0].teams[0].scores[0].result.as_deref(),
        Some("52reps")
    );
    Ok(())
}
