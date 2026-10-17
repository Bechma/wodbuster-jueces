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

#[sqlx::test]
#[ignore = "Solo Postgres local; navegador inaccesible, sin IA ni Arena"]
async fn submission_blocks_reject_parsed_photos_and_manual_retries_before_any_remote_access(
    pool: PgPool,
) -> anyhow::Result<()> {
    use crate::{browser::ArenaBrowser, submission_blocks::SubmissionBlock};
    let (store, photo, extraction, _) = submitting_job(&pool).await?;
    let block = SubmissionBlock {
        category_id: 16907,
        wod_id: 0,
        wod_name: "giralda".into(),
    };
    store
        .set_submission_block("judge@example.com", 3910, block.clone(), true)
        .await?;
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    // Simula la lectura devuelta por la IA, sin invocar al proveedor.
    sqlx::query("UPDATE score_photos SET state='parsing' WHERE id=$1")
        .bind(photo.id)
        .execute(&pool)
        .await?;
    let parsing = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    let mut reading = extraction.reading.clone();
    assert!(
        reject_if_blocked(
            &store,
            &parsing,
            &detail,
            &mut reading,
            std::slice::from_ref(&block)
        )
        .await?
    );
    let failed = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(
        failed.reading.as_ref().unwrap()["needs_retake"]
            .as_bool()
            .unwrap()
    );
    assert!(
        failed.reading.as_ref().unwrap()["retake_reason"]
            .as_str()
            .unwrap()
            .contains("Subida bloqueada")
    );
    assert!(failed.prepared.is_none());
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("Subida bloqueada")
    );
    assert!(!failed.validation_errors.as_array().unwrap().is_empty());
    assert!(store.claim_photo().await?.is_none());

    let state = AppState {
        store: store.clone(),
        browser: Arc::new(ArenaBrowser::new("ws://127.0.0.1:1".parse()?)),
        cookie_key: topcoat::cookie::Key::derive_from(
            b"submission-block-test-secret-with-at-least-32-bytes",
        ),
        secure_cookies: false,
    };
    for block in [
        block,
        SubmissionBlock {
            category_id: 16907,
            wod_id: 0,
            wod_name: String::new(),
        },
        SubmissionBlock {
            category_id: 0,
            wod_id: 0,
            wod_name: "giralda".into(),
        },
    ] {
        store
            .set_submission_block("judge@example.com", 3910, block.clone(), true)
            .await?;
        let failed = store
            .photo("judge@example.com", 3910, photo.id)
            .await?
            .unwrap();
        // La edición manual intenta borrar la marca de retake y vuelve a encolar.
        assert!(
            store
                .requeue_photo(&failed, Uuid::nil(), Some(&extraction), "test")
                .await?
        );
        let submitting = store.claim_photo().await?.unwrap();
        // Sin sesión ni navegador disponibles: debe terminar por el bloqueo antes de ambos.
        process(&state, &submitting).await?;
        let failed = store
            .photo("judge@example.com", 3910, photo.id)
            .await?
            .unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(failed.reading.as_ref().unwrap()["needs_retake"], true);
        assert!(failed.prepared.is_none());
        store
            .set_submission_block("judge@example.com", 3910, block, false)
            .await?;
    }
    let failed = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert!(
        store
            .requeue_photo(&failed, Uuid::nil(), Some(&extraction), "test")
            .await?
    );
    let submitting = store.claim_photo().await?.unwrap();
    assert!(submitting.prepared.is_some());
    assert!(
        !reject_if_blocked(
            &store,
            &submitting,
            &detail,
            &mut extraction.reading.clone(),
            &store.submission_blocks("judge@example.com", 3910).await?
        )
        .await?
    );
    Ok(())
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
            judge_name: None,
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
            expected_results: 1,
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

