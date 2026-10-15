use super::*;
use crate::loads::LoadJob;
use serde::Serialize;
use topcoat::router::{
    content::Json,
    error::{not_found, unauthorized},
    path_param_segment,
    response::IntoResponse,
};
use topcoat::view::component;

pub(super) async fn job_for_page(
    cx: &Cx,
    username: &str,
    requested: Option<Uuid>,
    target: Option<i64>,
) -> Result<Option<LoadJob>> {
    let state: &AppState = app_context(cx);
    let job = if let Some(id) = requested {
        let job = state
            .store
            .load_job(username, id)
            .await?
            .filter(|j| j.competition_id == target)
            .ok_or_else(not_found)?;
        Some(job)
    } else {
        state.store.running_load(username, target).await?
    };
    Ok(job)
}

#[derive(Serialize)]
pub(super) struct Status {
    state: String,
    message: &'static str,
    error_code: Option<String>,
    refresh_url: String,
}

#[route(GET "/loads/{job_id}")]
pub(super) async fn status(cx: &Cx) -> Result<Json<Status>> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(id) = session_id(cx, state) else {
        return Err(unauthorized().into());
    };
    let Some(session) = state.store.load(id).await? else {
        return Err(unauthorized().into());
    };
    let id = path_param_segment(cx, "job_id")
        .parse::<Uuid>()
        .map_err(|_| not_found())?;
    let job = state
        .store
        .load_job(&session.username, id)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(Status {
        message: job.message(),
        refresh_url: job.refresh_url(),
        state: job.state,
        error_code: job.error_code,
    }))
}

#[route(GET "/assets/load-status.js")]
pub(super) async fn script() -> Result<impl IntoResponse> {
    Ok((
        [(
            topcoat::router::header::CONTENT_TYPE,
            HeaderValue::from_static("text/javascript; charset=utf-8"),
        )],
        include_str!("load-status.js"),
    ))
}

#[component]
pub(super) async fn notice(job: Option<LoadJob>) -> Result<impl View> {
    Ok(view! {
        if let Some(job) = job {
            <div class=(if job.state == "failed" { "notice error load-notice" } else if job.state == "succeeded" { "notice success load-notice" } else { "notice loading load-notice" })
                role="status" aria-live="polite" aria-busy=(if job.is_running() {"true"} else {"false"})
                data-load-status=(format!("/loads/{}",job.id)) data-load-state=(job.state.clone())>
                <p data-load-message="">(job.message())</p>
                if job.state == "succeeded" {
                    <form method="get" action=(job.refresh_url()) data-load-action="">
                        <button type="submit" class="secondary">"Actualizar página"</button>
                    </form>
                }
                if job.error_code.as_deref() == Some("session") {
                    <form method="get" action="/" data-load-action="">
                        <input type="hidden" name="error" value="session">
                        <button type="submit" class="secondary">"Iniciar sesión"</button>
                    </form>
                }
            </div>
        }
    })
}
