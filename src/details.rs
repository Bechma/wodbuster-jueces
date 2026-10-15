use crate::{browser::ARENA_URL, competitions::CompetitionError};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompetitionDetail {
    pub id: i64,
    pub name: String,
    pub categories: Vec<Category>,
    pub metadata: Value,
    #[serde(default)]
    pub loaded_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub wods: Vec<Wod>,
    pub teams: Vec<Team>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Wod {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub scoring: String,
    pub metadata: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_format: Option<crate::scoring::ScoreFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Team {
    /// Inscripción numérica si existe; en otro caso, el gid expuesto por Arena.
    pub reference: String,
    pub registration_id: Option<i64>,
    pub gid: Option<String>,
    pub name: String,
    pub affiliate: String,
    pub bib: String,
    pub rank: String,
    pub points: String,
    pub members: Value,
    pub scores: Vec<Score>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Score {
    pub wod_id: i64,
    pub result: Option<String>,
    pub rank: Option<String>,
    pub points: Option<String>,
    pub metadata: Value,
    /// Valor interpretado; si el formato no se reconoce, `result` conserva el original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<crate::scoring::ScoreValue>,
}

impl Score {
    /// El resultado original null es la fuente de verdad, no los valores del formulario.
    pub fn is_scored(&self) -> bool {
        self.result.is_some()
    }
}

pub(crate) struct Leaderboard {
    pub categories: Vec<(i64, String)>,
    pub wods: Vec<(i64, String)>,
    pub teams: Vec<Team>,
}

pub(crate) fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("Selector estático válido")
}
pub(crate) fn text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn fragment_text(html: &str) -> String {
    text(Html::parse_fragment(html).root_element())
}

fn description_text(element: ElementRef<'_>) -> String {
    #[derive(Default)]
    struct Lines {
        text: String,
        pending_space: bool,
    }
    impl Lines {
        fn boundary(&mut self) {
            self.pending_space = false;
            if !self.text.is_empty() && !self.text.ends_with('\n') {
                self.text.push('\n');
            }
        }
        fn append(&mut self, text: &str) {
            for ch in text.chars() {
                // Colapsa el espacio del código HTML, conservando los &nbsp; visibles.
                if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000c}') {
                    self.pending_space = !self.text.is_empty() && !self.text.ends_with('\n');
                } else {
                    if self.pending_space {
                        self.text.push(' ');
                    }
                    self.pending_space = false;
                    self.text.push(ch);
                }
            }
        }
    }
    fn visit(element: ElementRef<'_>, lines: &mut Lines) {
        let name = element.value().name();
        if matches!(name, "button" | "script" | "style" | "template")
            || element.value().attr("hidden").is_some()
        {
            return;
        }
        if name == "br" {
            lines.text.push('\n');
            lines.pending_space = false;
            return;
        }
        let block = matches!(
            name,
            "div"
                | "p"
                | "li"
                | "ul"
                | "ol"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "pre"
                | "section"
                | "tr"
                | "hr"
        );
        if block {
            lines.boundary();
        }
        for node in element.children() {
            if let Some(text) = node.value().as_text() {
                lines.append(text);
            } else if let Some(child) = ElementRef::wrap(node) {
                visit(child, lines);
            }
        }
        if block {
            lines.boundary();
        }
    }
    let mut lines = Lines::default();
    visit(element, &mut lines);
    lines.text.trim().to_owned()
}
fn child_text(element: ElementRef<'_>, css: &str) -> String {
    element
        .select(&selector(css))
        .next()
        .map(text)
        .unwrap_or_default()
}

