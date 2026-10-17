use super::*;
use crate::photos::{NewPhoto, Photo, PhotoFilter};
use serde::Serialize;
use topcoat::{
    router::{
        content::{Json, multipart::Multipart},
        error::{bad_request, not_found, unauthorized},
        path_param_segment,
        response::IntoResponse,
    },
    view::component,
};
mod editor;
pub(super) use editor::{delete_photo, edit_photo, refresh_photo};

fn competition_id(cx: &Cx) -> Result<i64> {
    path_param_segment(cx, "competition_id")
        .parse()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| bad_request("Competición no válida").into())
}

pub(super) async fn account(
    cx: &Cx,
    competition: i64,
) -> Result<(Uuid, crate::storage::SavedSession)> {
    let state: &AppState = app_context(cx);
    let id = session_id(cx, state).ok_or_else(unauthorized)?;
    let session = state.store.load(id).await?.ok_or_else(unauthorized)?;
    if !state
        .store
        .competitions(&session.username)
        .await?
        .iter()
        .any(|c| c.id == competition)
    {
        return Err(not_found().into());
    }
    Ok((id, session))
}

#[derive(Serialize)]
pub(super) struct Uploaded {
    id: Uuid,
    location: String,
}

#[route(POST "/competitions/{competition_id}/photos")]
pub(super) async fn upload(cx: &Cx, mut body: Multipart) -> Result<Json<Uploaded>> {
    no_cache(cx);
    let state: &AppState = app_context(cx);
    let competition = competition_id(cx)?;
    let (session_id, session) = account(cx, competition).await?;
    let mut csrf = None;
    let mut id = None;
    let mut image_bytes = None;
    while let Some(field) = body.next_field().await? {
        match field.name() {
            Some("csrf") if csrf.is_none() => csrf = Some(field.text().await?),
            Some("capture_id") if id.is_none() => {
                id = Some(
                    field
                        .text()
                        .await?
                        .parse::<Uuid>()
                        .map_err(|_| bad_request("Captura no válida"))?,
                )
            }
            Some("photo") if image_bytes.is_none() => {
                image_bytes = Some(field.bytes().await?.to_vec())
            }
            _ => return Err(bad_request("Formulario de foto no válido").into()),
        }
    }
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|expected| Some(expected.value()) == csrf.as_deref())
    {
        return Err(forbidden().into());
    }
    if state
        .store
        .competition_detail(&session.username, competition)
        .await?
        .is_none()
    {
        return Err(bad_request("Carga primero los detalles de la competición").into());
    }
    let bytes = image_bytes.ok_or_else(|| bad_request("Falta la foto"))?;
    let (bytes, media_type, width, height) = tokio::task::spawn_blocking(move || {
        crate::photos::validate_image(&bytes).map(|(mime, w, h)| (bytes, mime, w, h))
    })
    .await
    .map_err(|_| bad_request("No se pudo leer la imagen"))?
    .map_err(|_| {
        bad_request("La foto debe ser JPEG, PNG o WebP, de hasta 1,5 MB y 4096 píxeles por lado")
    })?;
    let id = state
        .store
        .save_photo(NewPhoto {
            id: id.ok_or_else(|| bad_request("Falta el identificador de captura"))?,
            username: &session.username,
            competition_id: competition,
            session_id,
            image: &bytes,
            media_type,
            width,
            height,
            model: &crate::ai::model_name(),
        })
        .await?;
    Ok(Json(Uploaded {
        id,
        location: format!("/competitions/{competition}/photos"),
    }))
}

#[derive(Deserialize)]
pub(super) struct PhotosQuery {
    page: Option<u32>,
    category_id: Option<i64>,
    wod_id: Option<i64>,
    team_reference: Option<String>,
    issues: Option<bool>,
    unsynced: Option<bool>,
}

impl PhotosQuery {
    fn filter(&self) -> PhotoFilter {
        PhotoFilter {
            category_id: self.category_id,
            wod_id: self.wod_id,
            team_reference: self.team_reference.clone(),
            issues: self.issues.unwrap_or(false),
            unsynced: self.unsynced.unwrap_or(false),
        }
    }
}

