use cookie::{CookieJar, Key};
use reqwest::{Client, redirect::Policy};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    AppState,
    browser::{ArenaBrowser, AuthCookie},
    competitions::Competition,
    scorecard::ScorecardReading,
    storage::SessionStore,
    submission_blocks::{SubmissionBlock, enforce},
};
mod support;

#[sqlx::test(migrations = false)]
#[ignore = "Solo migraciones y datos simulados en Postgres local"]
async fn existing_id_blocks_migrate_to_names_without_losing_unknown_ids(
    pool: PgPool,
) -> topcoat::Result<()> {
    let migrator = sqlx::migrate!();
    for migration in migrator.iter().filter(|m| m.version < 202610170002) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await?;
    }
    let store = SessionStore::new(pool.clone(), "local-name-migration-test");
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Fixture")])
        .await?;
    let mut detail = support::fixture();
    let mut other = detail.categories[0].clone();
    other.id = 16908;
    other.wods[0].id = 64460;
    other.wods[0].name = "\t GIRALDA \n".into();
    other.teams.clear();
    detail.categories.push(other);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    for (category, wod) in [
        (0, 64459),
        (0, 64460),
        (16907, 64459),
        (16907, 0),
        (0, 99999),
    ] {
        sqlx::query("INSERT INTO score_submission_blocks (username,competition_id,category_id,wod_id) VALUES ('judge@example.com',3910,$1,$2)")
            .bind(category).bind(wod).execute(&pool).await?;
    }
    sqlx::raw_sql(include_str!(
        "../migrations/202610170002_submission_block_names.sql"
    ))
    .execute(&pool)
    .await?;
    let blocks = store.submission_blocks("judge@example.com", 3910).await?;
    assert_eq!(blocks.len(), 4);
    assert!(blocks.contains(&SubmissionBlock {
        category_id: 0,
        wod_id: 0,
        wod_name: "giralda".into()
    }));
    assert!(blocks.contains(&SubmissionBlock {
        category_id: 16907,
        wod_id: 0,
        wod_name: "giralda".into()
    }));
    assert!(blocks.contains(&SubmissionBlock {
        category_id: 16907,
        ..Default::default()
    }));
    assert!(blocks.contains(&SubmissionBlock {
        wod_id: 99999,
        ..Default::default()
    }));
    Ok(())
}