pub(crate) fn parse_leaderboard(html: &str) -> Result<Leaderboard, CompetitionError> {
    let doc = Html::parse_document(html);
    let options = |css| -> Vec<(i64, String)> {
        doc.select(&selector(css))
            .filter_map(|option| {
                Some((
                    option.value().attr("value")?.parse::<i64>().ok()?,
                    text(option),
                ))
            })
            .filter(|(id, _)| *id > 0)
            .collect()
    };
    if doc
        .select(&selector("select[onchange*=redirectFromDivisions]"))
        .next()
        .is_none()
    {
        return Err(CompetitionError::InvalidPage);
    }
    let mut teams = Vec::new();
    for anchor in doc.select(&selector("a[onclick*=ShowInfo]")) {
        let handler = anchor.value().attr("onclick").unwrap_or_default();
        let id = handler
            .split("ShowInfo(")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|id| *id > 0)
            .ok_or(CompetitionError::InvalidPage)?;
        let name = child_text(anchor, ".name");
        if name.is_empty() {
            return Err(CompetitionError::InvalidPage);
        }
        teams.push(Team {
            reference: id.to_string(),
            registration_id: Some(id),
            gid: None,
            name,
            affiliate: child_text(anchor, ".affiliate"),
            bib: child_text(anchor, ".dorsal"),
            rank: child_text(anchor, ".score"),
            points: child_text(anchor, ".result"),
            members: json!([]),
            scores: Vec::new(),
            metadata: json!({"info_url": format!("{ARENA_URL}competitioninfo.aspx?id={id}"),
                "api_url": format!("{ARENA_URL}api/CompeticionUI/GetInfoAtleta/{id}"),
                "leaderboard_text": text(anchor)}),
        });
    }
    Ok(Leaderboard {
        categories: options("select[onchange*=redirectFromDivisions] option"),
        wods: options("select[onchange*=redirectFromWods] option"),
        teams,
    })
}

pub(crate) fn judge_categories(html: &str) -> Vec<(i64, String)> {
    let doc = Html::parse_document(html);
    doc.select(&selector("a[href*=competitionjudge]"))
        .filter_map(|a| Some((query_id(a.value().attr("href")?, "idc")?, text(a))))
        .collect()
}

pub(crate) fn query_id(url: &str, key: &str) -> Option<i64> {
    Url::parse(ARENA_URL)
        .ok()?
        .join(url)
        .ok()?
        .query_pairs()
        .find(|(name, _)| name == key)?
        .1
        .parse()
        .ok()
}

pub(crate) fn judge_teams(html: &str) -> Vec<Team> {
    let doc = Html::parse_document(html);
    let mut teams = Vec::new();
    for row in doc.select(&selector(".athleteresult")) {
        let Some(link) = row
            .select(&selector("a[href*=competitionjudgedivision]"))
            .next()
        else {
            continue;
        };
        let href = link.value().attr("href").unwrap();
        let Ok(url) = Url::parse(ARENA_URL).unwrap().join(href) else {
            continue;
        };
        let Some(gid) = url
            .query_pairs()
            .find(|(key, _)| key == "gid")
            .map(|(_, v)| v.into_owned())
        else {
            continue;
        };
        let name = child_text(row, ".name");
        if name.is_empty() {
            continue;
        }
        teams.push(Team {
            reference: gid.clone(), registration_id: None, gid: Some(gid), name,
            affiliate: child_text(row, ".affiliate"), bib: child_text(row, ".dorsal"),
            rank: child_text(row, ".score"), points: String::new(), members: json!([]), scores: Vec::new(),
            metadata: json!({"judge_url": url.as_str(), "judge_text": text(row),
                "score_statuses": row.select(&selector(".athlete > div")).skip(2).map(text).collect::<Vec<_>>() }),
        });
    }
    teams
}

