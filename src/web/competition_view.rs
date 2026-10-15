use super::*;
use topcoat::router::{
    error::{bad_request, not_found},
    path_param_segment,
};

fn requested_id(cx: &Cx) -> Result<i64> {
    path_param_segment(cx, "competition_id")
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad_request("Identificador de competición no válido").into())
}

#[page("/competitions/{competition_id}")]
pub(super) async fn detail_page(cx: &Cx, Form(query): Form<PageQuery>) -> Result<impl View> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(session_id) = session_id(cx, state) else {
        return Err(see_other("/").into());
    };
    let Some(session) = state.store.load(session_id).await? else {
        return Err(see_other("/").into());
    };
    let id = requested_id(cx)?;
    let competition = state
        .store
        .competitions(&session.username)
        .await?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(not_found)?;
    let snapshot = state
        .store
        .competition_detail(&session.username, id)
        .await?;
    let job = load_view::job_for_page(cx, &session.username, query.job, Some(id)).await?;
    let loading = job
        .as_ref()
        .is_some_and(|j| j.is_running() || j.state == "succeeded");
    let csrf = csrf_token(cx, state);
    let error = match query.error.as_deref() {
        Some("session") => Some("La sesión de Arena ha caducado. Vuelve a iniciar sesión."),
        Some("verification") => {
            Some("Arena solicita una verificación adicional. Vuelve a intentarlo.")
        }
        Some("busy") => {
            Some("Hay otras operaciones en curso. Vuelve a intentarlo en unos segundos.")
        }
        Some("access") => {
            Some("Ya no tienes acceso como juez o administrador. Refresca Mis competiciones.")
        }
        Some("storage") => Some("No se pudo guardar la información. Vuelve a intentarlo."),
        Some("timeout") => Some("La carga ha tardado demasiado. Vuelve a intentarlo."),
        Some(_) => Some("No se pudo cargar la información de Arena. Vuelve a intentarlo."),
        None => None,
    };
    layout::configure(
        cx,
        competition.name.clone(),
        true,
        Some(layout::Back::COMPETITIONS),
    );
    Ok(view! {
        <p class="eyebrow">"COMPETICIÓN"</p>
        <h1 id="title">(competition.name)</h1>
        load_view::notice(job: job)
        if let Some(error)=error { <div class="notice error" role="alert">(error)</div> }
        if let Some(snapshot)=snapshot {
            <p class="intro">(snapshot.categories.len()) " categorías · "
                (snapshot.categories.iter().map(|c|c.teams.len()).sum::<usize>()) " equipos"
            </p>
            if let Some(loaded_at)=snapshot.loaded_at { <p class="footnote">"Cargado el " (loaded_at)</p> }
            <form method="post" action=(format!("/competitions/{id}/refresh"))>
                <input type="hidden" name="csrf" value=(csrf)>
                <button type="submit" class="secondary" data-load-trigger="" disabled=(loading)>"Refrescar"</button>
            </form>
            if snapshot.categories.is_empty() { <p>"No hay categorías disponibles."</p> }
            for category in snapshot.categories {
                <details class="category">
                    <summary><strong>(category.name)</strong> <span>(category.teams.len()) " equipos"</span></summary>
                    <p class="reference">"Categoría " (category.id)</p>
                    <h2>"WODs"</h2>
                    if category.wods.is_empty() { <p>"No hay WODs disponibles."</p> }
                    for wod in &category.wods {
                        <details class="wod">
                            <summary>(wod.name.clone())</summary>
                            <p class="reference">"WOD " (wod.id)</p>
                            if !wod.description.is_empty() { <p class="description">(wod.description.clone())</p> }
                            if !wod.scoring.is_empty() { <p class="footnote">(wod.scoring.clone())</p> }
                            if let Some(format) = &wod.score_format { <p class="footnote">(format.summary())</p> }
                        </details>
                    }
                    <h2>"Equipos y puntuaciones"</h2>
                    if category.teams.is_empty() { <p>"No hay equipos disponibles."</p> }
                    for team in category.teams {
                        <details class="team">
                            <summary><strong>(team.name)</strong> <span>(team.points)</span></summary>
                            <p class="reference">"Referencia " (team.reference) if !team.bib.is_empty() { " · " (team.bib) }</p>
                            if !team.affiliate.is_empty() { <p class="footnote">(team.affiliate)</p> }
                            if let Some(members)=team.members.as_array() {
                                <ul class="members">
                                    for member in members { <li>(member["Nombre"].as_str().unwrap_or_default())</li> }
                                </ul>
                            }
                            <div class="score-table"><table>
                                <thead><tr><th>"WOD"</th><th>"Resultado"</th><th>"Puntos"</th><th>"Puesto"</th></tr></thead>
                                <tbody>
                                    for score in team.scores {
                                        <tr>
                                            <td>(category.wods.iter().find(|w|w.id==score.wod_id).map(|w|w.name.as_str()).unwrap_or("WOD"))</td>
                                            <td>(score.result.as_deref().unwrap_or("Sin puntuación"))</td>
                                            <td>(score.points.as_deref().unwrap_or("—"))</td>
                                            <td>(score.rank.as_deref().unwrap_or("—"))</td>
                                        </tr>
                                    }
                                </tbody>
                            </table></div>
                        </details>
                    }
                </details>
            }
        } else {
            <p class="intro">"Todavía no hay información guardada de esta competición."</p>
            <form method="post" action=(format!("/competitions/{id}/load"))>
                <input type="hidden" name="csrf" value=(csrf)>
                <button type="submit" data-load-trigger="" disabled=(loading)>"Cargar"</button>
            </form>
        }
    })
}

