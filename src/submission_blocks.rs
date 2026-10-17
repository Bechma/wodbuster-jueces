//! Bloqueos locales por cuenta y competición, independientes de la lectura de IA.
use crate::{details::CompetitionDetail, scorecard::ScorecardReading};

#[derive(Debug, Default, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SubmissionBlock {
    /// Cero significa todas las categorías.
    pub category_id: i64,
    /// Solo para conservar bloqueos antiguos cuyo nombre no se pudo recuperar.
    pub wod_id: i64,
    /// Nombre normalizado del grupo; vacío significa todos los WODs si wod_id es cero.
    pub wod_name: String,
}

pub fn normalize_wod_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl SubmissionBlock {
    pub fn matches(&self, detail: &CompetitionDetail, reading: &ScorecardReading) -> bool {
        (self.category_id > 0 || self.wod_id > 0 || !self.wod_name.is_empty())
            && (self.category_id == 0 || reading.category_id == Some(self.category_id))
            && (self.wod_id == 0 || reading.wod_id == Some(self.wod_id))
            && (self.wod_name.is_empty()
                || detail
                    .categories
                    .iter()
                    .filter(|c| reading.category_id.is_none_or(|id| id == c.id))
                    .flat_map(|c| &c.wods)
                    .any(|w| {
                        Some(w.id) == reading.wod_id && normalize_wod_name(&w.name) == self.wod_name
                    }))
    }
}

/// La marca de revisión de la IA nunca puede saltarse un bloqueo configurado.
pub fn enforce(
    detail: &CompetitionDetail,
    reading: &mut ScorecardReading,
    blocks: &[SubmissionBlock],
) -> Option<String> {
    let block = blocks.iter().find(|block| block.matches(detail, reading))?;
    let scope = if !block.wod_name.is_empty() {
        if block.category_id == 0 {
            format!("el WOD «{}» en todas las categorías", block.wod_name)
        } else {
            format!(
                "la categoría {} y el WOD «{}»",
                block.category_id, block.wod_name
            )
        }
    } else {
        match (block.category_id, block.wod_id) {
            (0, wod) => format!("el WOD {wod} en todas las categorías"),
            (category, 0) => format!("la categoría {category} en todos sus WODs"),
            (category, wod) => format!("la categoría {category} y el WOD {wod}"),
        }
    };
    let reason =
        format!("Subida bloqueada para {scope}. Desbloquea este ámbito antes de reintentar.");
    reading.needs_retake = true;
    if reading
        .retake_reason
        .as_ref()
        .is_none_or(|r| r.trim().is_empty())
    {
        reading.retake_reason = Some(reason.clone());
    }
    Some(reason)
}

/// Una regla que afecte a cualquier resultado bloquea la scorecard completa.
pub fn enforce_scorecard(
    detail: &CompetitionDetail,
    card: &mut crate::scorecard::Scorecard,
    blocks: &[SubmissionBlock],
) -> Option<String> {
    for mut reading in card.readings() {
        if let Some(reason) = enforce(detail, &mut reading, blocks) {
            card.needs_retake = reading.needs_retake;
            card.retake_reason = reading.retake_reason;
            return Some(reason);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    mod support {
        use crate as wodbuster_jueces;
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
    }

    #[test]
    fn named_wod_blocks_cover_distinct_ids_and_respect_category_and_name() {
        let mut detail = support::fixture();
        let mut other = detail.categories[0].clone();
        other.id = 16908;
        other.wods[0].id = 70000;
        other.wods[0].name = "  GIRALDA  ".into();
        let mut unrelated = other.wods[0].clone();
        unrelated.id = 70001;
        unrelated.name = "Maestranza".into();
        other.wods.push(unrelated);
        detail.categories.push(other);
        let global = SubmissionBlock {
            wod_name: "giralda".into(),
            ..Default::default()
        };
        for (category, wod, expected) in [
            (16907, 64459, true),
            (16908, 70000, true),
            (16908, 70001, false),
            (16907, 70000, false),
        ] {
            let mut reading = reading();
            reading.category_id = Some(category);
            reading.wod_id = Some(wod);
            assert_eq!(global.matches(&detail, &reading), expected);
            let scoped = SubmissionBlock {
                category_id: 16907,
                ..global.clone()
            };
            assert_eq!(
                scoped.matches(&detail, &reading),
                expected && category == 16907
            );
        }
        assert_eq!(normalize_wod_name("  Puerta   del\tSol "), "puerta del sol");
    }

    fn detail() -> CompetitionDetail {
        serde_json::from_str(
            r#"{"id":1,"name":"Fixture","categories":[],"metadata":{},"loaded_at":null}"#,
        )
        .unwrap()
    }

    fn reading() -> ScorecardReading {
        serde_json::from_value(serde_json::json!({
            "category_id":12,"wod_id":34,"team_reference":null,"score_value":null,
            "needs_retake":false,"retake_reason":null
        }))
        .unwrap()
    }

    #[test]
    fn category_wod_and_combination_match_only_their_scope() {
        for (category_id, wod_id, matches) in [
            (12, 0, true),
            (0, 34, true),
            (12, 34, true),
            (13, 0, false),
            (0, 35, false),
            (12, 35, false),
            (13, 34, false),
            (0, 0, false),
        ] {
            let block = SubmissionBlock {
                category_id,
                wod_id,
                wod_name: String::new(),
            };
            assert_eq!(block.matches(&detail(), &reading()), matches, "{block:?}");
        }
        let mut missing = reading();
        missing.category_id = None;
        assert!(
            SubmissionBlock {
                category_id: 0,
                wod_id: 34,
                wod_name: String::new()
            }
            .matches(&detail(), &missing)
        );
        assert!(
            !SubmissionBlock {
                category_id: 12,
                wod_id: 34,
                wod_name: String::new()
            }
            .matches(&detail(), &missing)
        );
        missing.category_id = Some(12);
        missing.wod_id = None;
        assert!(
            SubmissionBlock {
                category_id: 12,
                wod_id: 0,
                wod_name: String::new()
            }
            .matches(&detail(), &missing)
        );
    }

    #[test]
    fn fills_missing_retake_fields_preserves_existing_reason_and_always_blocks() {
        let blocks = [SubmissionBlock {
            category_id: 12,
            wod_id: 34,
            wod_name: String::new(),
        }];
        for needs_retake in [false, true] {
            for reason in [None, Some(""), Some("  "), Some("Foto borrosa")] {
                let mut reading = reading();
                reading.needs_retake = needs_retake;
                reading.retake_reason = reason.map(str::to_owned);
                let error = enforce(&detail(), &mut reading, &blocks).unwrap();
                assert!(reading.needs_retake);
                assert_eq!(
                    reading.retake_reason.as_deref(),
                    Some(if reason == Some("Foto borrosa") {
                        "Foto borrosa"
                    } else {
                        &error
                    })
                );
            }
        }
        let mut reading = reading();
        assert!(enforce(&detail(), &mut reading, &[]).is_none());
        assert!(!reading.needs_retake);
        assert!(reading.retake_reason.is_none());
    }
}