#[sqlx::test]
#[ignore = "Solo Postgres local y HTTP local; no usa Browserless, IA ni Arena"]
async fn blocks_are_private_validated_persistent_and_reversible(
    pool: PgPool,
) -> topcoat::Result<()> {
    let secret = "submission-blocks-isolated-local-test";
    let store = std::sync::Arc::new(SessionStore::new(pool.clone(), secret));
    let session_id = Uuid::new_v4();
    store
        .save(
            session_id,
            "judge@example.com",
            &AuthCookie {
                name: ".WBAuth".into(),
                value: "fake-cookie".into(),
                domain: ".wodbuster.com".into(),
                path: "/".into(),
                expires: -1.,
                http_only: true,
                secure: true,
                session: true,
                same_site: None,
            },
        )
        .await?;
    for user in ["judge@example.com", "other@example.com"] {
        store
            .save_competitions(
                user,
                &[
                    Competition::new(3910, "Fixture"),
                    Competition::new(3911, "Other"),
                ],
            )
            .await?;
        store
            .save_competition_detail(user, &support::fixture())
            .await?;
    }
    let mut detail = support::fixture();
    let mut other_category = detail.categories[0].clone();
    other_category.id = 16908;
    other_category.wods[0].id = 64460;
    let mut maestranza = other_category.wods[0].clone();
    maestranza.id = 64461;
    maestranza.name = "Maestranza".into();
    other_category.wods.push(maestranza);
    other_category.teams.clear();
    detail.categories.push(other_category);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    let key = Key::derive_from(secret.as_bytes());
    let state = AppState {
        store: store.clone(),
        browser: ArenaBrowser::new("ws://127.0.0.1:1".parse().unwrap()).into(),
        cookie_key: key.clone(),
        secure_cookies: false,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(topcoat::serve(
        listener,
        wodbuster_jueces::web::router(state),
    ));
    let result = async {
        let client = Client::builder().redirect(Policy::none()).build()?;
        let mut jar = CookieJar::new();
        jar.signed_mut(&key)
            .add(cookie::Cookie::new("wb_session", session_id.to_string()));
        jar.signed_mut(&key)
            .add(cookie::Cookie::new("wb_csrf", "blocks-csrf"));
        let headers = format!(
            "wb_session={}; wb_csrf={}",
            jar.get("wb_session").unwrap().value(),
            jar.get("wb_csrf").unwrap().value()
        );
        let url = format!("{base}/competitions/3910/submission-blocks");
        assert_eq!(client.get(&url).send().await?.status(), 401);
        assert_eq!(
            client
                .get(format!("{base}/competitions/9999/submission-blocks"))
                .header("Cookie", &headers)
                .send()
                .await?
                .status(),
            404
        );
        let page = client.get(&url).header("Cookie", &headers).send().await?;
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["cache-control"], "no-store");
        let html = page.text().await?;
        for text in [
            "Bloquear subidas",
            "Todas las categorías",
            "Todos los WODs",
            "No hay bloqueos activos.",
        ] {
            assert!(html.contains(text), "{text}");
        }
        assert_eq!(html.matches("<option value=\"giralda\">").count(), 1);
        assert!(!html.contains("Giralda · 64459"));
        let post = |csrf: &str, category: &str, wod: &str, blocked: &str| {
            client.post(&url).header("Cookie", &headers).form(&[
                ("csrf", csrf),
                ("category_id", category),
                ("wod_name", wod),
                ("blocked", blocked),
            ])
        };
        assert_eq!(
            post("forged", "16907", "", "true").send().await?.status(),
            403
        );
        for (category, wod) in [
            ("0", ""),
            ("-1", ""),
            ("99999", ""),
            ("0", "Unknown"),
            ("16907", "Maestranza"),
        ] {
            assert_eq!(
                post("blocks-csrf", category, wod, "true")
                    .send()
                    .await?
                    .status(),
                400
            );
        }
        for (category, wod) in [
            ("16907", ""),
            ("0", "giralda"),
            ("16907", "giralda"),
            ("16907", " GIRALDA "),
        ] {
            let response = post("blocks-csrf", category, wod, "true").send().await?;
            assert_eq!(response.status(), 303);
            assert_eq!(
                response.headers()["location"],
                "/competitions/3910/submission-blocks?saved=true"
            );
        }
        assert_eq!(
            store
                .submission_blocks("JUDGE@example.com", 3910)
                .await?
                .len(),
            3
        );
        assert!(
            store
                .submission_blocks("other@example.com", 3910)
                .await?
                .is_empty()
        );
        assert!(
            store
                .submission_blocks("judge@example.com", 3911)
                .await?
                .is_empty()
        );
        // Una categoría nueva con otro ID de WOD queda cubierta por la misma regla.
        let mut refreshed = detail.clone();
        let mut added_category = detail.categories[0].clone();
        added_category.id = 16909;
        added_category.wods[0].id = 64462;
        added_category.teams.clear();
        refreshed.categories.push(added_category);
        store
            .save_competition_detail("judge@example.com", &refreshed)
            .await?;
        store
            .save_competitions(
                "judge@example.com",
                &[
                    Competition::new(3910, "Refreshed"),
                    Competition::new(3911, "Other"),
                ],
            )
            .await?;
        assert_eq!(
            store
                .submission_blocks("judge@example.com", 3910)
                .await?
                .len(),
            3
        );
        let rules = store.submission_blocks("judge@example.com", 3910).await?;
        let global: Vec<_> = rules
            .iter()
            .filter(|b| b.category_id == 0)
            .cloned()
            .collect();
        for (category, wod) in [(16907, 64459), (16908, 64460), (16909, 64462)] {
            let mut reading: ScorecardReading = serde_json::from_value(serde_json::json!({
                "category_id":category,"wod_id":wod,"team_reference":null,"score_value":null,
                "needs_retake":false,"retake_reason":null
            }))?;
            assert!(enforce(&refreshed, &mut reading, &global).is_some());
            assert!(reading.needs_retake);
        }
        let html = client
            .get(&url)
            .header("Cookie", &headers)
            .send()
            .await?
            .text()
            .await?;
        assert_eq!(html.matches(">Desbloquear</button>").count(), 3);
        assert!(html.contains("Giralda"));
        assert_eq!(
            post("blocks-csrf", "16907", "giralda", "false")
                .send()
                .await?
                .status(),
            303
        );
        let blocks = store.submission_blocks("judge@example.com", 3910).await?;
        assert_eq!(blocks.len(), 2);
        assert!(!blocks.contains(&SubmissionBlock {
            category_id: 16907,
            wod_id: 0,
            wod_name: "giralda".into()
        }));
        // También se puede quitar una regla cuyo WOD ya no figure en el último detalle.
        sqlx::query(
            "UPDATE score_submission_blocks SET wod_name='missing' WHERE wod_name='giralda'",
        )
        .execute(&pool)
        .await?;
        assert_eq!(
            post("blocks-csrf", "0", "missing", "false")
                .send()
                .await?
                .status(),
            303
        );
        assert_eq!(
            store
                .submission_blocks("judge@example.com", 3910)
                .await?
                .len(),
            1
        );
        Ok::<_, topcoat::Error>(())
    }
    .await;
    server.abort();
    result
}
