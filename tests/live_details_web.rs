use cookie::{Cookie, CookieJar, Key};
use reqwest::{Client, redirect::Policy};
use sqlx::PgPool;
use wodbuster_jueces::{AppState, browser::ArenaBrowser, config::Config, storage::SessionStore};
mod support;

#[sqlx::test]
#[ignore = "Flujo HTTP real de Cargar y Refrescar con la cookie guardada, en una base temporal"]
async fn loads_detail_and_refreshes_list_or_selected_competition(
    pool: PgPool,
) -> topcoat::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let original = PgPool::connect(&config.database_url).await?;
    let id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM wodbuster_sessions ORDER BY saved_at DESC LIMIT 1",
    )
    .fetch_one(&original)
    .await?;
    let saved = SessionStore::new(original, &config.session_secret)
        .load(id)
        .await?
        .unwrap();
    let store = SessionStore::new(pool.clone(), &config.session_secret);
    let browser = ArenaBrowser::new(config.browser_url);
    let competitions = browser.competitions(&saved.cookie).await?;
    let competition = competitions
        .into_iter()
        .find(|c| c.id == 1246)
        .expect("Competición de prueba accesible");
    store.save(id, &saved.username, &saved.cookie).await?;
    store
        .save_competitions(&saved.username, std::slice::from_ref(&competition))
        .await?;
    let key = Key::derive_from(config.session_secret.as_bytes());
    let mut jar = CookieJar::new();
    jar.signed_mut(&key)
        .add(Cookie::new("wb_session", id.to_string()));
    let session_header = format!("wb_session={}", jar.get("wb_session").unwrap().value());
    let state = AppState {
        store,
        browser,
        cookie_key: key,
        secure_cookies: false,
    };
    let reader = SessionStore::new(pool, &config.session_secret);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(topcoat::serve(
        listener,
        wodbuster_jueces::web::router(state),
    ));
    let result = async {
        let client = Client::builder().redirect(Policy::none()).build()?;
        let page = client
            .get(format!("{base}/competitions/1246"))
            .header("Cookie", &session_header)
            .send()
            .await?;
        assert_eq!(page.status(), 200);
        let csrf_header = page.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let html = page.text().await?;
        assert!(html.contains(">Cargar</button>"));
        let csrf = html
            .split("name=\"csrf\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let headers = format!("{session_header}; {csrf_header}");
        let loaded = client
            .post(format!("{base}/competitions/1246/load"))
            .header("Cookie", &headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(loaded.status(), 303);
        assert_eq!(loaded.headers()["location"], "/competitions/1246");
        let cached = reader
            .competition_detail(&saved.username, 1246)
            .await?
            .unwrap();
        assert_eq!(cached.categories.len(), 5);
        assert!(
            cached
                .categories
                .iter()
                .flat_map(|c| &c.teams)
                .flat_map(|t| &t.scores)
                .any(|s| s.result.as_deref() == Some("07:20"))
        );
        let page = client
            .get(format!("{base}/competitions/1246"))
            .header("Cookie", &headers)
            .send()
            .await?
            .text()
            .await?;
        assert!(page.contains("Evento 1") && page.contains("07:20"));
        assert!(!page.contains(">Cargar</button>"));
        let refreshed = client
            .post(format!("{base}/competitions/refresh"))
            .header("Cookie", &headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(refreshed.status(), 303);
        assert_eq!(refreshed.headers()["location"], "/competitions?loaded=true");
        assert_eq!(
            reader
                .competition_detail(&saved.username, 1246)
                .await?
                .unwrap(),
            cached
        );
        let list = client
            .get(format!("{base}/competitions"))
            .header("Cookie", &headers)
            .send()
            .await?
            .text()
            .await?;
        assert!(list.contains("Ver competicion") && list.contains(">Refrescar</button>"));
        assert_eq!(reader.competitions(&saved.username).await?.len(), 2);
        assert!(
            reader
                .competition_detail(&saved.username, 3910)
                .await?
                .is_none()
        );

        // Datos de otra competición: refrescar 1246 debe conservarlos sin volver a cargarlos.
        reader
            .save_competition_detail(&saved.username, &support::fixture())
            .await?;
        let other_before = reader.competition_detail(&saved.username, 3910).await?;
        let mut stale = cached;
        let score = stale
            .categories
            .iter_mut()
            .flat_map(|c| &mut c.teams)
            .flat_map(|t| &mut t.scores)
            .find(|s| s.result.as_deref() == Some("07:20"))
            .unwrap();
        score.result = Some("stale-test-result".into());
        reader
            .save_competition_detail(&saved.username, &stale)
            .await?;
        assert!(page.contains("action=\"/competitions/1246/refresh\""));
        let refreshed_detail = client
            .post(format!("{base}/competitions/1246/refresh"))
            .header("Cookie", &headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(refreshed_detail.status(), 303);
        assert_eq!(refreshed_detail.headers()["location"], "/competitions/1246");
        let updated = reader
            .competition_detail(&saved.username, 1246)
            .await?
            .unwrap();
        let results: Vec<_> = updated
            .categories
            .iter()
            .flat_map(|c| &c.teams)
            .flat_map(|t| &t.scores)
            .filter_map(|s| s.result.as_deref())
            .collect();
        assert!(results.contains(&"07:20"));
        assert!(!results.contains(&"stale-test-result"));
        assert_eq!(
            reader.competition_detail(&saved.username, 3910).await?,
            other_before
        );
        println!("Refrescar lista y competición seleccionada: OK; las demás conservan sus datos.");
        Ok::<_, topcoat::Error>(())
    }
    .await;
    server.abort();
    result
}
