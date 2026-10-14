use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    cookie::{Cookie, Cookies, RouterBuilderCookieExt, SameSite, cookies, time},
    router::{
        Router,
        content::Form,
        error::{SeeOther, forbidden, see_other},
        header::{CACHE_CONTROL, HeaderValue},
        page,
        response::response_headers,
        route,
    },
    view::{View, view},
};
use uuid::Uuid;

use crate::{AppState, browser::LoginError};

const SESSION_COOKIE: &str = "wb_session";
const CSRF_COOKIE: &str = "wb_csrf";

pub fn router(state: AppState) -> Router {
    Router::builder()
        .page(home)
        .route(login)
        .cookies()
        .app_context(state)
        .build()
}

#[derive(Deserialize)]
struct PageQuery {
    error: Option<String>,
}

#[page("/")]
async fn home(cx: &Cx, Form(query): Form<PageQuery>) -> Result<impl View> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let session = match session_id(cx, state) {
        Some(id) => state.store.load(id).await?,
        None => None,
    };
    let csrf = cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    set_cookie(cx, state, CSRF_COOKIE, csrf.clone());
    let error = match query.error.as_deref() {
        Some("credentials") => {
            Some("No se pudo iniciar sesión. Comprueba tu usuario y contraseña.")
        }
        Some("verification") => Some(
            "WodBuster solicita una verificación adicional y no permite completar el acceso automático.",
        ),
        Some("timeout") => Some("El acceso ha tardado demasiado. Vuelve a intentarlo."),
        Some("busy") => Some("Hay otros accesos en curso. Inténtalo de nuevo en unos segundos."),
        Some("storage") => Some("No se pudo guardar la sesión. Vuelve a intentarlo."),
        Some("unavailable") => {
            Some("No se pudo conectar con WodBuster o con el navegador. Vuelve a intentarlo.")
        }
        Some("empty") => Some("Introduce tu usuario y contraseña."),
        _ => None,
    };
    Ok(view! {
        <!DOCTYPE html>
        <html lang="es">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Acceso · WodBuster Jueces"</title>
                <style>(include_str!("style.css"))</style>
            </head>
            <body>
                <main>
                    <p class="brand">"WODBUSTER / JUECES"</p>
                    <section class="card" aria-labelledby="title">
                        if let Some(session) = session {
                            <p class="eyebrow">"SESIÓN GUARDADA"</p>
                            <h1 id="title">"Acceso completado"</h1>
                            <p>"Has accedido con " <strong>(session.username)</strong> "."</p>
                            <div class="notice success" role="status">
                                "La cookie .WBAuth está guardada para las próximas operaciones en Arena."
                            </div>
                        } else {
                            <p class="eyebrow">"WODBUSTER ARENA"</p>
                            <h1 id="title">"Inicia sesión"</h1>
                            <p class="intro">"Introduce tus credenciales de WodBuster para conectar con Arena."</p>
                            if let Some(error) = error {
                                <div class="notice error" role="alert">(error)</div>
                            }
                            <form method="post" action="/login">
                                <input type="hidden" name="csrf" value=(csrf)>
                                <label for="username">"Usuario / email"</label>
                                <input id="username" name="username" type="text" autocomplete="username"
                                    placeholder="Tu email de WodBuster" required="" maxlength="320" autofocus="">
                                <label for="password">"Contraseña"</label>
                                <input id="password" name="password" type="password" autocomplete="current-password"
                                    placeholder="Tu contraseña" required="" maxlength="1024">
                                <button type="submit">"Acceder y guardar sesión"</button>
                            </form>
                            <p class="footnote">"La contraseña no se guarda. La cookie de acceso se almacena cifrada."</p>
                        }
                    </section>
                </main>
            </body>
        </html>
    })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    csrf: String,
}

#[route(POST "/login")]
async fn login(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let expected = cookies(cx).signed(&state.cookie_key).get(CSRF_COOKIE);
    if !expected.is_some_and(|cookie| cookie.value() == form.csrf) {
        return Err(forbidden().into());
    }
    let username = form.username.trim();
    if username.is_empty()
        || form.password.is_empty()
        || username.len() > 320
        || form.password.len() > 1024
    {
        return Ok(see_other("/?error=empty"));
    }
    let cookie = match state.browser.login(username, &form.password).await {
        Ok(cookie) => cookie,
        Err(error) => {
            let code = match error {
                LoginError::Credentials => "credentials",
                LoginError::Verification => "verification",
                LoginError::Timeout => "timeout",
                LoginError::Unavailable => "unavailable",
                LoginError::Busy => "busy",
            };
            return Ok(see_other(format!("/?error={code}")));
        }
    };
    let id = session_id(cx, state).unwrap_or_else(Uuid::new_v4);
    if state.store.save(id, username, &cookie).await.is_err() {
        return Ok(see_other("/?error=storage"));
    }
    set_cookie(cx, state, SESSION_COOKIE, id.to_string());
    Ok(see_other("/"))
}

fn session_id(cx: &Cx, state: &AppState) -> Option<Uuid> {
    cookies(cx)
        .signed(&state.cookie_key)
        .get(SESSION_COOKIE)
        .and_then(|cookie| cookie.value().parse().ok())
}

fn set_cookie(cx: &Cx, state: &AppState, name: &'static str, value: String) {
    cookies(cx).signed(&state.cookie_key).add(
        Cookie::build((name, value))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Strict)
            .secure(state.secure_cookies)
            .max_age(time::Duration::days(30))
            .build(),
    );
}

fn no_cache(cx: &Cx) {
    response_headers(cx).append(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