pub(crate) fn apply_team_info(
    team: &mut Team,
    info: Value,
    wods: &mut BTreeMap<i64, Wod>,
) -> Result<(), CompetitionError> {
    let Some(items) = info.get("Wods").and_then(Value::as_array) else {
        return Err(CompetitionError::InvalidPage);
    };
    if let Some(name) = info["TeamName"].as_str().filter(|s| !s.trim().is_empty()) {
        team.name = name.trim().to_owned();
    }
    team.affiliate = info["Box"]
        .as_str()
        .unwrap_or(&team.affiliate)
        .trim()
        .to_owned();
    team.rank = info["Rank"].as_str().unwrap_or(&team.rank).to_owned();
    team.points = info["Score"].as_str().unwrap_or(&team.points).to_owned();
    team.members = info.get("AllAtletas").cloned().unwrap_or(json!([]));
    for item in items {
        let id = item["Id"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(CompetitionError::InvalidPage)?;
        let name = item["Wod"].as_str().unwrap_or_default().to_owned();
        wods.entry(id).or_insert(Wod {
            score_format: None,
            id,
            name,
            description: String::new(),
            scoring: String::new(),
            metadata: json!({}),
        });
        team.scores.push(Score {
            value: None,
            wod_id: id,
            result: item["Score"].as_str().map(fragment_text),
            rank: item["Rank"].as_str().map(str::to_owned),
            points: item["Points"].as_str().map(fragment_text),
            metadata: json!({"api":item}),
        });
    }
    team.metadata["api"] = info;
    Ok(())
}

pub(crate) fn admin_category_metadata(html: &str) -> BTreeMap<String, Value> {
    let doc = Html::parse_document(html);
    let mut data = BTreeMap::new();
    for table in doc.select(&selector("#maincolumn table")) {
        let headers = table.select(&selector("th")).map(text).collect::<Vec<_>>();
        if !headers
            .iter()
            .any(|h| matches!(h.as_str(), "Name" | "Nombre" | "Category" | "Categoría"))
        {
            continue;
        }
        for row in table.select(&selector("tr")) {
            let cells = row.select(&selector("td")).map(text).collect::<Vec<_>>();
            if cells.is_empty() {
                continue;
            }
            let name_index = headers
                .iter()
                .position(|h| matches!(h.as_str(), "Name" | "Nombre" | "Category" | "Categoría"))
                .unwrap();
            if let Some(name) = cells.get(name_index) {
                data.insert(name.clone(), json!({"columns":headers,"values":cells}));
            }
        }
    }
    data
}

pub(crate) fn admin_wods(html: &str) -> BTreeMap<i64, Wod> {
    let doc = Html::parse_document(html);
    let mut wods = BTreeMap::new();
    for link in doc.select(&selector("a[data-open^=wd]")) {
        let Some(id) = link
            .value()
            .attr("data-open")
            .and_then(|id| id.strip_prefix("wd"))
            .and_then(|id| id.parse::<i64>().ok())
        else {
            continue;
        };
        let Some(row) = link
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "tr")
        else {
            continue;
        };
        let cells = row.select(&selector("td")).map(text).collect::<Vec<_>>();
        let description = doc
            .select(&selector(&format!("#wd{id} .reveralInner")))
            .next();
        let category = row
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().classes().any(|class| class == "table-scroll"))
            .and_then(|table| {
                table
                    .prev_siblings()
                    .filter_map(ElementRef::wrap)
                    .find(|e| e.value().name() == "h3")
            })
            .map(text);
        wods.insert(id, Wod { score_format: None,id, name:cells.first().cloned().unwrap_or_default(),
            description:description.map(description_text).unwrap_or_default(), scoring:cells.get(2).cloned().unwrap_or_default(),
            metadata:json!({"columns":cells,"category":category,"description_html":description.map(|e|e.inner_html()),"description_reference":format!("wd{id}")}),
        });
    }
    wods
}

/// Solo conserva los controles del WOD, nunca CSRF, ViewState, cookies ni scripts.
pub(crate) fn score_fields(panel: ElementRef<'_>) -> Value {
    let mut fields = Vec::new();
    for control in panel.select(&selector(
        "input:not([type=hidden]):not([type=submit]),select,textarea",
    )) {
        let id = control.value().attr("id").unwrap_or_default();
        let label = panel
            .select(&selector("label"))
            .find(|l| l.value().attr("for") == Some(id))
            .map(text)
            .unwrap_or_default();
        let attrs = control
            .value()
            .attrs()
            .filter(|(key, _)| {
                matches!(
                    *key,
                    "id" | "name"
                        | "type"
                        | "class"
                        | "min"
                        | "max"
                        | "step"
                        | "pattern"
                        | "maxlength"
                        | "required"
                        | "placeholder"
                        | "disabled"
                        | "readonly"
                        | "inputmode"
                )
            })
            .map(|(k, v)| (k.to_owned(), Value::String(v.to_owned())))
            .collect::<serde_json::Map<_, _>>();
        let options = control.select(&selector("option")).map(|o| json!({"value":o.value().attr("value"),"label":text(o),"selected":o.value().attr("selected").is_some()})).collect::<Vec<_>>();
        let value = if control.value().name() == "select" {
            control
                .select(&selector("option[selected]"))
                .next()
                .and_then(|o| o.value().attr("value"))
                .unwrap_or_default()
                .to_owned()
        } else if control.value().name() == "textarea" {
            text(control)
        } else {
            control.value().attr("value").unwrap_or_default().to_owned()
        };
        let group = control
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().classes().any(|class| class == "sc_GlobalBox"));
        fields.push(json!({"attributes":attrs,"label":label,"value":value,"options":options,"checked":control.value().attr("checked").is_some(),
            "format":group.map(|g|g.value().classes().collect::<Vec<_>>()),
            "hint":group.map(|g|child_text(g,".sc_aclaracion")),
            "validation":group.map(|g|g.select(&selector(".sc_Validator")).map(text).collect::<Vec<_>>())}));
    }
    json!(fields)
}

