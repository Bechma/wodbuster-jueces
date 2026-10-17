use super::*;
use crate::{
    details::CompetitionDetail,
    scorecard::{Scorecard, ScorecardReading, WodReading, evaluate_scorecard},
    scoring::{MinutesSeconds, ScoreValue},
};
use serde_json::json;
use topcoat::router::StatusCode;

pub(super) fn context(detail: Option<&CompetitionDetail>) -> String {
    json!(detail.map(|d| d.categories.iter().map(|c| json!({
        "id":c.id,"name":c.name,
        "teams":c.teams.iter().map(|t|json!({"reference":t.reference,"name":t.name})).collect::<Vec<_>>(),
        "wods":c.wods.iter().map(|w|json!({"id":w.id,"name":w.name,"kind":w.score_format.as_ref().map(|f| f.kind),"tie_break":w.score_format.as_ref().is_some_and(|f|f.fields.iter().any(|field|field.role==crate::scoring::FieldRole::TieBreak))})).collect::<Vec<_>>()
    })).collect::<Vec<_>>()).unwrap_or_default()).to_string()
}

#[component]
pub(super) async fn table(photo: &Photo, competition: i64, csrf: &str) -> Result<impl View> {
    Ok(view! {
        <form action=(format!("/competitions/{competition}/photos/{}/edit",photo.id)) method="post" data-photo-edit="">
            <input type="hidden" name="csrf" value=(csrf)>
            <input type="hidden" name="revision" value=(photo.revision)>
            <fieldset disabled=(photo.busy())>
                <legend>"Campos extraídos"</legend>
                <table class="photo-fields"><tbody>
                    <tr>
                        <th scope="row">"Juez detectado"</th>
                        <td><span data-photo-judge="">(photo.reading.as_ref().and_then(|r| r["judge_name"].as_str()).filter(|name| !name.trim().is_empty()).unwrap_or("No detectado"))</span> <span class="footnote">"(informativo)"</span></td>
                    </tr>
                    for (name, label) in [
                        ("category_id", "Categoría"), ("team_reference", "Equipo"),
                        ("results_count", "Número de WODs"),
                        ("wod_id", "WOD"), ("kind", "Resultado"), ("time", "Tiempo"),
                        ("repetitions", "Repeticiones"), ("kilograms", "Peso (kg)"),
                        ("tie_break", "Desempate"),
                        ("wod_id_2", "Segundo WOD"), ("kind_2", "Segundo resultado"),
                        ("time_2", "Tiempo del segundo WOD"), ("repetitions_2", "Repeticiones del segundo WOD"),
                        ("kilograms_2", "Peso del segundo WOD (kg)"), ("tie_break_2", "Desempate del segundo WOD"),
                        ("needs_retake", "Revisar foto"),
                        ("retake_reason", "Motivo"),
                    ] { field_row(name: name, label: label) }
                </tbody></table>
                <button class="small" type="submit" data-photo-save="" disabled="">"Guardar cambios y encolar"</button>
            </fieldset>
            <noscript>"Activa JavaScript para editar los datos. El botón ↻ permite repetir el análisis."</noscript>
        </form>
    })
}

#[component]
async fn field_row(name: &'static str, label: &'static str) -> Result<impl View> {
    let base = name.strip_suffix("_2").unwrap_or(name);
    let score_field =
        matches!(base, "time" | "repetitions" | "kilograms" | "tie_break").then_some(name);
    Ok(view! {
        <tr data-score-field=(score_field) data-photo-field=(name)>
            <th scope="row">(label)</th>
            <td>
                <div class="photo-field-value">
                    <span data-field-value=(name)>"Sin datos"</span>
                    <button type="button" class="secondary" data-field-edit=(name) aria-label=(format!("Editar {label}")) aria-expanded="false">"Editar"</button>
                </div>
                <div data-field-control=(name) hidden="">
                    if name=="results_count" {
                        <select name=(name) aria-label=(label)><option value="1">"WOD único"</option><option value="2">"WOD múltiple (dos resultados)"</option></select>
                    } else if matches!(base, "category_id" | "team_reference" | "wod_id" | "kind") {
                        <select name=(name) aria-label=(label) required=""></select>
                    } else if name=="needs_retake" {
                        <select name=(name) aria-label=(label)><option value="false">"No"</option><option value="true">"Sí"</option></select>
                    } else if matches!(base, "time" | "tie_break") {
                        <input name=(name) aria-label=(format!("{label} (mm:ss)")) placeholder="mm:ss" inputmode="numeric" pattern="[0-9]+:[0-5][0-9]">
                    } else if matches!(base, "repetitions" | "kilograms") {
                        <input name=(name) aria-label=(label) type="number" min="0" max=((base=="repetitions").then_some("4294967295")) step=(if base=="repetitions" {"1"} else {"any"})>
                    } else {
                        <input name=(name) aria-label=(label) maxlength="2000">
                    }
                </div>
            </td>
        </tr>
    })
}

