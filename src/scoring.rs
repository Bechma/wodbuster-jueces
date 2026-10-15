//! Formatos y valores de puntuación. No escribe ni envía resultados a Arena.
use crate::details::{CompetitionDetail, Score};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    TimeOrRepetitions,
    Weight,
    Repetitions,
    Time,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldRole {
    Completed,
    Time,
    Repetitions,
    Weight,
    WeightUnit,
    TieBreak,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Boolean,
    MinutesSeconds,
    Integer,
    Decimal,
    Choice,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlReference {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScoreField {
    pub role: FieldRole,
    pub data_type: FieldType,
    pub label: String,
    pub required: bool,
    /// El campo se utiliza solo si «Completado» tiene este valor.
    pub when_completed: Option<bool>,
    pub control: Option<ControlReference>,
    pub choices: Vec<Choice>,
    pub default_value: Option<String>,
    /// Restricciones originales del formulario: rango, paso, patrón, etc.
    pub constraints: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScoreFormat {
    pub version: u8,
    pub kind: ScoreKind,
    pub fields: Vec<ScoreField>,
    pub weight_unit: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MinutesSeconds {
    pub minutes: u32,
    pub seconds: u8,
}

impl MinutesSeconds {
    pub fn parse(text: &str) -> Option<Self> {
        let (minutes, seconds) = text.trim().split_once(':')?;
        if minutes.is_empty()
            || !minutes.chars().all(|c| c.is_ascii_digit())
            || seconds.len() != 2
            || !seconds.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let result = Self {
            minutes: minutes.parse().ok()?,
            seconds: seconds.parse().ok()?,
        };
        (result.seconds < 60).then_some(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScoreValue {
    Completed {
        time: MinutesSeconds,
        tie_break: Option<MinutesSeconds>,
    },
    NotCompleted {
        repetitions: u32,
        tie_break: Option<MinutesSeconds>,
    },
    Weight {
        kilograms: f64,
    },
    Repetitions {
        repetitions: u32,
    },
    Time {
        time: MinutesSeconds,
    },
}

fn role(field: &Value) -> FieldRole {
    match field["attributes"]["name"]
        .as_str()
        .unwrap_or_default()
        .rsplit('$')
        .next()
        .unwrap_or_default()
    {
        "IoComplete" => FieldRole::Completed,
        "IoTimeMMSS" => FieldRole::Time,
        "IoReps" => FieldRole::Repetitions,
        "IoWeight" => FieldRole::Weight,
        "IoWeightMesure" => FieldRole::WeightUnit,
        "IoDesempate" => FieldRole::TieBreak,
        _ => FieldRole::Unknown,
    }
}

impl ScoreFormat {
    pub fn from_fields(raw: &Value) -> Option<Self> {
        let raw = raw.as_array().filter(|v| !v.is_empty())?;
        let roles: Vec<_> = raw.iter().map(role).collect();
        let kind = if roles.contains(&FieldRole::Completed) {
            ScoreKind::TimeOrRepetitions
        } else if roles.contains(&FieldRole::Weight) {
            ScoreKind::Weight
        } else if roles.contains(&FieldRole::Time) {
            ScoreKind::Time
        } else if roles.contains(&FieldRole::Repetitions) {
            ScoreKind::Repetitions
        } else {
            ScoreKind::Unknown
        };
        let mut fields: Vec<_> = raw
            .iter()
            .map(|field| {
                let role = role(field);
                let data_type = match role {
                    FieldRole::Completed => FieldType::Boolean,
                    FieldRole::Time | FieldRole::TieBreak => FieldType::MinutesSeconds,
                    FieldRole::Repetitions => FieldType::Integer,
                    FieldRole::Weight => FieldType::Decimal,
                    FieldRole::WeightUnit => FieldType::Choice,
                    FieldRole::Unknown => FieldType::Text,
                };
                let choices: Vec<Choice> = field["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| Choice {
                        value: o["value"].as_str().unwrap_or_default().into(),
                        label: o["label"].as_str().unwrap_or_default().into(),
                    })
                    .collect();
                let default_value = (role == FieldRole::WeightUnit)
                    .then(|| {
                        choices
                            .iter()
                            .find(|o| o.label.eq_ignore_ascii_case("kg"))
                            .map(|o| o.value.clone())
                    })
                    .flatten();
                let required = role != FieldRole::TieBreak
                    && (field["attributes"].get("required").is_some()
                        || field["attributes"]["class"]
                            .as_str()
                            .unwrap_or_default()
                            .contains("sc_valRequired")
                        || field["validation"].as_array().is_some_and(|v| {
                            v.iter().any(|x| {
                                x.as_str().is_some_and(|s| {
                                    matches!(s.to_lowercase().as_str(), "required" | "obligatorio")
                                })
                            })
                        }));
                let control = field["attributes"]["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .map(|name| ControlReference {
                        name: name.into(),
                        id: field["attributes"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    });
                let mut constraints = field["attributes"].clone();
                if let Some(attrs) = constraints.as_object_mut() {
                    for attr in ["name", "id", "class"] {
                        attrs.remove(attr);
                    }
                }
                ScoreField {
                    role,
                    data_type,
                    label: field["label"].as_str().unwrap_or_default().into(),
                    required,
                    when_completed: if kind == ScoreKind::TimeOrRepetitions {
                        match role {
                            FieldRole::Time => Some(true),
                            FieldRole::Repetitions => Some(false),
                            _ => None,
                        }
                    } else {
                        None
                    },
                    control,
                    choices,
                    default_value,
                    constraints,
                }
            })
            .collect();
        // Los esquemas antiguos solo tenían el selector: conserva la semántica,
        // pero no inventa referencias a controles que aún no se han observado.
        if kind == ScoreKind::TimeOrRepetitions {
            for (role, data_type, label, completed) in [
                (
                    FieldRole::Time,
                    FieldType::MinutesSeconds,
                    "Tiempo (mm:ss)",
                    true,
                ),
                (
                    FieldRole::Repetitions,
                    FieldType::Integer,
                    "Repeticiones",
                    false,
                ),
            ] {
                if !fields.iter().any(|f| f.role == role) {
                    fields.push(ScoreField {
                        role,
                        data_type,
                        label: label.into(),
                        required: true,
                        when_completed: Some(completed),
                        control: None,
                        choices: vec![],
                        default_value: None,
                        constraints: Value::Null,
                    });
                }
            }
        }
        Some(Self {
            version: 1,
            kind,
            fields,
            weight_unit: (kind == ScoreKind::Weight).then(|| "kg".into()),
        })
    }

    pub fn has_all_controls(&self) -> bool {
        self.kind != ScoreKind::Unknown && self.fields.iter().all(|f| f.control.is_some())
    }

    pub fn summary(&self) -> &'static str {
        match self.kind {
            ScoreKind::TimeOrRepetitions => {
                "Completado Sí/No: Sí → tiempo (mm:ss); No → repeticiones."
            }
            ScoreKind::Weight => "Peso en kg.",
            ScoreKind::Repetitions => "Número de repeticiones.",
            ScoreKind::Time => "Tiempo (mm:ss).",
            ScoreKind::Unknown => "Formato de puntuación pendiente de identificar.",
        }
    }

    fn parse_value(&self, score: &Score) -> Option<ScoreValue> {
        // Un formulario vacío puede tener valores por defecto: null siempre gana.
        let result = score.result.as_deref()?.trim();
        let fields = score.metadata["fields"].as_array();
        let value = |key| {
            fields
                .and_then(|fields| fields.iter().find(|f| role(f) == key))
                .and_then(|f| f["value"].as_str())
                .unwrap_or_default()
                .trim()
        };
        let tie_break = MinutesSeconds::parse(value(FieldRole::TieBreak));
        match self.kind {
            ScoreKind::TimeOrRepetitions => {
                match value(FieldRole::Completed) {
                    "True" => {
                        return MinutesSeconds::parse(value(FieldRole::Time))
                            .or_else(|| MinutesSeconds::parse(result))
                            .map(|time| ScoreValue::Completed { time, tie_break });
                    }
                    "False" => {
                        return integer(value(FieldRole::Repetitions))
                            .or_else(|| count_result(result))
                            .map(|repetitions| ScoreValue::NotCompleted {
                                repetitions,
                                tie_break,
                            });
                    }
                    _ => {}
                }
                if let Some(time) = MinutesSeconds::parse(result) {
                    return Some(ScoreValue::Completed { time, tie_break });
                }
                count_result(result).map(|repetitions| ScoreValue::NotCompleted {
                    repetitions,
                    tie_break,
                })
            }
            ScoreKind::Weight => {
                let kg_choice = self
                    .fields
                    .iter()
                    .find(|f| f.role == FieldRole::WeightUnit)
                    .and_then(|f| f.default_value.as_deref());
                let kilograms = if kg_choice.is_some_and(|kg| kg == value(FieldRole::WeightUnit)) {
                    decimal(value(FieldRole::Weight))
                } else {
                    None
                }
                .or_else(|| decimal(result.strip_suffix("kg").unwrap_or(result).trim()))?;
                Some(ScoreValue::Weight { kilograms })
            }
            ScoreKind::Repetitions => integer(value(FieldRole::Repetitions))
                .or_else(|| count_result(result))
                .map(|repetitions| ScoreValue::Repetitions { repetitions }),
            ScoreKind::Time => MinutesSeconds::parse(value(FieldRole::Time))
                .or_else(|| MinutesSeconds::parse(result))
                .map(|time| ScoreValue::Time { time }),
            ScoreKind::Unknown => None,
        }
    }
}

fn integer(text: &str) -> Option<u32> {
    (!text.is_empty() && text.chars().all(|c| c.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}
fn count_result(text: &str) -> Option<u32> {
    integer(
        text.strip_suffix("reps")
            .or_else(|| text.strip_suffix("rep"))
            .unwrap_or(text)
            .trim(),
    )
}
fn decimal(text: &str) -> Option<f64> {
    let number: f64 = text.replace(',', ".").parse().ok()?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

pub fn prepare_detail(detail: &mut CompetitionDetail) {
    for category in &mut detail.categories {
        for wod in &mut category.wods {
            if let Some(format) = ScoreFormat::from_fields(&wod.metadata["score_fields"]) {
                wod.score_format = Some(format);
            }
        }
        for team in &mut category.teams {
            for score in &mut team.scores {
                score.value = category
                    .wods
                    .iter()
                    .find(|w| w.id == score.wod_id)
                    .and_then(|w| w.score_format.as_ref())
                    .and_then(|format| format.parse_value(score));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(name: &str, value: &str) -> Value {
        json!({"attributes":{"name":format!("panel${name}"),"id":format!("panel_{name}"),"class":"sc_valRequired"},
            "label":name,"value":value,"options":[],"validation":["Required"]})
    }
    fn timed_fields(completed: &str, time: &str, reps: &str) -> Value {
        json!([
            field("IoComplete", completed),
            field("IoTimeMMSS", time),
            field("IoReps", reps)
        ])
    }
    fn score(result: Option<&str>, fields: Value) -> Score {
        Score {
            wod_id: 1,
            result: result.map(str::to_owned),
            rank: None,
            points: None,
            metadata: json!({"fields":fields}),
            value: None,
        }
    }

    #[test]
    fn completion_requires_time_or_repetitions_and_preserves_bindings() {
        let fields = timed_fields("True", "07:20", "");
        let format = ScoreFormat::from_fields(&fields).unwrap();
        assert_eq!(format.kind, ScoreKind::TimeOrRepetitions);
        assert!(format.has_all_controls());
        let time = format
            .fields
            .iter()
            .find(|f| f.role == FieldRole::Time)
            .unwrap();
        assert_eq!(time.when_completed, Some(true));
        assert_eq!(time.data_type, FieldType::MinutesSeconds);
        assert_eq!(time.control.as_ref().unwrap().name, "panel$IoTimeMMSS");
        let reps = format
            .fields
            .iter()
            .find(|f| f.role == FieldRole::Repetitions)
            .unwrap();
        assert_eq!(reps.when_completed, Some(false));
        assert_eq!(
            format.parse_value(&score(Some("07:20"), fields)),
            Some(ScoreValue::Completed {
                time: MinutesSeconds {
                    minutes: 7,
                    seconds: 20
                },
                tie_break: None
            })
        );
        assert_eq!(
            format.parse_value(&score(Some("0reps"), timed_fields("False", "", "0"))),
            Some(ScoreValue::NotCompleted {
                repetitions: 0,
                tie_break: None
            })
        );
        assert!(MinutesSeconds::parse("7:60").is_none());
        assert!(matches!(
            format.parse_value(&score(Some("07:20"), json!([field("IoComplete", "True")]))),
            Some(ScoreValue::Completed { .. })
        ));
        assert!(MinutesSeconds::parse("07:2").is_none());
        assert!(MinutesSeconds::parse("-1:20").is_none());
    }

    #[test]
    fn null_is_unscored_even_with_filled_form_defaults() {
        let fields = timed_fields("False", "", "0");
        let format = ScoreFormat::from_fields(&fields).unwrap();
        let empty_score = score(None, fields);
        assert!(!empty_score.is_scored());
        assert_eq!(format.parse_value(&empty_score), None);
        let unrecognized = score(Some("DNS"), json!([]));
        assert!(unrecognized.is_scored());
        assert_eq!(format.parse_value(&unrecognized), None);
    }

    #[test]
    fn weights_use_captured_kg_option_and_decimal_amount() {
        let mut unit = field("IoWeightMesure", "10");
        unit["options"] = json!([{"value":"10","label":"Kg"},{"value":"20","label":"Lb"},{"value":"30","label":"Oz"}]);
        let fields = json!([field("IoWeight", "82,5"), unit]);
        let format = ScoreFormat::from_fields(&fields).unwrap();
        assert_eq!(format.kind, ScoreKind::Weight);
        assert_eq!(format.weight_unit.as_deref(), Some("kg"));
        assert_eq!(format.fields[1].default_value.as_deref(), Some("10"));
        assert_eq!(
            format.parse_value(&score(Some("82.5kg"), fields)),
            Some(ScoreValue::Weight { kilograms: 82.5 })
        );
        assert_eq!(
            format.parse_value(&score(None, json!([field("IoWeight", "0")]))),
            None
        );
        assert_eq!(format.parse_value(&score(Some("-1kg"), json!([]))), None);
        assert_eq!(format.parse_value(&score(Some("NaNkg"), json!([]))), None);
    }

    #[test]
    fn cached_schemas_declare_missing_branches_without_inventing_controls() {
        let format = ScoreFormat::from_fields(&json!([field("IoComplete", "")])).unwrap();
        assert_eq!(format.kind, ScoreKind::TimeOrRepetitions);
        assert!(!format.has_all_controls());
        assert_eq!(format.fields[1].control, None);
        assert_eq!(format.fields[2].control, None);
        let mut reps = field("IoReps", "12");
        reps["label"] = json!("Repetitions");
        assert_eq!(
            ScoreFormat::from_fields(&json!([reps])).unwrap().kind,
            ScoreKind::Repetitions
        );
    }
}
