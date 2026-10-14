use cookie::{Cookie, CookieJar};
use reqwest::{Client, redirect::Policy};
use wodbuster_jueces::{AppState, config::Config, storage::SessionStore};

#[tokio::test]
#[ignore = "Requiere Postgres, Browserless y una cookie real guardada"]
async fn loads_real_competitions_with_saved_cookie() -> topcoat::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = sqlx::PgPool::connect(&config.database_url).await?;
    let id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM wodbuster_sessions ORDER BY saved_at DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await?;
    let store = SessionStore::new(pool, &config.session_secret);
    let saved = store.load(id).await?.expect("Primero inicia sesión");
    let state = AppState::connect(config).await?;
    // Consulta el sitio con la cookie almacenada, sin volver a usar la contraseña.
    let expected = state.browser.competitions(&saved.cookie).await?;
    let mut jar = CookieJar::new();
    jar.signed_mut(&state.cookie_key)
        .add(Cookie::new("wb_session", id.to_string()));
    let session_header = format!("wb_session={}", jar.get("wb_session").unwrap().value());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(topcoat::serve(
        listener,
        wodbuster_jueces::web::router(state),
    ));
    let result = async {
        let client = Client::builder().redirect(Policy::none()).build()?;
        let page = client.get(format!("{base}/competitions")).header("Cookie", &session_header).send().await?;
        let csrf_header = page.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
        let html = page.text().await?;
        if html.contains("action=\"/competitions/load\"") {
            let csrf = html.split("name=\"csrf\" value=\"").nth(1).unwrap().split('"').next().unwrap();
            let loaded = client.post(format!("{base}/competitions/load"))
                .header("Cookie", format!("{session_header}; {csrf_header}"))
                .form(&[("csrf", csrf)]).send().await?;
            assert_eq!(loaded.status(), 303);
            assert_eq!(loaded.headers()["location"], "/competitions?loaded=true");
        }
        let stored = store.competitions(&saved.username).await?;
        let mut expected_ids = expected.iter().map(|competition| competition.id).collect::<Vec<_>>();
        let mut stored_ids = stored.iter().map(|competition| competition.id).collect::<Vec<_>>();
        expected_ids.sort(); stored_ids.sort();
        assert_eq!(stored_ids, expected_ids);
        let cached = client.get(format!("{base}/competitions")).header("Cookie", &session_header).send().await?.text().await?;
        for competition in &stored {
            assert!(cached.contains(&competition.name));
        }
        println!("{} competiciones de juez/administrador cargadas y guardadas usando la cookie existente.", stored.len());
        Ok::<_, topcoat::Error>(())
    }.await;
    server.abort();
    result
}