#[route(POST "/competitions/{competition_id}/load")]
pub(super) async fn load_detail(cx: &Cx, Form(form): Form<LoadForm>) -> Result<SeeOther> {
    sync_detail(cx, form, false).await
}

#[route(POST "/competitions/{competition_id}/refresh")]
pub(super) async fn refresh_detail(cx: &Cx, Form(form): Form<LoadForm>) -> Result<SeeOther> {
    sync_detail(cx, form, true).await
}

#[route(POST "/competitions/{competition_id}/delete")]
pub(super) async fn delete_competition(cx: &Cx, Form(form): Form<LoadForm>) -> Result<SeeOther> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(session_id) = session_id(cx, state) else {
        return Ok(see_other("/"));
    };
    let Some(session) = state.store.load(session_id).await? else {
        return Ok(see_other("/"));
    };
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|c| c.value() == form.csrf)
    {
        return Err(forbidden().into());
    }
    let id = requested_id(cx)?;
    match state.store.delete_competition(&session.username, id).await {
        Ok(true) => Ok(see_other("/competitions")),
        Ok(false) => Err(not_found().into()),
        Err(_) => Ok(see_other("/competitions?error=storage")),
    }
}

async fn sync_detail(cx: &Cx, form: LoadForm, refresh: bool) -> Result<SeeOther> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let Some(session_id) = session_id(cx, state) else {
        return Ok(see_other("/"));
    };
    let Some(session) = state.store.load(session_id).await? else {
        return Ok(see_other("/"));
    };
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|c| c.value() == form.csrf)
    {
        return Err(forbidden().into());
    }
    let id = requested_id(cx)?;
    if !state
        .store
        .competitions(&session.username)
        .await?
        .iter()
        .any(|c| c.id == id)
    {
        return Err(not_found().into());
    }
    let location = format!("/competitions/{id}");
    if !refresh
        && state
            .store
            .competition_detail(&session.username, id)
            .await?
            .is_some()
    {
        return Ok(see_other(location));
    }
    let (job, created) = state.store.begin_load(&session.username, Some(id)).await?;
    if created {
        crate::loads::start(state.clone(), session, &job);
    }
    Ok(see_other(job.location()))
}
