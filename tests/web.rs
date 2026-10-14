use cookie::{CookieJar, Key};
use reqwest::{Client, redirect::Policy};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    AppState,
    browser::{ArenaBrowser, AuthCookie},
    storage::SessionStore,
};

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn form_csrf_and_private_success_page(pool: PgPool) -> topcoat::Result<()> {
    let secret = "test-session-secret-with-at-least-32-bytes";
    let store = SessionStore::new(pool.clone(), secret);
    let id = Uuid::new_v4();
    store
        .save(
            id,
            "fixture@example.com",
            &AuthCookie {
                name: ".WBAuth".into(),
                value: "never-expose-this-cookie".into(),
                domain: ".wodbuster.com".into(),
                path: "/".into(),
                expires: -1.0,
                http_only: true,
                secure: true,
                session: true,
                same_site: None,
            },
        )
        .await?;
    let key = Key::derive_from(secret.as_bytes());
    let state = AppState {
        store,
        browser: ArenaBrowser::new("ws://127.0.0.1:1".parse().unwrap()),
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
        let client = Client::builder()
            .cookie_store(true)
            .redirect(Policy::none())
            .build()?;
        let response = client.get(&base).send().await?;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let set_cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
        let html = response.text().await?;
        assert!(html.contains("autocomplete=\"current-password\""));
        let csrf = html
            .split("name=\"csrf\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let denied = client
            .post(format!("{base}/login"))
            .form(&[
                ("username", "test@example.com"),
                ("password", "test"),
                ("csrf", "forged"),
            ])
            .send()
            .await?;
        assert_eq!(denied.status(), 403);
        let empty = client
            .post(format!("{base}/login"))
            .form(&[("username", ""), ("password", ""), ("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(empty.status(), 303);
        assert_eq!(empty.headers()["location"], "/?error=empty");

        let mut jar = CookieJar::new();
        jar.signed_mut(&key)
            .add(cookie::Cookie::new("wb_session", id.to_string()));
        let cookie = jar.get("wb_session").unwrap();
        let success = client
            .get(&base)
            .header("Cookie", format!("wb_session={}", cookie.value()))
            .send()
            .await?
            .text()
            .await?;
        assert!(success.contains("Acceso completado") && success.contains("fixture@example.com"));
        assert!(!success.contains("never-expose-this-cookie"));
        let anonymous = Client::new().get(&base).send().await?.text().await?;
        assert!(!anonymous.contains("fixture@example.com") && anonymous.contains("Inicia sesión"));
        let forged = Client::new()
            .get(&base)
            .header("Cookie", format!("wb_session={id}"))
            .send()
            .await?
            .text()
            .await?;
        assert!(!forged.contains("fixture@example.com"));
        Ok::<_, topcoat::Error>(())
    }
    .await;
    server.abort();
    result
}
