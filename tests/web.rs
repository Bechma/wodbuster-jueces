use cookie::{CookieJar, Key};
use reqwest::{Client, redirect::Policy};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    AppState,
    browser::{ArenaBrowser, AuthCookie},
    competitions::Competition,
    storage::SessionStore,
};
mod support;

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
    let saved_store = SessionStore::new(pool, secret);
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
        let csrf_header = set_cookie.split(';').next().unwrap().to_owned();
        let html = response.text().await?;
        assert_eq!(html.matches("<!DOCTYPE html>").count(), 1);
        assert_eq!(html.matches("<html lang=\"es\">").count(), 1);
        assert!(html.contains("<title>Acceso · WodBuster Jueces</title>"));
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
        assert!(
            success.contains("Mis competiciones") && success.contains("action=\"/competitions\"")
        );
        let signed_headers = format!("wb_session={}; {csrf_header}", cookie.value());
        let page = client
            .get(format!("{base}/competitions"))
            .header("Cookie", &signed_headers)
            .send()
            .await?
            .text()
            .await?;
        assert!(
            page.contains(">Cargar</button>")
                && page.contains("Todavía no hay competiciones guardadas")
        );
        let denied_load = client
            .post(format!("{base}/competitions/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", "forged")])
            .send()
            .await?;
        assert_eq!(denied_load.status(), 403);
        let failed_load = client
            .post(format!("{base}/competitions/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(failed_load.status(), 303);
        assert_eq!(
            failed_load.headers()["location"],
            "/competitions?error=unavailable"
        );
        assert!(
            saved_store
                .competitions("fixture@example.com")
                .await?
                .is_empty()
        );
        saved_store
            .save_competitions(
                "fixture@example.com",
                &[Competition::new(3910, "SOUTH GAMES")],
            )
            .await?;
        saved_store
            .save_competitions(
                "other@example.com",
                &[Competition::new(519, "Another account")],
            )
            .await?;
        let cached = client
            .get(format!("{base}/competitions"))
            .header("Cookie", &signed_headers)
            .send()
            .await?
            .text()
            .await?;
        assert!(cached.contains("SOUTH GAMES") && !cached.contains(">Cargar</button>"));
        assert!(!cached.contains("Another account"));
        assert!(cached.contains("<title>Mis competiciones · WodBuster Jueces</title>"));
        assert_eq!(cached.matches(">Volver</button>").count(), 2);
        assert!(
            cached.contains("action=\"/competitions/3910/delete\"")
                && cached.contains(">Borrar</button>")
        );
        assert!(cached.contains("<button type=\"submit\" class=\"secondary\">Volver</button>"));
        // La lista se sirve incluso con Browserless inaccesible: se consulta Postgres.
        let load_again = client
            .post(format!("{base}/competitions/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(load_again.headers()["location"], "/competitions");
        let detail = client
            .get(format!("{base}/competitions/3910"))
            .header("Cookie", &signed_headers)
            .send()
            .await?;
        assert_eq!(detail.status(), 200);
        let detail = detail.text().await?;
        assert!(
            detail.contains("action=\"/competitions/3910/load\"")
                && detail.contains(">Cargar</button>")
        );
        let denied_detail = client
            .post(format!("{base}/competitions/3910/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", "forged")])
            .send()
            .await?;
        assert_eq!(denied_detail.status(), 403);
        let failed_detail = client
            .post(format!("{base}/competitions/3910/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(
            failed_detail.headers()["location"],
            "/competitions/3910?error=unavailable"
        );
        assert!(
            saved_store
                .competition_detail("fixture@example.com", 3910)
                .await?
                .is_none()
        );
        saved_store
            .save_competition_detail("fixture@example.com", &support::fixture())
            .await?;
        let cached_detail = client
            .get(format!("{base}/competitions/3910"))
            .header("Cookie", &signed_headers)
            .send()
            .await?
            .text()
            .await?;
        assert!(
            cached_detail.contains("Giralda")
                && cached_detail.contains("52reps")
                && cached_detail.contains("Team &amp; Co")
        );
        assert!(!cached_detail.contains(">Cargar</button>"));
        assert!(cached_detail.contains("<p class=\"description\">AMRAP 10 minutos</p>"));
        assert!(cached_detail.contains("<title>SOUTH GAMES · WodBuster Jueces</title>"));
        assert_eq!(cached_detail.matches("<head>").count(), 1);
        assert_eq!(
            cached_detail
                .matches("<main class=\"competition-detail\">")
                .count(),
            1
        );
        assert_eq!(
            cached_detail
                .matches(">Volver a Mis competiciones</button>")
                .count(),
            2
        );
        assert!(cached_detail.contains(
            "<button type=\"submit\" class=\"secondary\">Volver a Mis competiciones</button>"
        ));
        assert!(
            cached_detail.contains("action=\"/competitions/3910/refresh\"")
                && cached_detail.contains(">Refrescar</button>")
        );
        let already_loaded = client
            .post(format!("{base}/competitions/3910/load"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(already_loaded.headers()["location"], "/competitions/3910");
        let before_refresh = saved_store
            .competition_detail("fixture@example.com", 3910)
            .await?;
        let denied_detail_refresh = client
            .post(format!("{base}/competitions/3910/refresh"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", "forged")])
            .send()
            .await?;
        assert_eq!(denied_detail_refresh.status(), 403);
        let failed_detail_refresh = client
            .post(format!("{base}/competitions/3910/refresh"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(
            failed_detail_refresh.headers()["location"],
            "/competitions/3910?error=unavailable"
        );
        assert_eq!(
            saved_store
                .competition_detail("fixture@example.com", 3910)
                .await?,
            before_refresh
        );
        let denied_refresh = client
            .post(format!("{base}/competitions/refresh"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", "forged")])
            .send()
            .await?;
        assert_eq!(denied_refresh.status(), 403);
        let refresh = client
            .post(format!("{base}/competitions/refresh"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(
            refresh.headers()["location"],
            "/competitions?error=unavailable"
        );
        assert!(
            saved_store
                .competition_detail("fixture@example.com", 3910)
                .await?
                .is_some()
        );
        let unauthorized_detail = client
            .get(format!("{base}/competitions/519"))
            .header("Cookie", &signed_headers)
            .send()
            .await?;
        assert_eq!(unauthorized_detail.status(), 404);
        let denied_delete = client
            .post(format!("{base}/competitions/3910/delete"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", "forged")])
            .send()
            .await?;
        assert_eq!(denied_delete.status(), 403);
        let other_delete = client
            .post(format!("{base}/competitions/519/delete"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(other_delete.status(), 404);
        assert_eq!(
            saved_store.competitions("other@example.com").await?.len(),
            1
        );
        assert_eq!(
            saved_store
                .competition_detail("fixture@example.com", 3910)
                .await?,
            before_refresh
        );
        let deleted = client
            .post(format!("{base}/competitions/3910/delete"))
            .header("Cookie", &signed_headers)
            .form(&[("csrf", csrf)])
            .send()
            .await?;
        assert_eq!(deleted.status(), 303);
        assert_eq!(deleted.headers()["location"], "/competitions");
        assert!(
            saved_store
                .competitions("fixture@example.com")
                .await?
                .is_empty()
        );
        assert!(
            saved_store
                .competition_detail("fixture@example.com", 3910)
                .await?
                .is_none()
        );
        let removed_page = client
            .get(format!("{base}/competitions/3910"))
            .header("Cookie", &signed_headers)
            .send()
            .await?;
        assert_eq!(removed_page.status(), 404);
        let unauthorized = Client::builder()
            .redirect(Policy::none())
            .build()?
            .get(format!("{base}/competitions"))
            .send()
            .await?;
        assert_eq!(unauthorized.status(), 303);
        assert_eq!(unauthorized.headers()["location"], "/");
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
