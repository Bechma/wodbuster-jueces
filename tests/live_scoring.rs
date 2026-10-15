use wodbuster_jueces::{
    AppState,
    config::Config,
    scoring::{FieldRole, ScoreKind},
};

#[tokio::test]
#[ignore = "Descubre y guarda los formatos reales de South Games sin guardar puntuaciones en Arena"]
async fn captures_south_games_score_formats() -> topcoat::Result<()> {
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
        .expect("Cookie guardada");
    let detail = state
        .browser
        .competition_detail(&saved.cookie, 3910)
        .await?;
    let category = detail
        .categories
        .iter()
        .find(|c| c.name == "INT Mixto")
        .unwrap();
    let giralda = category.wods.iter().find(|w| w.name == "Giralda").unwrap();
    let format = giralda.score_format.as_ref().unwrap();
    assert_eq!(format.kind, ScoreKind::TimeOrRepetitions);
    assert!(format.has_all_controls());
    assert!(
        format
            .fields
            .iter()
            .any(|f| f.role == FieldRole::Time && f.when_completed == Some(true))
    );
    assert!(
        format
            .fields
            .iter()
            .any(|f| f.role == FieldRole::Repetitions && f.when_completed == Some(false))
    );
    let chicuelo = category.wods.iter().find(|w| w.name == "Chicuelo").unwrap();
    assert_eq!(
        chicuelo.score_format.as_ref().unwrap().kind,
        ScoreKind::Weight
    );
    let scaled = detail
        .categories
        .iter()
        .find(|c| c.name == "SCL Mixto")
        .unwrap();
    assert_eq!(
        scaled
            .wods
            .iter()
            .find(|w| w.name == "Chicuelo")
            .unwrap()
            .score_format
            .as_ref()
            .unwrap()
            .kind,
        ScoreKind::Repetitions
    );
    for category in &detail.categories {
        for wod in &category.wods {
            assert!(
                wod.score_format.as_ref().unwrap().has_all_controls(),
                "{} / {}",
                category.name,
                wod.name
            );
        }
        for score in category.teams.iter().flat_map(|t| &t.scores) {
            if score.result.is_none() {
                assert!(!score.is_scored());
                assert!(score.value.is_none());
            }
        }
    }
    state
        .store
        .save_competition_detail(&saved.username, &detail)
        .await?;
    let mut restored = state
        .store
        .competition_detail(&saved.username, 3910)
        .await?
        .unwrap();
    restored.loaded_at = None;
    assert_eq!(restored, detail);
    println!(
        "Formatos completos guardados para {} categorías de South Games.",
        detail.categories.len()
    );
    Ok(())
}