pub(super) fn gallery_location(competition: i64, filter: &PhotoFilter, page: u32) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(id) = filter.category_id {
        query.append_pair("category_id", &id.to_string());
    }
    if let Some(id) = filter.wod_id {
        query.append_pair("wod_id", &id.to_string());
    }
    if let Some(reference) = &filter.team_reference {
        query.append_pair("team_reference", reference);
    }
    if filter.issues {
        query.append_pair("issues", "true");
    }
    if filter.unsynced {
        query.append_pair("unsynced", "true");
    }
    query.append_pair("page", &page.to_string());
    format!("/competitions/{competition}/photos?{}", query.finish())
}

fn filter_label(
    filter: &PhotoFilter,
    detail: Option<&crate::details::CompetitionDetail>,
) -> Result<Option<String>> {
    let Some(category_id) = filter.category_id else {
        if filter.wod_id.is_some() || filter.team_reference.is_some() {
            return Err(
                bad_request("Selecciona una categoría para filtrar por WOD o equipo").into(),
            );
        }
        let label = match (filter.issues, filter.unsynced) {
            (true, true) => Some("Fotos con problemas · No sincronizadas con Wodbuster"),
            (true, false) => Some("Fotos sin categorizar o con problemas"),
            (false, true) => Some("Fotos no sincronizadas con Wodbuster"),
            (false, false) => None,
        };
        return Ok(label.map(str::to_owned));
    };
    let category = detail
        .and_then(|d| d.categories.iter().find(|c| c.id == category_id))
        .ok_or_else(not_found)?;
    let mut names = vec![category.name.as_str()];
    if filter.issues {
        names.push("Sin categorizar o con problemas");
    }
    if filter.unsynced {
        names.push("No sincronizadas con Wodbuster");
    }
    if let Some(id) = filter.wod_id {
        names.push(
            &category
                .wods
                .iter()
                .find(|w| w.id == id)
                .ok_or_else(not_found)?
                .name,
        );
    }
    if let Some(reference) = &filter.team_reference {
        names.push(
            &category
                .teams
                .iter()
                .find(|t| t.reference == *reference)
                .ok_or_else(not_found)?
                .name,
        );
    }
    Ok(Some(names.join(" · ")))
}

#[page("/competitions/{competition_id}/photos")]
pub(super) async fn gallery(cx: &Cx, Form(query): Form<PhotosQuery>) -> Result<impl View> {
    no_cache(cx);
    let competition = competition_id(cx)?;
    let (_, session) = account(cx, competition).await?;
    let state: &AppState = app_context(cx);
    let page = query.page.unwrap_or(0);
    let filter = query.filter();
    let csrf = csrf_token(cx, state);
    let detail = state
        .store
        .competition_detail(&session.username, competition)
        .await?;
    let context = editor::context(detail.as_ref());
    let label = filter_label(&filter, detail.as_ref())?;
    let filtered = label.is_some();
    let photos = state
        .store
        .filtered_photos(
            &session.username,
            competition,
            i64::from(page) * 20,
            &filter,
        )
        .await?;
    let status_location =
        gallery_location(competition, &filter, page).replace("/photos?", "/photos/status?");
    layout::configure(
        cx,
        "Fotos y puntuaciones",
        true,
        Some(layout::Back::competition(competition)),
    );
    Ok(view! {
        <p class="eyebrow">"PUNTUACIONES CON IA"</p>
        <h1 id="title">"Fotos y estado"</h1>
        if let Some(label) = label {
            <p class="photo-filter-label">(label)</p>
            <a class="button small" href=(format!("/competitions/{competition}/photos"))>"Ver todas las fotos"</a>
        }
        if filter.issues {
            <p class="footnote">"Incluye fotos sin una categoría válida, duplicadas, puntuaciones distintas de Wodbuster, lecturas no comparables, errores, envíos sin confirmar y avisos de revisión de la IA (needs_retake)."</p>
        }
        if filter.unsynced {
            <p class="footnote">"Muestra las puntuaciones que difieren de los últimos datos consultados en Arena. Se utiliza la última foto añadida de cada categoría, WOD y equipo. Pulsa «Comprobar sincronización» en la competición para actualizar la comparación."</p>
        }
        <p class="intro">"Las fotos se procesan en segundo plano y las puntuaciones válidas se guardan automáticamente en Wodbuster."</p>
        <div data-photo-gallery=(status_location) data-photo-context=(context) data-photo-filtered=(filtered.to_string())>
            <p class="notice success" data-photo-list-feedback="" role="status" hidden=""></p>
            <p data-photo-empty="" hidden=(!photos.is_empty())>"No hay fotos en esta página para la selección actual."</p>
            <p class="notice loading" data-photo-list-changed="" hidden="">"La lista de fotos ha cambiado. " <a href=(gallery_location(competition, &filter, page))>"Actualizar vista"</a></p>
            for photo in &photos { photo_card(photo: photo, competition: competition, csrf: &csrf) }
        </div>
        <div class="photo-navigation">
            if page>0 { <a class="button" href=(gallery_location(competition, &filter, page-1))>"Anteriores"</a> }
            if photos.len()==20 { <a class="button" href=(gallery_location(competition, &filter, page+1))>"Siguientes"</a> }
        </div>
    })
}

