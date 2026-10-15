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
mod competition_view;
mod layout;
mod load_view;
mod photos;

use crate::{AppState, browser::LoginError};

const SESSION_COOKIE: &str = "wb_session";
const CSRF_COOKIE: &str = "wb_csrf";

pub fn router(state: AppState) -> Router {
    Router::builder()
        .layout(layout::app_layout)
        .page(home)
        .route(login)
        .page(my_competitions)
        .route(load_competitions)
        .route(refresh_competitions)
        .page(competition_view::detail_page)
        .route(competition_view::load_detail)
        .route(competition_view::refresh_detail)
        .route(competition_view::delete_competition)
        .route(load_view::status)
        .route(load_view::script)
        .page(photos::gallery)
        .route(photos::upload)
        .route(photos::status)
        .route(photos::image)
        .route(photos::refresh_photo)
        .route(photos::edit_photo)
        .route(photos::script)
        .cookies()
        .app_context(state)
        .build()
}

#[derive(Deserialize)]
struct PageQuery {
    error: Option<String>,
    job: Option<Uuid>,
}

#[page("/")]
async fn home(cx: &Cx, Form(query): Form<PageQuery>) -> Result<impl View> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let session = if query.error.as_deref() == Some("session") {
        None
    } else {
        match session_id(cx, state) {
            Some(id) => state.store.load(id).await?,
            None => None,
        }
    };
    let csrf = csrf_token(cx, state);
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
        Some("session") => Some("La sesión de WodBuster ha caducado. Inicia sesión de nuevo."),
        _ => None,
    };
    layout::configure(cx, "Acceso", false, None);
    Ok(view! {
        if let Some(session) = session {
            <p class="eyebrow">"SESIÓN GUARDADA"</p>
            <h1 id="title">"Acceso completado"</h1>
            <p>"Has accedido con " <strong>(session.username)</strong> "."</p>
            <form method="get" action="/competitions">
                <button type="submit">"Mis competiciones"</button>
            </form>
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
    })
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    csrf: String,
}

#[derive(Deserialize)]
struct CompetitionsQuery {
    error: Option<String>,
    loaded: Option<bool>,
    job: Option<Uuid>,
}

#[page("/competitions")]
async fn my_competitions(cx: &Cx, Form(query): Form<CompetitionsQuery>) -> Result<impl View> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(id) = session_id(cx, state) else {
        return Err(see_other("/").into());
    };
    let Some(session) = state.store.load(id).await? else {
        return Err(see_other("/").into());
    };
    let competitions = state.store.competitions(&session.username).await?;
    let job = load_view::job_for_page(cx, &session.username, query.job, None).await?;
    let loading = job
        .as_ref()
        .is_some_and(|j| j.is_running() || j.state == "succeeded");
    let loaded = query.loaded == Some(true) || job.as_ref().is_some_and(|j| j.state == "succeeded");
    let csrf = csrf_token(cx, state);
    let error = match query.error.as_deref() {
        Some("verification") => {
            Some("WodBuster solicita una verificación adicional. Vuelve a intentarlo.")
        }
        Some("timeout") => Some("La carga ha tardado demasiado. Vuelve a intentarlo."),
        Some("busy") => {
            Some("Hay otras operaciones en curso. Vuelve a intentarlo en unos segundos.")
        }
        Some("storage") => {
            Some("No se pudo actualizar la lista de competiciones. Vuelve a intentarlo.")
        }
        Some("invalid") => Some("No se pudo leer la página de competiciones de WodBuster."),
        Some("unavailable") => Some("No se pudo conectar con WodBuster o con el navegador."),
        _ => None,
    };
    layout::configure(cx, "Mis competiciones", false, Some(layout::Back::HOME));
    Ok(view! {
        <p class="eyebrow">"WODBUSTER ARENA"</p>
        <h1 id="title">"Mis competiciones"</h1>
        load_view::notice(job: job)
        if let Some(error) = error {
            <div class="notice error" role="alert">(error)</div>
        }
        if competitions.is_empty() {
            <p class="intro">
                if loaded {
                    "No hay competiciones en las que seas juez o administrador."
                } else {
                    "Todavía no hay competiciones guardadas."
                }
            </p>
            <form method="post" action="/competitions/load">
                <input type="hidden" name="csrf" value=(csrf.clone())>
                <button type="submit" data-load-trigger="" disabled=(loading)>"Cargar"</button>
            </form>
        } else {
            <ul class="competition-list">
                for competition in competitions {
                    <li class="competition-row">
                        <span>(competition.name.clone())</span>
                        <div class="competition-actions">
                            <a class="button small" href=(format!("/competitions/{}",competition.id))>"Ver competicion"</a>
                            <form method="post" action=(format!("/competitions/{}/delete",competition.id))>
                                <input type="hidden" name="csrf" value=(csrf.clone())>
                                <button type="submit" class="small danger" aria-label=(format!("Borrar {}",competition.name))>"Borrar"</button>
                            </form>
                        </div>
                    </li>
                }
            </ul>
            <form method="post" action="/competitions/refresh">
                <input type="hidden" name="csrf" value=(csrf.clone())>
                <button type="submit" class="secondary" data-load-trigger="" disabled=(loading)>"Refrescar competiciones"</button>
            </form>
        }
    })
}

#[derive(Deserialize)]
struct LoadForm {
    csrf: String,
}

#[route(POST "/competitions/load")]
async fn load_competitions(cx: &Cx, Form(form): Form<LoadForm>) -> Result<SeeOther> {
    sync_competitions(cx, form, false).await
}

#[route(POST "/competitions/refresh")]
async fn refresh_competitions(cx: &Cx, Form(form): Form<LoadForm>) -> Result<SeeOther> {
    sync_competitions(cx, form, true).await
}

async fn sync_competitions(cx: &Cx, form: LoadForm, refresh: bool) -> Result<SeeOther> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(id) = session_id(cx, state) else {
        return Ok(see_other("/"));
    };
    let Some(session) = state.store.load(id).await? else {
        return Ok(see_other("/"));
    };
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|cookie| cookie.value() == form.csrf)
    {
        return Err(forbidden().into());
    }
    // Si ya hay datos, se sirve la lista guardada sin volver a consultar Arena.
    if !refresh
        && !state
            .store
            .competitions(&session.username)
            .await?
            .is_empty()
    {
        return Ok(see_other("/competitions"));
    }
    let (job, created) = state.store.begin_load(&session.username, None).await?;
    if created {
        crate::loads::start(state.clone(), session, &job);
    }
    Ok(see_other(job.location()))
}

fn csrf_token(cx: &Cx, state: &AppState) -> String {
    let csrf = cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .map(|cookie| cookie.value().to_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    set_cookie(cx, state, CSRF_COOKIE, csrf.clone());
    csrf
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
