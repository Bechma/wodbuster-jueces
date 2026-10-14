use wodbuster_jueces::{AppState, config::Config};

#[tokio::test]
#[ignore = "Carga real y persistencia de categorías, equipos, WODs y puntuaciones"]
async fn captures_and_persists_all_accessible_competition_details() -> topcoat::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = sqlx::PgPool::connect(&config.database_url).await?;
    let session_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM wodbuster_sessions ORDER BY saved_at DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await?;
    let state = AppState::connect(config).await?;
    let saved = state
        .store
        .load(session_id)
        .await?
        .expect("Cookie existente");
    let competitions = state.browser.competitions(&saved.cookie).await?;
    state
        .store
        .save_competitions(&saved.username, &competitions)
        .await?;
    for competition in competitions {
        let detail = state
            .browser
            .competition_detail(&saved.cookie, competition.id)
            .await?;
        assert!(!detail.categories.is_empty());
        let teams = detail
            .categories
            .iter()
            .map(|c| c.teams.len())
            .sum::<usize>();
        let wods = detail
            .categories
            .iter()
            .map(|c| c.wods.len())
            .sum::<usize>();
        let scores = detail
            .categories
            .iter()
            .flat_map(|c| &c.teams)
            .map(|t| t.scores.len())
            .sum::<usize>();
        for category in &detail.categories {
            assert!(category.id > 0 && !category.name.is_empty());
            for team in &category.teams {
                assert!(!team.reference.is_empty() && !team.name.is_empty());
                for score in &team.scores {
                    assert!(category.wods.iter().any(|w| w.id == score.wod_id));
                }
            }
        }
        state
            .store
            .save_competition_detail(&saved.username, &detail)
            .await?;
        let mut restored = state
            .store
            .competition_detail(&saved.username, competition.id)
            .await?
            .unwrap();
        assert!(restored.loaded_at.is_some());
        restored.loaded_at = None;
        assert_eq!(restored, detail);
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM competition_scores WHERE username=$1 AND competition_id=$2",
        )
        .bind(saved.username.to_lowercase())
        .bind(competition.id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(count, scores as i64);
        println!(
            "{}: {} categorías, {teams} equipos, {wods} WODs, {scores} puntuaciones/referencias guardadas",
            competition.name,
            detail.categories.len()
        );
    }
    Ok(())
}
