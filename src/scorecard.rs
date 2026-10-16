//! Contrato del PoC: lectura visual tipada → plan de rellenado. No guarda ni envía puntuaciones.
use crate::{
    details::CompetitionDetail,
    scoring::{ControlReference, FieldRole, MinutesSeconds, ScoreFormat, ScoreKind, ScoreValue},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScorecardReading {
    /// ID de categoría del contexto. No deducirla de un nombre de equipo ambiguo.
    pub category_id: Option<i64>,
    /// ID del WOD dentro de esa categoría, nunca de otra categoría con el mismo nombre.
    pub wod_id: Option<i64>,
    /// Referencia del equipo dentro de esa categoría, copiada del contexto.
    pub team_reference: Option<String>,
    /// Resultado tipado según score_format. Null si falta o no se puede leer con certeza.
    pub score_value: Option<ScoreValue>,
    /// Nombre o identificador del juez escrito en la imagen. Solo informativo; null si no es legible.
    #[serde(default)]
    pub judge_name: Option<String>,
    /// Señal informativa para datos que no se han podido leer. No bloquea una lectura completa y válida.
    #[serde(default)]
    pub needs_retake: bool,
    /// Motivo breve de los datos ausentes o ilegibles. Null si no hay observaciones.
    pub retake_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum FormStep {
    Select {
        role: FieldRole,
        control: ControlReference,
        value: String,
        wait_for_postback: bool,
    },
    Fill {
        role: FieldRole,
        control: ControlReference,
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoreFormPlan {
    pub url: String,
    /// Ejecutar en este orden; después del postback hay que volver a localizar los controles.
    pub steps: Vec<FormStep>,
    /// Control que el ejecutor pulsa únicamente al solicitar el envío.
    pub save_control: ControlReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreparedScore {
    pub version: u8,
    pub competition_id: i64,
    pub category_id: i64,
    pub category_name: String,
    pub wod_id: i64,
    pub wod_name: String,
    pub team_reference: String,
    pub team_name: String,
    pub registration_id: Option<i64>,
    pub gid: String,
    pub value: ScoreValue,
    pub form: ScoreFormPlan,
}

#[derive(Debug, Serialize)]
pub struct ExtractionResult {
    pub reading: ScorecardReading,
    pub ready_for_form: bool,
    pub prepared: Option<PreparedScore>,
    pub validation_errors: Vec<String>,
}

/// Solo nombres, referencias y semántica de los formatos; no envía cookies, URLs,
/// miembros, puntuaciones previas ni identificadores de controles al proveedor de IA.
pub fn build_context(detail: &CompetitionDetail) -> String {
    let mut formats = Vec::<serde_json::Value>::new();
    let categories: Vec<_> = detail.categories.iter().map(|cat| {
        let wods: Vec<_> = cat.wods.iter().map(|wod| {
            let format_index = wod.score_format.as_ref().map(|f| {
                let compact = json!({"kind":f.kind,"weight_unit":f.weight_unit,
                    "fields":f.fields.iter().map(|field|json!([field.role,field.data_type,field.required,field.when_completed])).collect::<Vec<_>>()});
                if let Some(index) = formats.iter().position(|existing|existing==&compact) { index }
                else { formats.push(compact); formats.len()-1 }
            });
            json!([wod.id,wod.name,format_index])
        }).collect();
        json!({"id":cat.id,"name":cat.name,"wods":wods,
            "teams":cat.teams.iter().map(|team|json!([team.reference,team.name,team.bib])).collect::<Vec<_>>()})
    }).collect();
    json!({"competition":{"id":detail.id,"name":detail.name},
        "columns":{"wods":["id","name","format_index"],"teams":["reference","name","bib"],"format_fields":["role","type","required","when_completed"]},
        "format_index":"Indice de formats, empezando por 0. Null significa formato no disponible.",
        "formats":formats,"categories":categories}).to_string()
}

pub fn evaluate_reading(detail: &CompetitionDetail, reading: ScorecardReading) -> ExtractionResult {
    let (prepared, validation_errors) = match prepare_score(detail, &reading) {
        Ok(prepared) => (Some(prepared), vec![]),
        Err(error) => (None, vec![error]),
    };
    ExtractionResult {
        ready_for_form: prepared.is_some(),
        prepared,
        reading,
        validation_errors,
    }
}

pub fn prepare_score(
    detail: &CompetitionDetail,
    reading: &ScorecardReading,
) -> Result<PreparedScore, String> {
    if detail.id <= 0 {
        return Err("El identificador de competición no es válido.".into());
    }
    let category = detail
        .categories
        .iter()
        .find(|c| Some(c.id) == reading.category_id)
        .ok_or("La categoría no pertenece al contexto.")?;
    let wod = category
        .wods
        .iter()
        .find(|w| Some(w.id) == reading.wod_id)
        .ok_or("El WOD no pertenece a la categoría seleccionada.")?;
    if category.id <= 0 || wod.id <= 0 {
        return Err("Los identificadores de categoría y WOD no son válidos.".into());
    }
    let team = category
        .teams
        .iter()
        .find(|t| Some(t.reference.as_str()) == reading.team_reference.as_deref())
        .ok_or("El equipo no pertenece a la categoría seleccionada.")?;
    let score = team
        .scores
        .iter()
        .find(|s| s.wod_id == wod.id)
        .ok_or("No se ha capturado el formulario de este equipo y WOD.")?;
    let format = wod
        .score_format
        .as_ref()
        .ok_or("Falta el formato de puntuación del WOD; carga su información.")?;
    let value = reading
        .score_value
        .as_ref()
        .ok_or("No hay una puntuación legible.")?;
    validate_value(format, value)?;
    let gid = team
        .gid
        .as_ref()
        .filter(|s| !s.is_empty())
        .ok_or("Falta el gid del equipo para acceder al formulario de juez.")?;
    let save = score.metadata["save_controls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(ControlReference {
                name: c["name"].as_str()?.into(),
                id: c["id"].as_str()?.into(),
            })
        })
        .find(|c| c.name.ends_with("$CtlSave"))
        .ok_or("Falta la referencia al control de guardar del WOD.")?;
    let scope = save
        .name
        .rsplit_once('$')
        .ok_or("Referencia de formulario inválida.")?
        .0;
    let control = |role| -> Result<ControlReference, String> {
        let current = score.metadata["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| crate::scoring::role(f) == role)
            .and_then(|f| {
                Some(ControlReference {
                    name: f["attributes"]["name"].as_str()?.into(),
                    id: f["attributes"]["id"].as_str()?.into(),
                })
            });
        let reference = current
            .or_else(|| {
                format
                    .fields
                    .iter()
                    .find(|f| f.role == role)
                    .and_then(|f| f.control.clone())
            })
            .ok_or_else(|| {
                format!("Falta el control de {role:?}; refresca los detalles de la competición.")
            })?;
        if !reference.name.starts_with(&format!("{scope}$")) {
            return Err("Un control pertenece al formulario de otro WOD.".into());
        }
        Ok(reference)
    };
    let mut steps = Vec::new();
    let completed = match value {
        ScoreValue::Completed { .. } => Some(true),
        ScoreValue::NotCompleted { .. } => Some(false),
        _ => None,
    };
    if let Some(completed) = completed {
        let field = format
            .fields
            .iter()
            .find(|f| f.role == FieldRole::Completed)
            .ok_or("Falta el selector Completado.")?;
        let choice = field
            .choices
            .iter()
            .find(|c| {
                c.value
                    .eq_ignore_ascii_case(if completed { "true" } else { "false" })
            })
            .ok_or("No se ha capturado la opción Sí/No del formulario.")?;
        steps.push(FormStep::Select {
            role: FieldRole::Completed,
            control: control(FieldRole::Completed)?,
            value: choice.value.clone(),
            wait_for_postback: true,
        });
    }
    if matches!(value, ScoreValue::Weight { .. })
        && let Some(field) = format
            .fields
            .iter()
            .find(|f| f.role == FieldRole::WeightUnit)
    {
        let kg = field
            .choices
            .iter()
            .find(|c| c.label.eq_ignore_ascii_case("kg"))
            .ok_or("No se ha capturado la opción Kg del formulario.")?;
        steps.push(FormStep::Select {
            role: FieldRole::WeightUnit,
            control: control(FieldRole::WeightUnit)?,
            value: kg.value.clone(),
            wait_for_postback: false,
        });
    }
    let (role, text) = match value {
        ScoreValue::Completed { time, .. } | ScoreValue::Time { time } => {
            (FieldRole::Time, format_time(time))
        }
        ScoreValue::NotCompleted { repetitions, .. } | ScoreValue::Repetitions { repetitions } => {
            (FieldRole::Repetitions, repetitions.to_string())
        }
        ScoreValue::Weight { kilograms } => (FieldRole::Weight, kilograms.to_string()),
    };
    steps.push(FormStep::Fill {
        role,
        control: control(role)?,
        value: text,
    });
    let tie_break = match value {
        ScoreValue::Completed { tie_break, .. } | ScoreValue::NotCompleted { tie_break, .. } => {
            *tie_break
        }
        _ => None,
    };
    if format.fields.iter().any(|f| f.role == FieldRole::TieBreak) {
        steps.push(FormStep::Fill {
            role: FieldRole::TieBreak,
            control: control(FieldRole::TieBreak)?,
            value: tie_break.as_ref().map(format_time).unwrap_or_default(),
        });
    } else if tie_break.is_some() {
        return Err("Este WOD no admite desempate.".into());
    }
    for field in &format.fields {
        if field.required && field.when_completed.is_none_or(|c| Some(c) == completed) {
            let filled = steps.iter().any(|s| match s {
                FormStep::Select { role, value, .. } | FormStep::Fill { role, value, .. } => {
                    *role == field.role && !value.is_empty()
                }
            });
            if !filled {
                return Err(format!(
                    "Falta un campo obligatorio del formato: {:?}.",
                    field.role
                ));
            }
        }
    }
    let mut url = url::Url::parse(crate::browser::ARENA_URL)
        .map_err(|_| "URL de Arena inválida.")?
        .join("mywodbusterarena/competitionjudgedivision.aspx")
        .map_err(|_| "URL de Arena inválida.")?;
    url.query_pairs_mut()
        .append_pair("id", &category.id.to_string())
        .append_pair("gid", gid);
    Ok(PreparedScore {
        version: 1,
        competition_id: detail.id,
        category_id: category.id,
        category_name: category.name.clone(),
        wod_id: wod.id,
        wod_name: wod.name.clone(),
        team_reference: team.reference.clone(),
        team_name: team.name.clone(),
        registration_id: team.registration_id,
        gid: gid.clone(),
        value: value.clone(),
        form: ScoreFormPlan {
            url: url.to_string(),
            steps,
            save_control: save,
        },
    })
}

fn format_time(time: &MinutesSeconds) -> String {
    format!("{:02}:{:02}", time.minutes, time.seconds)
}

fn validate_value(format: &ScoreFormat, value: &ScoreValue) -> Result<(), String> {
    if format.version != 1 {
        return Err("Versión del formato de puntuación no compatible.".into());
    }
    let compatible = matches!(
        (format.kind, value),
        (
            ScoreKind::TimeOrRepetitions,
            ScoreValue::Completed { .. } | ScoreValue::NotCompleted { .. }
        ) | (ScoreKind::Weight, ScoreValue::Weight { .. })
            | (ScoreKind::Repetitions, ScoreValue::Repetitions { .. })
            | (ScoreKind::Time, ScoreValue::Time { .. })
    );
    if !compatible {
        return Err("La puntuación no corresponde al formato de este WOD y categoría.".into());
    }
    let (time, tie) = match value {
        ScoreValue::Completed { time, tie_break } => (Some(time), tie_break.as_ref()),
        ScoreValue::NotCompleted { tie_break, .. } => (None, tie_break.as_ref()),
        ScoreValue::Time { time } => (Some(time), None),
        _ => (None, None),
    };
    if time.into_iter().chain(tie).any(|t| t.seconds >= 60) {
        return Err("Los segundos de tiempo o desempate deben estar entre 00 y 59.".into());
    }
    if let ScoreValue::Weight { kilograms } = value
        && (!kilograms.is_finite() || *kilograms < 0.0)
    {
        return Err("El peso debe ser un número finito no negativo en kg.".into());
    }
    Ok(())
}