async fn multiple_job(
    pool: &PgPool,
) -> anyhow::Result<(
    Arc<SessionStore>,
    Photo,
    crate::scorecard::ScorecardExtraction,
    Vec<Score>,
)> {
    use crate::scorecard::{Scorecard, WodReading, evaluate_scorecard};
    let (store, photo, extraction, mut first) = submitting_job(pool).await?;
    first.metadata["fields"][0]["value"] = json!("2");
    let mut detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    let mut wod = detail.categories[0].wods[0].clone();
    wod.id += 1;
    wod.name = "Segundo WOD".into();
    let mut score = detail.categories[0].teams[0].scores[0].clone();
    score.wod_id = wod.id;
    let mut second = first.clone();
    second.wod_id = wod.id;
    second.result = Some("7reps".into());
    second.value = Some(ScoreValue::Repetitions { repetitions: 7 });
    second.metadata["fields"][0]["value"] = json!("7");
    detail.categories[0].wods.push(wod);
    detail.categories[0].teams[0].scores.push(score);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut card = Scorecard::from(extraction.reading);
    card.judge_name = Some("Ana".into());
    card.results.push(WodReading {
        wod_id: Some(second.wod_id),
        score_value: second.value.clone(),
    });
    let extraction = evaluate_scorecard(&detail, card);
    assert!(extraction.ready_for_form);
    sqlx::query("UPDATE score_photos SET state='parsing',expected_results=2,reading=NULL,prepared=NULL WHERE id=$1")
        .bind(photo.id).execute(pool).await?;
    let parsing = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert!(store.parsed_scorecard(&parsing, &extraction).await?);
    let submitting = store.claim_photo().await?.unwrap();
    Ok((store, submitting, extraction, vec![first, second]))
}

#[sqlx::test]
#[ignore = "Solo Postgres local; sin IA ni Arena"]
async fn scorecard_blocks_and_validation_prevent_any_partial_submission(
    pool: PgPool,
) -> anyhow::Result<()> {
    use crate::{browser::ArenaBrowser, submission_blocks::SubmissionBlock};
    let (store, photo, extraction, _) = multiple_job(&pool).await?;
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    let mut missing = extraction.reading.clone();
    missing.results[1].score_value = None;
    let invalid = evaluate_scorecard(&detail, missing);
    assert!(!invalid.ready_for_form && invalid.prepared.is_none());
    assert!(store.parsed_scorecard(&photo, &invalid).await?);
    assert!(store.claim_photo().await?.is_none());
    let incomplete = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(incomplete.state, "incomplete");
    assert_eq!(
        incomplete.reading.as_ref().unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .requeue_scorecard(&incomplete, Uuid::nil(), &extraction)
            .await?
    );
    let photo = store.claim_photo().await?.unwrap();
    store
        .set_submission_block(
            "judge@example.com",
            3910,
            SubmissionBlock {
                category_id: 16907,
                wod_id: 64460,
                wod_name: String::new(),
            },
            true,
        )
        .await?;
    let state = AppState {
        store: store.clone(),
        browser: Arc::new(ArenaBrowser::new("ws://127.0.0.1:1".parse()?)),
        cookie_key: topcoat::cookie::Key::derive_from(
            b"multiple-test-secret-with-at-least-32-bytes",
        ),
        secure_cookies: false,
    };
    // La segunda puntuación bloquea toda la tarjeta antes de cargar la sesión o el navegador.
    process(&state, &photo).await?;
    let failed = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.prepared.is_none() && failed.result.is_none());
    assert_eq!(failed.reading.as_ref().unwrap()["needs_retake"], true);
    assert!(failed.error.as_ref().unwrap().contains("64460"));
    Ok(())
}

