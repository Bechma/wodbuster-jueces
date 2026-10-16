use serde_json::{Value, json};
use wodbuster_jueces::{
    details::CompetitionDetail,
    scorecard::{FormStep, ScorecardReading, build_context, evaluate_reading},
    scoring::{FieldRole, MinutesSeconds, ScoreFormat, ScoreValue},
};
mod support;

fn field(name: &str) -> Value {
    json!({"attributes":{"name":format!("panel${name}"),"id":format!("panel_{name}"),"class":"sc_valRequired"},"label":name,"options":[],"validation":["Required"]})
}
fn detail() -> CompetitionDetail {
    let mut detail = support::fixture();
    let mut complete = field("IoComplete");
    complete["options"] = json!([{"value":"True","label":"Yes"},{"value":"False","label":"No"}]);
    let fields = json!([complete.clone(), field("IoTimeMMSS"), field("IoReps")]);
    detail.categories[0].wods[0].score_format = ScoreFormat::from_fields(&fields);
    let score = &mut detail.categories[0].teams[0].scores[0];
    complete["value"] = json!("");
    score.result = None;
    score.metadata =
        json!({"fields":[complete],"save_controls":[{"name":"panel$CtlSave","id":"save"}]});
    detail
}
fn reading(value: Option<ScoreValue>) -> ScorecardReading {
    ScorecardReading {
        category_id: Some(16907),
        wod_id: Some(64459),
        team_reference: Some("298682".into()),
        score_value: value,
        judge_name: None,
        needs_retake: false,
        retake_reason: None,
    }
}

#[test]
fn minimal_reading_has_only_the_data_needed_to_prepare_the_form() {
    let input = json!({
        "category_id":16907,"wod_id":64459,"team_reference":"298682",
        "score_value":{"kind":"not_completed","repetitions":2,"tie_break":null}
    });
    let input: ScorecardReading = serde_json::from_value(input).unwrap();
    assert!(input.judge_name.is_none());
    let result = evaluate_reading(&detail(), input);
    assert!(result.ready_for_form && result.prepared.is_some());
    assert!(result.validation_errors.is_empty());
    let output = serde_json::to_value(&result.reading).unwrap();
    let schema = serde_json::to_value(schemars::schema_for!(ScorecardReading)).unwrap();
    for removed in ["is_confident", "score_raw", "score_notes"] {
        assert!(output.get(removed).is_none());
        assert!(schema["properties"].get(removed).is_none());
    }
    assert!(
        schema["required"]
            .as_array()
            .is_none_or(|fields| { !fields.iter().any(|field| field == "needs_retake") })
    );
}

#[test]
fn detected_judge_is_preserved_without_changing_score_validation_or_submission() {
    let original = reading(Some(ScoreValue::NotCompleted {
        repetitions: 2,
        tie_break: None,
    }));
    let expected = evaluate_reading(&detail(), original.clone());
    for judge_name in [None, Some("Ana García · Juez 12".to_owned())] {
        let mut with_judge = original.clone();
        with_judge.judge_name = judge_name.clone();
        let serialized = serde_json::to_value(&with_judge).unwrap();
        let restored: ScorecardReading = serde_json::from_value(serialized).unwrap();
        let actual = evaluate_reading(&detail(), restored);
        assert!(actual.ready_for_form);
        assert_eq!(actual.reading.judge_name, judge_name);
        assert_eq!(actual.prepared, expected.prepared);
        assert_eq!(actual.validation_errors, expected.validation_errors);
        assert!(!actual.reading.needs_retake);
        let plan = serde_json::to_value(actual.prepared).unwrap();
        assert!(plan.get("judge_name").is_none());
        assert!(!plan.to_string().contains("Ana García"));
    }
}

