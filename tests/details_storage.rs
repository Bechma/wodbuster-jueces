use sqlx::PgPool;
use wodbuster_jueces::{competitions::Competition, storage::SessionStore};

mod support;
use support::fixture;

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn score_formats_and_values_are_saved_per_category(pool: PgPool) -> topcoat::Result<()> {
    use serde_json::json;
    use wodbuster_jueces::scoring::{ScoreKind, ScoreValue};
    let store = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Fixture")])
        .await?;
    let field = |name: &str, value: &str| json!({"attributes":{"name":format!("panel${name}"),"id":name},"value":value,"options":[]});
    let mut detail = fixture();
    let timed = json!([
        field("IoComplete", "False"),
        field("IoTimeMMSS", ""),
        field("IoReps", "52")
    ]);
    detail.categories[0].wods[0].metadata["score_fields"] = timed.clone();
    detail.categories[0].teams[0].scores[0].metadata["fields"] = timed;
    let mut pending = detail.categories[0].teams[0].clone();
    pending.reference = "pending".into();
    pending.scores[0].result = None;
    detail.categories[0].teams.push(pending);
    let mut scaled = detail.categories[0].clone();
    scaled.id = 16912;
    scaled.name = "SCL Mixto".into();
    scaled.wods[0].metadata["score_fields"] = json!([field("IoReps", "52")]);
    detail.categories.push(scaled);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    let restored = store
        .competition_detail("judge@example.com", 3910)
        .await?
        .unwrap();
    assert_eq!(
        restored.categories[0].wods[0]
            .score_format
            .as_ref()
            .unwrap()
            .kind,
        ScoreKind::TimeOrRepetitions
    );
    assert_eq!(
        restored.categories[1].wods[0]
            .score_format
            .as_ref()
            .unwrap()
            .kind,
        ScoreKind::Repetitions
    );
    assert_eq!(
        restored.categories[0].teams[0].scores[0].value,
        Some(ScoreValue::NotCompleted {
            repetitions: 52,
            tie_break: None
        })
    );
    assert!(!restored.categories[0].teams[1].scores[0].is_scored());
    assert!(restored.categories[0].teams[1].scores[0].value.is_none());
    let stored_kind: String = sqlx::query_scalar(
        "SELECT data->'score_format'->>'kind' FROM competition_wods WHERE category_id=16907",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(stored_kind, "time_or_repetitions");
    let stored_value: serde_json::Value = sqlx::query_scalar("SELECT data->'value' FROM competition_scores WHERE category_id=16907 AND team_reference='298682'").fetch_one(&pool).await?;
    assert_eq!(stored_value["kind"], "not_completed");
    assert_eq!(stored_value["repetitions"], 52);
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn snapshots_and_related_references_are_atomic_and_survive_list_refresh(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    let competition = Competition::new(3910, "Fixture competition");
    store
        .save_competitions("judge@example.com", std::slice::from_ref(&competition))
        .await?;
    let expected = fixture();
    store
        .save_competition_detail("judge@example.com", &expected)
        .await?;
    let reopened = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    let mut stored = reopened
        .competition_detail("JUDGE@example.com", 3910)
        .await?
        .unwrap();
    assert!(stored.loaded_at.is_some());
    stored.loaded_at = None;
    assert_eq!(stored, expected);
    assert!(
        reopened
            .competition_detail("other@example.com", 3910)
            .await?
            .is_none()
    );
    let reference:(String,Option<i64>,Option<String>)=sqlx::query_as("SELECT reference,registration_id,gid FROM competition_teams WHERE username='judge@example.com'").fetch_one(&pool).await?;
    assert_eq!(
        reference,
        (
            "298682".into(),
            Some(298682),
            Some("captain-reference".into())
        )
    );
    let score: (i64, Option<String>) = sqlx::query_as(
        "SELECT wod_id,result FROM competition_scores WHERE username='judge@example.com'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(score, (64459, Some("52reps".into())));
    // Refrescar la lista conserva la información de las competiciones que siguen disponibles.
    reopened
        .save_competitions("judge@example.com", std::slice::from_ref(&competition))
        .await?;
    assert!(
        reopened
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_some()
    );
    // Referencias incorrectas revierten toda la sustitución, sin perder el snapshot anterior.
    let mut invalid = expected.clone();
    invalid.categories[0].teams[0].scores[0].wod_id = 999;
    assert!(
        reopened
            .save_competition_detail("judge@example.com", &invalid)
            .await
            .is_err()
    );
    assert_eq!(
        reopened
            .competition_detail("judge@example.com", 3910)
            .await?
            .unwrap()
            .categories,
        expected.categories
    );
    reopened.save_competitions("judge@example.com", &[]).await?;
    assert!(
        reopened
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_none()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM competition_scores")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn deletes_only_selected_account_competition_and_can_reload_list(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    let competitions = [
        Competition::new(3910, "Selected"),
        Competition::new(1246, "Kept"),
    ];
    store
        .save_competitions("judge@example.com", &competitions)
        .await?;
    store
        .save_competitions("other@example.com", &competitions[..1])
        .await?;
    store
        .save_competition_detail("judge@example.com", &fixture())
        .await?;
    store
        .save_competition_detail("other@example.com", &fixture())
        .await?;
    let mut kept = fixture();
    kept.id = 1246;
    store
        .save_competition_detail("judge@example.com", &kept)
        .await?;
    let kept_before = store.competition_detail("judge@example.com", 1246).await?;
    let other_before = store.competition_detail("other@example.com", 3910).await?;

    assert!(store.delete_competition("JUDGE@example.com", 3910).await?);
    assert!(!store.delete_competition("judge@example.com", 3910).await?);
    assert_eq!(
        store.competitions("judge@example.com").await?,
        competitions[1..]
    );
    assert!(
        store
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_none()
    );
    assert_eq!(
        store.competition_detail("judge@example.com", 1246).await?,
        kept_before
    );
    assert_eq!(
        store.competition_detail("other@example.com", 3910).await?,
        other_before
    );
    for table in [
        "competition_details",
        "competition_categories",
        "competition_teams",
        "competition_wods",
        "competition_scores",
    ] {
        let remaining: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE username=$1 AND competition_id=$2"
        ))
        .bind("judge@example.com")
        .bind(3910_i64)
        .fetch_one(&pool)
        .await?;
        assert_eq!(remaining, 0, "{table}");
    }
    store
        .save_competitions("judge@example.com", &competitions)
        .await?;
    assert_eq!(
        store.competitions("judge@example.com").await?,
        competitions[1..]
            .iter()
            .chain(competitions[..1].iter())
            .cloned()
            .collect::<Vec<_>>()
    );
    assert!(
        store
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_none()
    );
    assert_eq!(
        store.competition_detail("judge@example.com", 1246).await?,
        kept_before
    );
    Ok(())
}