fn one_result() -> u8 {
    1
}

#[derive(Clone, Deserialize)]
pub(crate) struct EditForm {
    csrf: String,
    revision: i64,
    category_id: String,
    wod_id: String,
    team_reference: String,
    kind: String,
    #[serde(default = "one_result")]
    results_count: u8,
    #[serde(default)]
    wod_id_2: String,
    #[serde(default)]
    kind_2: String,
    #[serde(default)]
    time_2: String,
    #[serde(default)]
    repetitions_2: String,
    #[serde(default)]
    kilograms_2: String,
    #[serde(default)]
    tie_break_2: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    repetitions: String,
    #[serde(default)]
    kilograms: String,
    #[serde(default)]
    tie_break: String,
    #[serde(default)]
    needs_retake: bool,
    #[serde(default)]
    retake_reason: String,
}

impl EditForm {
    fn scorecard(
        &self,
        judge_name: Option<String>,
    ) -> std::result::Result<Scorecard, &'static str> {
        if !(1..=2).contains(&self.results_count) {
            return Err("La scorecard debe tener uno o dos resultados.");
        }
        let mut card = Scorecard::from(self.reading(judge_name.clone())?);
        if self.results_count == 2 {
            let second = Self {
                wod_id: self.wod_id_2.clone(),
                kind: self.kind_2.clone(),
                time: self.time_2.clone(),
                repetitions: self.repetitions_2.clone(),
                kilograms: self.kilograms_2.clone(),
                tie_break: self.tie_break_2.clone(),
                ..self.clone()
            }
            .reading(judge_name)?;
            card.results.push(WodReading {
                wod_id: second.wod_id,
                score_value: second.score_value,
            });
        }
        Ok(card)
    }

    fn reading(
        &self,
        judge_name: Option<String>,
    ) -> std::result::Result<ScorecardReading, &'static str> {
        let time =
            || MinutesSeconds::parse(&self.time).ok_or("El tiempo debe tener formato mm:ss.");
        let reps = || {
            self.repetitions
                .trim()
                .parse::<u32>()
                .map_err(|_| "Las repeticiones deben ser un entero no negativo.")
        };
        let tie_break = if self.tie_break.trim().is_empty() {
            None
        } else {
            Some(
                MinutesSeconds::parse(&self.tie_break)
                    .ok_or("El desempate debe tener formato mm:ss.")?,
            )
        };
        let score_value = match self.kind.as_str() {
            "completed" => ScoreValue::Completed {
                time: time()?,
                tie_break,
            },
            "not_completed" => ScoreValue::NotCompleted {
                repetitions: reps()?,
                tie_break,
            },
            "time" if tie_break.is_none() => ScoreValue::Time { time: time()? },
            "repetitions" if tie_break.is_none() => ScoreValue::Repetitions {
                repetitions: reps()?,
            },
            "weight" if tie_break.is_none() => {
                let kg = self
                    .kilograms
                    .trim()
                    .replace(',', ".")
                    .parse::<f64>()
                    .map_err(|_| "El peso debe ser un número no negativo en kg.")?;
                if !kg.is_finite() || kg < 0.0 {
                    return Err("El peso debe ser un número finito no negativo en kg.");
                }
                ScoreValue::Weight { kilograms: kg }
            }
            _ => return Err("Tipo de resultado o desempate no válido."),
        };
        if self.retake_reason.chars().count() > 2000 {
            return Err("El motivo de revisión es demasiado largo.");
        }
        Ok(ScorecardReading {
            category_id: Some(
                self.category_id
                    .parse()
                    .map_err(|_| "Categoría no válida.")?,
            ),
            wod_id: Some(self.wod_id.parse().map_err(|_| "WOD no válido.")?),
            team_reference: Some(self.team_reference.trim().into()),
            score_value: Some(score_value),
            judge_name,
            needs_retake: self.needs_retake,
            retake_reason: (!self.retake_reason.trim().is_empty())
                .then(|| self.retake_reason.trim().into()),
        })
    }
}

#[derive(Deserialize)]
pub(crate) struct RefreshForm {
    csrf: String,
    revision: i64,
}

#[derive(Serialize)]
pub(crate) struct ActionResponse {
    queued: bool,
    message: String,
}

