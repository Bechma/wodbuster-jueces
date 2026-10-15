mod support;

use serde_json::json;
use sqlx::{PgPool, types::Json};
use wodbuster_jueces::{
    competitions::Competition, details::Score, scoring::ScoreValue, storage::SessionStore,
};

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn updates_only_the_confirmed_score_for_the_selected_account(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-session-secret-with-at-least-32-bytes");
    let mut original = support::fixture();
    original.categories[0].wods[0].score_format =
        wodbuster_jueces::scoring::ScoreFormat::from_fields(
            &json!([{ "attributes": {"name":"panel$IoReps", "id":"reps"}, "options":[]} ]),
        );
    for account in ["one@example.com", "two@example.com"] {
        store
            .save_competitions(account, &[Competition::new(original.id, &original.name)])
            .await?;
        store.save_competition_detail(account, &original).await?;
    }
    let mut updated = original.categories[0].teams[0].scores[0].clone();
    updated.result = Some("2reps".into());
    updated.value = Some(ScoreValue::Repetitions { repetitions: 2 });
    updated.metadata = json!({"api":{"Score":"2reps"},"fields":[]});
    store
        .save_confirmed_score("ONE@example.com", original.id, 16907, "298682", &updated)
        .await?;
    let saved = store
        .competition_detail("one@example.com", original.id)
        .await?
        .unwrap();
    assert_eq!(
        saved.categories[0].teams[0].scores[0].result,
        updated.result
    );
    assert_eq!(saved.categories[0].teams[0].scores[0].value, updated.value);
    assert_eq!(
        saved.categories[0].wods[0].name,
        original.categories[0].wods[0].name
    );
    assert_eq!(
        saved.categories[0].teams[0].name,
        original.categories[0].teams[0].name
    );
    let row: Json<Score> =
        sqlx::query_scalar("SELECT data FROM competition_scores WHERE username=$1 AND wod_id=$2")
            .bind("one@example.com")
            .bind(updated.wod_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(row.0.value, updated.value);
    let other = store
        .competition_detail("two@example.com", original.id)
        .await?
        .unwrap();
    assert_eq!(
        other.categories[0].teams[0].scores[0].result,
        original.categories[0].teams[0].scores[0].result
    );
    assert!(
        store
            .save_confirmed_score("one@example.com", original.id, 999, "298682", &updated)
            .await
            .is_err()
    );
    Ok(())
}