#[component]
async fn photo_card(photo: &Photo, competition: i64, csrf: &str) -> Result<impl View> {
    let errors = photo.error.clone().unwrap_or_else(|| {
        photo
            .validation_errors
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let target = target(photo.prepared.as_ref());
    Ok(view! {
        <article class="photo-card" data-photo-id=(photo.id.to_string()) data-photo-status=(photo.state.clone()) data-photo-reading=(photo.reading.as_ref().unwrap_or(&serde_json::Value::Null).to_string())>
            <a href=(format!("/competitions/{competition}/photos/{}/image",photo.id)) target="_blank" rel="noopener">
                <img src=(format!("/competitions/{competition}/photos/{}/image",photo.id)) alt="Foto de la puntuación" loading="lazy" width=(photo.width) height=(photo.height)>
            </a>
            <div>
                <div class="photo-heading">
                    <h2 data-photo-state="" role="status">(photo.label())</h2>
                    <div class="photo-card-actions">
                    <form action=(format!("/competitions/{competition}/photos/{}/refresh",photo.id)) method="post" data-photo-refresh="">
                        <input type="hidden" name="csrf" value=(csrf)>
                        <input type="hidden" name="revision" value=(photo.revision)>
                        <button class="small secondary photo-refresh" type="submit" disabled=(photo.busy()) title="Volver a analizar la foto y guardar la puntuación en Wodbuster" aria-label="Volver a analizar y guardar esta foto"><span data-photo-refresh-icon="" aria-hidden="true">"↻"</span></button>
                    </form>
                    <button type="button" class="small danger" data-photo-delete-open="" disabled=(photo.busy())>"Borrar foto"</button>
                    </div>
                </div>
                <dialog data-photo-delete-dialog="" aria-labelledby=(format!("delete-title-{}",photo.id))>
                    <h2 id=(format!("delete-title-{}",photo.id))>"¿Borrar esta foto?"</h2>
                    <p>"Se eliminarán la imagen y sus datos extraídos. La puntuación guardada en Wodbuster se conservará."</p>
                    <form action=(format!("/competitions/{competition}/photos/{}/delete",photo.id)) method="post" data-photo-delete="">
                        <input type="hidden" name="csrf" value=(csrf)>
                        <input type="hidden" name="revision" value=(photo.revision)>
                        <button type="button" class="secondary" data-photo-delete-cancel="" autofocus="">"Cancelar"</button>
                        <button type="submit" class="danger">"Borrar foto definitivamente"</button>
                        <p data-photo-delete-error="" role="alert" hidden=""></p>
                    </form>
                </dialog>
                <p class="notice loading photo-progress" data-photo-action-message="" role="status" aria-live="polite" aria-atomic="true" hidden=(!photo.busy())>(photo.progress())</p>
                <p class="footnote">(photo.created_at.clone()) " · " (photo.bytes/1024) " KB · " (photo.width) " × " (photo.height)</p>
                <p class="reference" data-photo-model="">"Modelo: " (photo.provider.clone()) " / " (photo.model.clone())</p>
                <p class=(if photo.sync_status=="synced" {"notice success"} else {"notice warning"}) data-photo-sync="">(photo.sync_label())</p>
                <p class="reference" data-photo-sync-time="">(photo.sync_checked_at.as_ref().map(|at|format!("Datos de Arena: {at}")).unwrap_or_default())</p>
                <p class="notice warning" data-photo-duplicate="" role="alert" hidden=(photo.duplicate_count<2)>(photo.duplicate_label())</p>
                <p data-photo-target="">(target)</p>
                <div class="notice warning" data-photo-warning="" role="alert" hidden=(!photo.warning())>"Aviso: la puntuación se ha guardado, pero la IA ha indicado que hace falta revisar la foto (needs_retake)."</div>
                <p class="notice error" data-photo-error="" hidden=(errors.is_empty())>(errors)</p>
                editor::table(photo: photo, competition: competition, csrf: csrf)
            </div>
        </article>
    })
}

fn target(prepared: Option<&serde_json::Value>) -> String {
    prepared
        .map(|p| {
            ["category_name", "wod_name", "team_name"]
                .iter()
                .filter_map(|key| p[*key].as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default()
}

#[derive(Serialize)]
pub(super) struct PhotoStatus {
    #[serde(flatten)]
    photo: Photo,
    label: &'static str,
    warning: bool,
    target: String,
    busy: bool,
    progress: &'static str,
    sync_label: &'static str,
    duplicate_label: &'static str,
}

#[route(GET "/competitions/{competition_id}/photos/status")]
pub(super) async fn status(
    cx: &Cx,
    Form(query): Form<PhotosQuery>,
) -> Result<Json<Vec<PhotoStatus>>> {
    no_cache(cx);
    let competition = competition_id(cx)?;
    let (_, session) = account(cx, competition).await?;
    let state: &AppState = app_context(cx);
    let filter = query.filter();
    let detail = state
        .store
        .competition_detail(&session.username, competition)
        .await?;
    filter_label(&filter, detail.as_ref())?;
    let photos = state
        .store
        .filtered_photos(
            &session.username,
            competition,
            i64::from(query.page.unwrap_or(0)) * 20,
            &filter,
        )
        .await?;
    Ok(Json(
        photos
            .into_iter()
            .map(|photo| PhotoStatus {
                label: photo.label(),
                warning: photo.warning(),
                target: target(photo.prepared.as_ref()),
                busy: photo.busy(),
                progress: photo.progress(),
                sync_label: photo.sync_label(),
                duplicate_label: photo.duplicate_label(),
                photo,
            })
            .collect(),
    ))
}

#[route(GET "/competitions/{competition_id}/photos/{photo_id}/image")]
pub(super) async fn image(cx: &Cx) -> Result<impl IntoResponse> {
    no_cache(cx);
    let competition = competition_id(cx)?;
    let (_, session) = account(cx, competition).await?;
    let id = path_param_segment(cx, "photo_id")
        .parse::<Uuid>()
        .map_err(|_| not_found())?;
    let state: &AppState = app_context(cx);
    let (bytes, mime) = state
        .store
        .photo_image(&session.username, competition, id)
        .await?
        .ok_or_else(not_found)?;
    Ok((
        [
            (
                topcoat::router::header::CONTENT_TYPE,
                HeaderValue::from_str(&mime).unwrap(),
            ),
            (
                topcoat::router::header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        bytes,
    ))
}

#[route(GET "/assets/photos.js")]
pub(super) async fn script() -> Result<impl IntoResponse> {
    Ok((
        [(
            topcoat::router::header::CONTENT_TYPE,
            HeaderValue::from_static("text/javascript; charset=utf-8"),
        )],
        include_str!("photos.js"),
    ))
}