#[sqlx::test]
#[ignore = "Solo Postgres local; confirmaciones simuladas, sin IA ni Arena"]
async fn scorecard_partial_failure_recovery_and_retry_keep_one_incident_and_both_results(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, photo, extraction, scores) = multiple_job(&pool).await?;
    let plans = extraction.prepared.as_ref().unwrap();
    assert!(persist_confirmed_photo(&store, &photo, &plans[0], &scores[0]).await?);
    // Repetir la persistencia de una misma confirmación no completa la pareja.
    assert!(store.confirm_photo(&photo, &scores[0]).await?);
    let partial = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert!(partial.busy());
    assert_eq!(
        partial.result.as_ref().unwrap().as_array().unwrap().len(),
        1
    );
    assert!(
        partial
            .result_summary()
            .contains("Segundo WOD: sin confirmación")
    );
    assert!(
        !store
            .requeue_scorecard(&partial, Uuid::nil(), &extraction)
            .await?
    );
    assert!(
        store
            .finish_photo(
                &photo,
                "failed",
                Some("Segundo WOD: fallo antes de guardar. Confirmado 1 de 2."),
                None
            )
            .await?
    );
    let failed = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.result, partial.result);
    // Una corrección explícita vuelve a encolar ambos resultados; conserva la imagen y el juez.
    assert!(
        store
            .requeue_scorecard(&failed, Uuid::nil(), &extraction)
            .await?
    );
    let retry = store.claim_photo().await?.unwrap();
    assert_eq!(retry.expected_results, 2);
    assert!(retry.result.is_none());
    assert_eq!(retry.reading.as_ref().unwrap()["judge_name"], "Ana");
    assert_eq!(
        retry.prepared.as_ref().unwrap().as_array().unwrap().len(),
        2
    );
    assert!(!persist_confirmed_photo(&store, &photo, &plans[1], &scores[1]).await?);
    assert!(persist_confirmed_photo(&store, &retry, &plans[0], &scores[0]).await?);
    store.recover_photos().await?;
    let recovered = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(recovered.state, "unconfirmed");
    assert_eq!(
        recovered.result.as_ref().unwrap().as_array().unwrap().len(),
        1
    );
    assert!(store.claim_photo().await?.is_none());
    assert!(!persist_confirmed_photo(&store, &retry, &plans[1], &scores[1]).await?);
    // Reanalizar borra la lectura y confirmaciones anteriores para procesar ambos de nuevo.
    assert!(
        store
            .requeue_photo(&recovered, Uuid::nil(), None, "test")
            .await?
    );
    let parsing = store.claim_photo().await?.unwrap();
    assert_eq!(parsing.state, "parsing");
    assert_eq!(parsing.expected_results, 2);
    assert!(parsing.reading.is_none() && parsing.result.is_none());
    assert!(store.parsed_scorecard(&parsing, &extraction).await?);
    let submitting = store.claim_photo().await?.unwrap();
    // Ambas confirmadas en remoto, pero se interrumpe la última copia local.
    for score in &scores {
        assert!(store.confirm_photo(&submitting, score).await?);
    }
    store.recover_photos().await?;
    let recovered = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(recovered.state, "saved_local_failed");
    assert_eq!(
        recovered.result.as_ref().unwrap().as_array().unwrap().len(),
        2
    );
    Ok(())
}

#[sqlx::test]
#[ignore = "Solo Postgres local; confirmaciones simuladas, sin IA ni Arena"]
async fn scorecard_succeeds_only_after_both_local_copies_and_can_remove_a_result(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (store, photo, extraction, scores) = multiple_job(&pool).await?;
    let plans = extraction.prepared.as_ref().unwrap();
    for index in 0..2 {
        assert!(persist_confirmed_photo(&store, &photo, &plans[index], &scores[index]).await?);
        let saved = store
            .photo("judge@example.com", 3910, photo.id)
            .await?
            .unwrap();
        assert_eq!(
            saved.state,
            if index == 0 {
                "submitting"
            } else {
                "succeeded"
            }
        );
    }
    let saved = store.photos("judge@example.com", 3910, 0).await?.remove(0);
    assert_eq!(saved.sync_status, "synced");
    assert_eq!(saved.result.as_ref().unwrap().as_array().unwrap().len(), 2);
    let detail = store
        .competition_detail("judge@example.com", 3910)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .unwrap();
    for score in &scores {
        assert_eq!(
            detail.categories[0].teams[0]
                .scores
                .iter()
                .find(|s| s.wod_id == score.wod_id)
                .unwrap()
                .value,
            score.value
        );
    }
    let mut single = extraction.reading;
    single.results.pop();
    let single = evaluate_scorecard(&detail, single);
    assert!(
        store
            .requeue_scorecard(&saved, Uuid::nil(), &single)
            .await?
    );
    let edited = store
        .photo("judge@example.com", 3910, photo.id)
        .await?
        .unwrap();
    assert_eq!(edited.expected_results, 1);
    assert!(edited.reading.as_ref().unwrap().get("results").is_none());
    assert_eq!(
        store
            .competition_detail("judge@example.com", 3910)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .unwrap(),
        detail
    );
    Ok(())
}