pub(crate) fn enrich_judge_form(html: &str, team: &mut Team, wods: &mut BTreeMap<i64, Wod>) {
    let doc = Html::parse_document(html);
    let panels = doc
        .select(&selector("div[id*=CtlWodsResults][id*=CtlUpMain]"))
        .collect::<Vec<_>>();
    // La página de juez y el API de la inscripción enumeran los WODs en el mismo orden.
    // Si el número difiere, conserva los controles sin inventar asociaciones.
    if panels.len() != team.scores.len() {
        team.metadata["unmatched_score_fields"] =
            json!(panels.iter().map(|p| score_fields(*p)).collect::<Vec<_>>());
        return;
    }
    for (panel, score) in panels.into_iter().zip(team.scores.iter_mut()) {
        score.metadata["fields"] = score_fields(panel);
        let heading = panel
            .prev_siblings()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "h4")
            .map(text);
        let description = panel
            .select(&selector("a[data-open]"))
            .next()
            .and_then(|a| a.value().attr("data-open"));
        if let Some(wod) = wods.get_mut(&score.wod_id) {
            if let Some(name) = heading {
                wod.name = name;
            }
            if let Some(reference) = description
                && let Some(block) = doc
                    .select(&selector(&format!("#{reference} .reveralInner")))
                    .next()
            {
                wod.description = description_text(block);
                wod.metadata["description_html"] = json!(block.inner_html());
                wod.metadata["description_reference"] = json!(reference);
            }
            let mut schema = score_fields(panel);
            for field in schema.as_array_mut().unwrap() {
                field.as_object_mut().unwrap().remove("value");
                field.as_object_mut().unwrap().remove("checked");
                for option in field["options"].as_array_mut().unwrap() {
                    option.as_object_mut().unwrap().remove("selected");
                }
            }
            if !wod.metadata["score_fields"].is_array() {
                wod.metadata["score_fields"] = json!([]);
            }
            let definitions = wod.metadata["score_fields"].as_array_mut().unwrap();
            for field in schema.as_array().unwrap() {
                if !definitions
                    .iter()
                    .any(|existing| existing["attributes"]["name"] == field["attributes"]["name"])
                {
                    definitions.push(field.clone());
                }
            }
        }
        score.metadata["save_controls"] = json!(
            panel
                .select(&selector("input[type=submit]"))
                .map(|e| json!({"id":e.value().attr("id"),"name":e.value().attr("name")}))
                .collect::<Vec<_>>()
        );
    }
    if let Some(extras) = doc.select(&selector("fieldset")).find(|e| {
        e.select(&selector("input[id*=IoExtraPoints]"))
            .next()
            .is_some()
    }) {
        team.metadata["extras"] = score_fields(extras);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_description_lines_without_breaking_inline_words() {
        let html = r#"
            <div>
                <div>For time: <strong>Cap 13'</strong></div>
                <div>450m Run</div>
                <div>10 <span>Synchro</span> BBJO</div>
                <div>&nbsp;</div>
                <div>30 Heavy Wall Ball<br>12/9</div>
                <div><br></div>
                <p>Scale: <strong>10</strong>kg &amp; 8kg</p>
            </div>
            <button class="close-button">×</button>
            <script>ignored script</script>
            <div hidden>ignored hidden content</div>
        "#;
        assert_eq!(
            description_text(Html::parse_fragment(html).root_element()),
            "For time: Cap 13'\n450m Run\n10 Synchro BBJO\n\u{a0}\n30 Heavy Wall Ball\n12/9\n\nScale: 10kg & 8kg"
        );
    }

    #[test]
    fn captures_registration_and_wod_references_and_preserves_null_scores() {
        let html = r#"<select onchange="redirectFromDivisions(this)"><option value="12">RX</option></select>
        <select onchange="redirectFromWods(this)"><option value="">General</option><option value="34">Evento 1</option></select>
        <a onclick="return ShowInfo(56)"><div class="athleteresult"><div class="score">1</div><div class="athlete"><div class="name">Team &amp; Co</div><div class="affiliate">Box</div><span class="dorsal">Dorsal: 7</span></div><div class="result">100pts</div></div></a>"#;
        let board = parse_leaderboard(html).unwrap();
        assert_eq!(board.categories, vec![(12, "RX".into())]);
        assert_eq!(board.wods, vec![(34, "Evento 1".into())]);
        let mut team = board.teams.into_iter().next().unwrap();
        assert_eq!(team.registration_id, Some(56));
        assert_eq!(team.reference, "56");
        assert_eq!(team.name, "Team & Co");
        assert_eq!(team.bib, "Dorsal: 7");
        let mut wods = BTreeMap::new();
        apply_team_info(&mut team,json!({"TeamName":"Team & Co","categoria":"RX","AllAtletas":[{"Id":"member-reference","Nombre":"Member"}],"Wods":[{"Id":34,"Wod":"Evento 1","Score":null,"Points":null,"Rank":null,"Horario":{"NumHeat":2,"Stage":"A"}},{"Id":35,"Wod":"Evento 2","Score":"07:20","Points":"<div>95pts</div>","Rank":"2"}]}),&mut wods).unwrap();
        assert!(team.scores[0].result.is_none());
        assert!(team.scores[0].points.is_none());
        assert_eq!(team.scores[0].metadata["api"]["Horario"]["NumHeat"], 2);
        assert_eq!(team.scores[1].points.as_deref(), Some("95pts"));
        assert_eq!(team.members[0]["Id"], "member-reference");
        assert_eq!(wods.len(), 2);
    }

    #[test]
    fn keeps_judge_gid_descriptions_and_fields_without_request_secrets() {
        let row = r#"<div class="athleteresult"><div class="athlete"><div class="name">Team</div></div><div class="result"><a href="/mywodbusterarena/competitionjudgedivision.aspx?id=12&amp;gid=abc123">Judge</a></div></div>"#;
        let mut team = judge_teams(row).remove(0);
        assert_eq!(team.gid.as_deref(), Some("abc123"));
        assert!(team.registration_id.is_none());
        team.scores = vec![Score {
            value: None,
            wod_id: 34,
            result: None,
            rank: None,
            points: None,
            metadata: json!({}),
        }];
        let mut wods = BTreeMap::from([(
            34,
            Wod {
                score_format: None,
                id: 34,
                name: "Hidden".into(),
                description: String::new(),
                scoring: String::new(),
                metadata: json!({}),
            },
        )]);
        let form = r#"<h4>Wod real</h4><div id="CtlWodsResults_CtlUpMain"><input type="hidden" name="CSRFToken" value="secret-token"><input type="hidden" name="__VIEWSTATE" value="secret-state"><label for="tie">Desempate</label><input id="tie" name="score$tie" value="03:21"><label for="unit">Unidad</label><select id="unit" name="score$unit"><option value="10" selected>Kg</option><option value="20">Lb</option></select><a data-open="description">Description</a><input id="save" name="score$save" type="submit"></div><div id="description"><div class="reveralInner"><div>AMRAP 10 minutos</div><div>10 burpees</div></div></div>"#;
        enrich_judge_form(form, &mut team, &mut wods);
        assert_eq!(wods[&34].name, "Wod real");
        assert_eq!(wods[&34].description, "AMRAP 10 minutos\n10 burpees");
        let fields = &team.scores[0].metadata["fields"];
        assert_eq!(fields[0]["attributes"]["name"], "score$tie");
        assert_eq!(fields[0]["value"], "03:21");
        assert_eq!(fields[1]["value"], "10");
        assert_eq!(fields[1]["options"].as_array().unwrap().len(), 2);
        let persisted = serde_json::to_string(&team).unwrap();
        assert!(!persisted.contains("secret-token") && !persisted.contains("secret-state"));
        assert_eq!(
            team.scores[0].metadata["save_controls"][0]["name"],
            "score$save"
        );
    }

    #[test]
    fn reads_admin_metadata_and_rejects_unexpected_leaderboards() {
        let html = r#"<div id="maincolumn"><h3>RX</h3><div class="table-scroll"><table><tr><th>Name</th><th>Dates</th><th>Scores</th></tr><tr><td>Evento 1</td><td>15/04/2023</td><td><a data-open="wd34">Description</a>For time</td></tr></table></div></div><div id="wd34"><div class="reveralInner"><div>50 burpees</div><div>100m run</div></div></div>"#;
        let wods = admin_wods(html);
        assert_eq!(wods[&34].description, "50 burpees\n100m run");
        assert_eq!(wods[&34].metadata["category"], "RX");
        assert_eq!(wods[&34].metadata["columns"][1], "15/04/2023");
        assert!(parse_leaderboard("<input type=password>").is_err());
    }
}
