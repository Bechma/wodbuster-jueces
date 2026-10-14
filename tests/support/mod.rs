use serde_json::json;
use wodbuster_jueces::details::{Category, CompetitionDetail, Score, Team, Wod};

pub fn fixture() -> CompetitionDetail {
    CompetitionDetail {
        id: 3910,
        name: "Fixture competition".into(),
        loaded_at: None,
        metadata: json!({"judge_url":"https://arena.wodbuster.com/mywodbusterarena/competitionjudge.aspx?id=3910"}),
        categories: vec![Category {
            id: 16907,
            name: "RX".into(),
            metadata: json!({"type":"Team (2)"}),
            wods: vec![Wod {
                id: 64459,
                name: "Giralda".into(),
                description: "AMRAP 10 minutos".into(),
                scoring: "Reps".into(),
                metadata: json!({"time_cap":600}),
            }],
            teams: vec![Team {
                reference: "298682".into(),
                registration_id: Some(298682),
                gid: Some("captain-reference".into()),
                name: "Team & Co".into(),
                affiliate: "Box".into(),
                bib: "7".into(),
                rank: "1".into(),
                points: "100pts".into(),
                members: json!([{"Id":"member-reference","Nombre":"Member"}]),
                metadata: json!({"judge_url":"score-reference"}),
                scores: vec![Score {
                    wod_id: 64459,
                    result: Some("52reps".into()),
                    rank: Some("1".into()),
                    points: Some("100pts".into()),
                    metadata: json!({"fields":[{"attributes":{"id":"tie","name":"score$tie"},"value":"03:21"}]}),
                }],
            }],
        }],
    }
}