#[test]
fn resolves_non_completed_image_into_ordered_form_steps() {
    let result = evaluate_reading(
        &detail(),
        reading(Some(ScoreValue::NotCompleted {
            repetitions: 2,
            tie_break: None,
        })),
    );
    assert!(result.ready_for_form);
    let prepared = result.prepared.unwrap();
    assert_eq!(prepared.competition_id, 3910);
    assert_eq!(prepared.registration_id, Some(298682));
    let url = url::Url::parse(&prepared.form.url).unwrap();
    assert_eq!(url.host_str(), Some("arena.wodbuster.com"));
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "id").unwrap().1,
        "16907"
    );
    assert_eq!(
        url.query_pairs().find(|(k, _)| k == "gid").unwrap().1,
        "captain-reference"
    );
    assert!(
        matches!(&prepared.form.steps[0],FormStep::Select {role:FieldRole::Completed,value,wait_for_postback:true,..} if value=="False")
    );
    assert!(
        matches!(&prepared.form.steps[1],FormStep::Fill {role:FieldRole::Repetitions,control,value} if value=="2" && control.name=="panel$IoReps")
    );
    assert_eq!(prepared.form.save_control.id, "save");
}

#[test]
fn validates_time_weight_and_category_specific_formats() {
    let time = ScoreValue::Completed {
        time: MinutesSeconds {
            minutes: 7,
            seconds: 20,
        },
        tie_break: None,
    };
    let result = evaluate_reading(&detail(), reading(Some(time)));
    assert!(
        matches!(&result.prepared.unwrap().form.steps[1],FormStep::Fill {role:FieldRole::Time,value,..} if value=="07:20")
    );
    let invalid = ScoreValue::Completed {
        time: MinutesSeconds {
            minutes: 7,
            seconds: 60,
        },
        tie_break: None,
    };
    assert!(!evaluate_reading(&detail(), reading(Some(invalid))).ready_for_form);
    let mut weighted = detail();
    let mut unit = field("IoWeightMesure");
    unit["options"] = json!([{"value":"10","label":"Kg"},{"value":"20","label":"Lb"},{"value":"30","label":"Oz"}]);
    weighted.categories[0].wods[0].score_format =
        ScoreFormat::from_fields(&json!([field("IoWeight"), unit]));
    let result = evaluate_reading(
        &weighted,
        reading(Some(ScoreValue::Weight { kilograms: 82.5 })),
    );
    assert!(result.ready_for_form);
    let plan = result.prepared.unwrap().form;
    assert!(
        matches!(&plan.steps[0],FormStep::Select {role:FieldRole::WeightUnit,value,..} if value=="10")
    );
    assert!(
        matches!(&plan.steps[1],FormStep::Fill {role:FieldRole::Weight,value,..} if value=="82.5")
    );
    assert!(
        !evaluate_reading(
            &weighted,
            reading(Some(ScoreValue::Weight { kilograms: -1.0 }))
        )
        .ready_for_form
    );
    assert!(
        !evaluate_reading(
            &weighted,
            reading(Some(ScoreValue::NotCompleted {
                repetitions: 2,
                tie_break: None
            }))
        )
        .ready_for_form
    );
    // Mismo nombre de WOD y equipo, otra categoría con formato de repeticiones.
    let mut scaled = weighted.categories[0].clone();
    scaled.id = 16912;
    scaled.wods[0].id = 64463;
    scaled.wods[0].score_format = ScoreFormat::from_fields(&json!([field("IoReps")]));
    scaled.teams[0].scores[0].wod_id = 64463;
    weighted.categories.push(scaled);
    let mut input = reading(Some(ScoreValue::Repetitions { repetitions: 2 }));
    input.category_id = Some(16912);
    input.wod_id = Some(64463);
    assert!(evaluate_reading(&weighted, input.clone()).ready_for_form);
    input.wod_id = Some(64459);
    assert!(!evaluate_reading(&weighted, input).ready_for_form);
}

