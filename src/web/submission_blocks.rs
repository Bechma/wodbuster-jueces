use super::*;
use crate::{
    details::CompetitionDetail,
    submission_blocks::{SubmissionBlock, normalize_wod_name},
};
use std::collections::BTreeMap;
use topcoat::router::{error::bad_request, path_param_segment};

fn competition_id(cx: &Cx) -> Result<i64> {
    path_param_segment(cx, "competition_id")
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad_request("Competición no válida").into())
}

#[derive(Deserialize)]
pub(super) struct BlocksQuery {
    saved: Option<bool>,
}

fn scope_label(block: &SubmissionBlock, detail: Option<&CompetitionDetail>) -> String {
    let category = if block.category_id == 0 {
        "Todas las categorías".into()
    } else {
        detail
            .and_then(|d| d.categories.iter().find(|c| c.id == block.category_id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("Categoría {}", block.category_id))
    };
    let wod = if !block.wod_name.is_empty() {
        detail
            .and_then(|d| {
                d.categories
                    .iter()
                    .flat_map(|c| &c.wods)
                    .find(|w| normalize_wod_name(&w.name) == block.wod_name)
            })
            .map(|w| w.name.trim().to_owned())
            .unwrap_or_else(|| block.wod_name.clone())
    } else if block.wod_id == 0 {
        "Todos los WODs".into()
    } else {
        detail
            .and_then(|d| {
                d.categories
                    .iter()
                    .filter(|c| block.category_id == 0 || c.id == block.category_id)
                    .flat_map(|c| &c.wods)
                    .find(|w| w.id == block.wod_id)
            })
            .map(|w| w.name.clone())
            .unwrap_or_else(|| format!("WOD {}", block.wod_id))
    };
    format!("{category} · {wod}")
}

#[page("/competitions/{competition_id}/submission-blocks")]
pub(super) async fn blocks_page(cx: &Cx, Form(query): Form<BlocksQuery>) -> Result<impl View> {
    no_cache(cx);
    let id = competition_id(cx)?;
    let (_, session) = photos::account(cx, id).await?;
    let state: &AppState = app_context(cx);
    let detail = state
        .store
        .competition_detail(&session.username, id)
        .await?;
    let blocks = state.store.submission_blocks(&session.username, id).await?;
    let csrf = csrf_token(cx, state);
    let wods: BTreeMap<_, _> = detail
        .iter()
        .flat_map(|d| &d.categories)
        .flat_map(|c| &c.wods)
        .filter(|w| !w.name.trim().is_empty())
        .map(|w| (normalize_wod_name(&w.name), w.name.trim().to_owned()))
        .collect();
    layout::configure(
        cx,
        "Bloquear subidas",
        true,
        Some(layout::Back::competition(id)),
    );
    Ok(view! {
        <p class="eyebrow">"CONTROL DE PUNTUACIONES"</p>
        <h1 id="title">"Bloquear subidas"</h1>
        <p>"Los bloqueos se aplican a tu cuenta en esta competición. Los WODs se agrupan por nombre: selecciona Giralda y Todas las categorías para bloquearlo en todas ellas, aunque tenga distintos IDs."</p>
        <p>"La foto se conserva con un error y marcada para revisión. El bloqueo también se aplica a fotos pendientes y correcciones manuales. Tras desbloquear, reintenta la foto desde la galería."</p>
        if query.saved == Some(true) {
            <div class="notice success" role="status">"Bloqueos actualizados."</div>
        }
        <h2>"Nuevo bloqueo"</h2>
        if let Some(detail) = &detail {
            <form method="post" action=(format!("/competitions/{id}/submission-blocks")) class="submission-block-form">
                <input type="hidden" name="csrf" value=(csrf.clone())>
                <input type="hidden" name="blocked" value="true">
                <label for="block-category">"Categoría"</label>
                <select id="block-category" name="category_id">
                    <option value="0">"Todas las categorías"</option>
                    for category in &detail.categories {
                        <option value=(category.id)>(category.name.clone()) " · " (category.id)</option>
                    }
                </select>
                <label for="block-wod">"WOD"</label>
                <select id="block-wod" name="wod_name">
                    <option value="">"Todos los WODs"</option>
                    for (wod_name, name) in &wods {
                        <option value=(wod_name)>(name)</option>
                    }
                </select>
                <p class="footnote">"Selecciona al menos una categoría o un WOD. Si eliges ambos, el WOD debe pertenecer a esa categoría."</p>
                <button type="submit" class="danger">"Bloquear subidas de puntuaciones"</button>
            </form>
        } else {
            <p>"Carga los detalles de la competición para crear bloqueos."</p>
        }
        <h2>"Bloqueos activos"</h2>
        if blocks.is_empty() { <p>"No hay bloqueos activos."</p> }
        <ul class="competition-list">
            for block in &blocks {
                <li class="competition-row">
                    <span>(scope_label(block, detail.as_ref()))</span>
                    <form method="post" action=(format!("/competitions/{id}/submission-blocks")) class="competition-actions">
                        <input type="hidden" name="csrf" value=(csrf.clone())>
                        <input type="hidden" name="category_id" value=(block.category_id)>
                        <input type="hidden" name="wod_id" value=(block.wod_id)>
                        <input type="hidden" name="wod_name" value=(block.wod_name.clone())>
                        <input type="hidden" name="blocked" value="false">
                        <button type="submit" class="small secondary">"Desbloquear"</button>
                    </form>
                </li>
            }
        </ul>
        <p class="footnote">"Refrescar la competición conserva estos bloqueos. Un envío que ya esté en curso terminará antes de confirmar un nuevo bloqueo."</p>
    })
}

#[derive(Deserialize)]
pub(super) struct BlockForm {
    csrf: String,
    category_id: i64,
    #[serde(default)]
    wod_id: i64,
    #[serde(default)]
    wod_name: String,
    blocked: bool,
}

#[route(POST "/competitions/{competition_id}/submission-blocks")]
pub(super) async fn set_block(cx: &Cx, Form(form): Form<BlockForm>) -> Result<SeeOther> {
    no_cache(cx);
    let id = competition_id(cx)?;
    let (_, session) = photos::account(cx, id).await?;
    let state: &AppState = app_context(cx);
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|c| c.value() == form.csrf)
    {
        return Err(forbidden().into());
    }
    let wod_name = normalize_wod_name(&form.wod_name);
    if form.category_id < 0
        || form.wod_id < 0
        || (form.wod_id > 0 && !wod_name.is_empty())
        || (form.category_id == 0 && form.wod_id == 0 && wod_name.is_empty())
    {
        return Err(bad_request("Selecciona una categoría y/o un WOD válidos").into());
    }
    if form.blocked {
        let detail = state
            .store
            .competition_detail(&session.username, id)
            .await?
            .ok_or_else(|| bad_request("Carga primero los detalles de la competición"))?;
        if form.wod_id != 0
            || !detail.categories.iter().any(|c| {
                (form.category_id == 0 || c.id == form.category_id)
                    && (wod_name.is_empty()
                        || c.wods
                            .iter()
                            .any(|w| normalize_wod_name(&w.name) == wod_name))
            })
        {
            return Err(
                bad_request("La categoría o el WOD no pertenecen al ámbito seleccionado").into(),
            );
        }
    }
    state
        .store
        .set_submission_block(
            &session.username,
            id,
            SubmissionBlock {
                category_id: form.category_id,
                wod_id: form.wod_id,
                wod_name,
            },
            form.blocked,
        )
        .await?;
    Ok(see_other(format!(
        "/competitions/{id}/submission-blocks?saved=true"
    )))
}
