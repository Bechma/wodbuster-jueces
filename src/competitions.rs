use std::collections::BTreeMap;

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::browser::ARENA_URL;

pub const COMPETITIONS_URL: &str = "https://arena.wodbuster.com/mywodbusterarena/competitions.aspx";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, sqlx::FromRow)]
pub struct Competition {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub judge_url: Option<String>,
    #[serde(default)]
    pub admin_url: Option<String>,
}

impl Competition {
    pub fn new(id: i64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            judge_url: None,
            admin_url: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompetitionError {
    #[error("La sesión de WodBuster ha caducado")]
    SessionExpired,
    #[error("WodBuster solicita una verificación adicional")]
    Verification,
    #[error("La carga de competiciones ha tardado demasiado")]
    Timeout,
    #[error("No se pudo conectar con el navegador o con WodBuster")]
    Unavailable,
    #[error("Hay otras operaciones en curso")]
    Busy,
    #[error("La página de competiciones no tiene el formato esperado")]
    InvalidPage,
    #[error("Ya no tienes acceso como juez o administrador a esta competición")]
    AccessDenied,
}

pub fn parse_competitions(html: &str) -> Result<Vec<Competition>, CompetitionError> {
    let document = Html::parse_document(html);
    let container = Selector::parse(".competitions").unwrap();
    if document.select(&container).next().is_none() {
        return Err(CompetitionError::InvalidPage);
    }
    let cards = Selector::parse(".competitions .competition").unwrap();
    let names = Selector::parse(".nombre").unwrap();
    let actions = Selector::parse(".primaryactions a[href]").unwrap();
    let base = Url::parse(ARENA_URL).unwrap();
    let mut competitions = BTreeMap::new();
    for card in document.select(&cards) {
        for action in card.select(&actions) {
            let Ok(url) = base.join(action.value().attr("href").unwrap()) else {
                continue;
            };
            if url.origin() != base.origin()
                || !matches!(
                    url.path(),
                    "/mywodbusterarena/competitionjudge.aspx"
                        | "/mywodbusterarena/competitionadmin/competition.aspx"
                )
            {
                continue;
            }
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "id")
                .and_then(|(_, value)| value.parse::<i64>().ok())
                .filter(|id| *id > 0)
                .ok_or(CompetitionError::InvalidPage)?;
            let name = card
                .select(&names)
                .next()
                .ok_or(CompetitionError::InvalidPage)?
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if name.is_empty() {
                return Err(CompetitionError::InvalidPage);
            }
            let competition = competitions
                .entry(id)
                .or_insert_with(|| Competition::new(id, name));
            match url.path() {
                "/mywodbusterarena/competitionjudge.aspx" => {
                    competition.judge_url = Some(url.to_string())
                }
                _ => competition.admin_url = Some(url.to_string()),
            }
        }
    }
    Ok(competitions.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_judge_and_admin_ids_excluding_athlete_only_cards() {
        let html = r#"<div class="competitions">
          <div class="competition"><div class="nombre"> South &amp; Games </div><div class="primaryactions">
            <a href="/mywodbusterarena/competitionjudge.aspx?id=3910">Judge</a></div></div>
          <div class="competition"><div class="nombre">InterBOX 909</div><div class="primaryactions">
            <a href="/mywodbusterarena/competitionadmin/competition.aspx?id=1246">Administrador</a>
            <a href="/mywodbusterarena/competitionjudge.aspx?id=1246">Juez</a></div></div>
          <div class="competition"><div class="nombre">InterBOX 909</div><div class="secondaryactions">
            <a href="/mywodbusterarena/competitioncaptain.aspx?id=519">Athlete zone</a></div><div class="primaryactions">
            <a href="/competition.aspx?id=519">Leaderboard</a></div></div>
          <div class="competition"><div class="nombre">Judge games</div><div class="primaryactions">
            <a href="https://other.example/mywodbusterarena/competitionjudge.aspx?id=777">Judge</a></div></div>
        </div>"#;
        assert_eq!(
            parse_competitions(html).unwrap(),
            vec![
                Competition {
                    id: 1246,
                    name: "InterBOX 909".into(),
                    judge_url: Some(format!(
                        "{ARENA_URL}mywodbusterarena/competitionjudge.aspx?id=1246"
                    )),
                    admin_url: Some(format!(
                        "{ARENA_URL}mywodbusterarena/competitionadmin/competition.aspx?id=1246"
                    )),
                },
                Competition {
                    id: 3910,
                    name: "South & Games".into(),
                    judge_url: Some(format!(
                        "{ARENA_URL}mywodbusterarena/competitionjudge.aspx?id=3910"
                    )),
                    admin_url: None,
                },
            ]
        );
    }

    #[test]
    fn distinguishes_empty_list_from_invalid_page() {
        assert!(
            parse_competitions("<div class=competitions></div>")
                .unwrap()
                .is_empty()
        );
        assert!(parse_competitions("<html><input type=password></html>").is_err());
        assert!(parse_competitions(r#"<div class=competitions><div class=competition><div class=nombre>Invalid</div><div class=primaryactions><a href="/mywodbusterarena/competitionjudge.aspx?id=no">Judge</a></div></div></div>"#).is_err());
    }
}