#[test]
fn accepts_complete_readings_regardless_of_retake_flags_and_rejects_missing_or_foreign_data() {
    let value = ScoreValue::NotCompleted {
        repetitions: 0,
        tie_break: None,
    };
    assert!(evaluate_reading(&detail(), reading(Some(value.clone()))).ready_for_form);
    assert!(!evaluate_reading(&detail(), reading(None)).ready_for_form);
    let mut uncertain = reading(Some(value.clone()));
    uncertain.needs_retake = true;
    uncertain.retake_reason = Some("No sé si el nombre debe coincidir literalmente.".into());
    assert!(evaluate_reading(&detail(), uncertain).ready_for_form);
    for missing in ["category_id", "wod_id", "team_reference", "score_value"] {
        let mut input = serde_json::to_value(reading(Some(value.clone()))).unwrap();
        input.as_object_mut().unwrap().remove(missing);
        let input = serde_json::from_value(input).unwrap();
        let result = evaluate_reading(&detail(), input);
        assert!(
            !result.ready_for_form,
            "No debe preparar el formulario sin {missing}"
        );
        assert!(result.prepared.is_none() && !result.validation_errors.is_empty());
    }
    let mut foreign = reading(Some(value.clone()));
    foreign.team_reference = Some("unknown".into());
    assert!(!evaluate_reading(&detail(), foreign).ready_for_form);
    let mut uncaptured = detail();
    uncaptured.categories[0].teams[0].gid = None;
    assert!(!evaluate_reading(&uncaptured, reading(Some(value.clone()))).ready_for_form);
    let mut uncaptured = detail();
    uncaptured.categories[0].teams[0].scores[0].metadata["save_controls"] = json!([]);
    assert!(!evaluate_reading(&uncaptured, reading(Some(value.clone()))).ready_for_form);
    let mut wrong_scope = detail();
    wrong_scope.categories[0].wods[0]
        .score_format
        .as_mut()
        .unwrap()
        .fields
        .iter_mut()
        .find(|f| f.role == FieldRole::Repetitions)
        .unwrap()
        .control
        .as_mut()
        .unwrap()
        .name = "other_wod$IoReps".into();
    assert!(!evaluate_reading(&wrong_scope, reading(Some(value))).ready_for_form);
}

#[test]
fn clears_optional_tie_break_and_rejects_unsupported_required_fields() {
    let mut detail = detail();
    let mut tie = ScoreFormat::from_fields(&json!([field("IoDesempate")]))
        .unwrap()
        .fields
        .remove(0);
    tie.required = false;
    detail.categories[0].wods[0]
        .score_format
        .as_mut()
        .unwrap()
        .fields
        .push(tie);
    let result = evaluate_reading(
        &detail,
        reading(Some(ScoreValue::NotCompleted {
            repetitions: 2,
            tie_break: None,
        })),
    );
    assert!(
        matches!(result.prepared.unwrap().form.steps.last().unwrap(),FormStep::Fill {role:FieldRole::TieBreak,value,..} if value.is_empty())
    );
    let unknown = ScoreFormat::from_fields(&json!([field("IoUnknown")]))
        .unwrap()
        .fields
        .remove(0);
    detail.categories[0].wods[0]
        .score_format
        .as_mut()
        .unwrap()
        .fields
        .push(unknown);
    assert!(
        !evaluate_reading(
            &detail,
            reading(Some(ScoreValue::NotCompleted {
                repetitions: 2,
                tie_break: None
            }))
        )
        .ready_for_form
    );
}

#[test]
fn model_context_has_formats_but_no_remote_controls_or_private_metadata() {
    let mut detail = detail();
    detail.metadata["private"] = json!("cookie-sentinel");
    let context = build_context(&detail);
    assert!(context.contains("time_or_repetitions") && context.contains("when_completed"));
    for value in [
        "cookie-sentinel",
        "member-reference",
        "panel$IoComplete",
        "save_controls",
        "arena.wodbuster.com",
        "52reps",
    ] {
        assert!(!context.contains(value));
    }
    let schema = serde_json::to_value(schemars::schema_for!(ScorecardReading)).unwrap();
    assert_eq!(
        schema.pointer("/$defs/MinutesSeconds/properties/seconds/maximum"),
        Some(&json!(59))
    );
    assert!(
        serde_json::from_value::<ScoreValue>(
            json!({"kind":"not_completed","repetitions":2,"time":{"minutes":2,"seconds":0}})
        )
        .is_err()
    );
    let mut repeated = detail.clone();
    let mut another = repeated.categories[0].wods[0].clone();
    another.id += 1;
    repeated.categories[0].wods.push(another);
    let compact: Value = serde_json::from_str(&build_context(&repeated)).unwrap();
    assert_eq!(compact["formats"].as_array().unwrap().len(), 1);
    assert_eq!(
        compact["categories"][0]["wods"][0][2],
        compact["categories"][0]["wods"][1][2]
    );
}