fn response(code: StatusCode, message: &str) -> (StatusCode, Json<ActionResponse>) {
    (
        code,
        Json(ActionResponse {
            queued: code == StatusCode::OK,
            message: message.into(),
        }),
    )
}

async fn authorized_photo(cx: &Cx, csrf: &str) -> Result<(Uuid, Photo)> {
    no_cache(cx);
    let competition = competition_id(cx)?;
    let (session_id, session) = account(cx, competition).await?;
    let state: &AppState = app_context(cx);
    if !cookies(cx)
        .signed(&state.cookie_key)
        .get(CSRF_COOKIE)
        .is_some_and(|c| c.value() == csrf)
    {
        return Err(forbidden().into());
    }
    let id = path_param_segment(cx, "photo_id")
        .parse::<Uuid>()
        .map_err(|_| not_found())?;
    let photo = state
        .store
        .photo(&session.username, competition, id)
        .await?
        .ok_or_else(not_found)?;
    Ok((session_id, photo))
}

#[derive(Serialize)]
pub(crate) struct DeleteResponse {
    deleted: bool,
    message: &'static str,
}

#[route(POST "/competitions/{competition_id}/photos/{photo_id}/delete")]
pub(crate) async fn delete_photo(
    cx: &Cx,
    Form(form): Form<RefreshForm>,
) -> Result<impl IntoResponse> {
    let (_, photo) = authorized_photo(cx, &form.csrf).await?;
    let state: &AppState = app_context(cx);
    let deleted = !photo.busy()
        && photo.revision == form.revision
        && state.store.delete_photo(&photo).await?;
    Ok((
        if deleted {
            StatusCode::OK
        } else {
            StatusCode::CONFLICT
        },
        Json(DeleteResponse {
            deleted,
            message: if deleted {
                "Foto borrada. La puntuación de Wodbuster se conserva."
            } else {
                "La foto ha cambiado o está en proceso. Espera a que termine y actualiza la vista."
            },
        }),
    ))
}

#[route(POST "/competitions/{competition_id}/photos/{photo_id}/refresh")]
pub(crate) async fn refresh_photo(
    cx: &Cx,
    Form(form): Form<RefreshForm>,
) -> Result<impl IntoResponse> {
    let (session, photo) = authorized_photo(cx, &form.csrf).await?;
    if photo.busy() || photo.revision != form.revision {
        return Ok(response(
            StatusCode::CONFLICT,
            "La foto está en proceso o ha cambiado. Espera a que termine y recarga la página.",
        ));
    }
    let state: &AppState = app_context(cx);
    let queued = state
        .store
        .requeue_photo(&photo, session, None, &crate::ai::model_name())
        .await?;
    Ok(if queued {
        response(
            StatusCode::OK,
            "Foto encolada para repetir el análisis de IA y guardar en Wodbuster.",
        )
    } else {
        response(
            StatusCode::CONFLICT,
            "La foto ha cambiado o está en proceso. Recarga la página.",
        )
    })
}

#[route(POST "/competitions/{competition_id}/photos/{photo_id}/edit")]
pub(crate) async fn edit_photo(cx: &Cx, Form(form): Form<EditForm>) -> Result<impl IntoResponse> {
    let (session, photo) = authorized_photo(cx, &form.csrf).await?;
    if photo.busy() || photo.revision != form.revision {
        return Ok(response(
            StatusCode::CONFLICT,
            "La foto está en proceso o ha cambiado. Espera a que termine y recarga la página.",
        ));
    }
    let judge_name = photo
        .reading
        .as_ref()
        .and_then(|r| r["judge_name"].as_str())
        .map(str::to_owned);
    let reading = match form.scorecard(judge_name) {
        Ok(r) => r,
        Err(e) => return Ok(response(StatusCode::BAD_REQUEST, e)),
    };
    let state: &AppState = app_context(cx);
    let detail = state
        .store
        .competition_detail(&photo.username, photo.competition_id)
        .await?
        .ok_or_else(|| bad_request("Carga los detalles de la competición"))?;
    let extraction = evaluate_scorecard(&detail, reading);
    if !extraction.ready_for_form {
        return Ok(response(
            StatusCode::BAD_REQUEST,
            &extraction.validation_errors.join(" · "),
        ));
    }
    let queued = state
        .store
        .requeue_scorecard(&photo, session, &extraction)
        .await?;
    Ok(if queued {
        response(
            StatusCode::OK,
            "Cambios guardados y puntuación encolada para guardar en Wodbuster.",
        )
    } else {
        response(
            StatusCode::CONFLICT,
            "La foto ha cambiado o está en proceso. Recarga la página.",
        )
    })
}
