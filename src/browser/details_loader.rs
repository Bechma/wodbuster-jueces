use super::*;
use crate::{
    competitions::Competition,
    details::{self, Category, CompetitionDetail, Score, Team, Wod},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

struct Connection {
    browser: Browser,
    page: Page,
    context: chromiumoxide::cdp::browser_protocol::browser::BrowserContextId,
    _driver: Driver,
}

struct Resource {
    requested: String,
    url: String,
    status: u64,
    body: String,
}
impl Resource {
    fn available(&self) -> bool {
        self.status == 200
            && Url::parse(&self.url)
                .ok()
                .zip(Url::parse(&self.requested).ok())
                .is_some_and(|(actual, requested)| {
                    actual.host_str() == requested.host_str() && actual.path() == requested.path()
                })
    }
    fn metadata(&self) -> Value {
        json!({"requested_url":self.requested,"response_url":self.url,"status":self.status,"available":self.available()})
    }
}

impl Connection {
    async fn new(endpoint: &Url, cookie: &AuthCookie, url: &str) -> Result<Self, CompetitionError> {
        let (browser, mut handler) = Browser::connect(endpoint.as_str())
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        let driver = Driver(tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        }));
        let context = browser
            .create_browser_context(CreateBrowserContextParams {
                dispose_on_detach: Some(true),
                ..Default::default()
            })
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        browser
            .execute(SetCookiesParams {
                cookies: vec![cookie.cdp_cookie()],
                browser_context_id: Some(context.clone()),
            })
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        let page = browser
            .new_page(
                CreateTargetParams::builder()
                    .url(url)
                    .browser_context_id(context.clone())
                    .build()
                    .map_err(|_| CompetitionError::Unavailable)?,
            )
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        for _ in 0..40 {
            let status=page.evaluate("({authenticated:!!document.querySelector('a[href*=\"signout.ashx\"]'),challenge:document.title.includes('Just a moment'),login:location.hostname==='wodbuster.com'||!!document.querySelector('input[type=password]')})")
                .await.map_err(|_|CompetitionError::Unavailable)?.into_value::<Value>().map_err(|_|CompetitionError::InvalidPage)?;
            if status["authenticated"] == true {
                return Ok(Self {
                    browser,
                    page,
                    context,
                    _driver: driver,
                });
            }
            if status["login"] == true && status["challenge"] != true {
                return Err(CompetitionError::SessionExpired);
            }
            sleep(Duration::from_millis(500)).await;
        }
        Err(CompetitionError::Verification)
    }
    async fn get_many(&self, paths: &[String]) -> Result<Vec<Resource>, CompetitionError> {
        let base = Url::parse(ARENA_URL).unwrap();
        let mut all = Vec::new();
        for chunk in paths.chunks(6) {
            let urls = chunk
                .iter()
                .map(|path| {
                    let url = base.join(path).map_err(|_| CompetitionError::InvalidPage)?;
                    if url.origin() != base.origin() {
                        return Err(CompetitionError::InvalidPage);
                    }
                    Ok(url.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let script = format!(
                "(async()=>{{const urls={};return await Promise.all(urls.map(async url=>{{const r=await fetch(url,{{method:'GET',credentials:'include',headers:{{'Accept':'application/json,text/html'}}}});return {{requested:url,url:r.url,status:r.status,body:await r.text()}};}}));}})()",
                serde_json::to_string(&urls).map_err(|_| CompetitionError::InvalidPage)?
            );
            let values = self
                .page
                .evaluate(script)
                .await
                .map_err(|_| CompetitionError::Unavailable)?
                .into_value::<Vec<Value>>()
                .map_err(|_| CompetitionError::InvalidPage)?;
            for value in values {
                let resource = Resource {
                    requested: value["requested"].as_str().unwrap_or_default().to_owned(),
                    url: value["url"].as_str().unwrap_or_default().to_owned(),
                    status: value["status"].as_u64().unwrap_or_default(),
                    body: value["body"].as_str().unwrap_or_default().to_owned(),
                };
                if resource.status == 401
                    || Url::parse(&resource.url)
                        .ok()
                        .is_some_and(|url| url.host_str() != Some("arena.wodbuster.com"))
                {
                    return Err(CompetitionError::SessionExpired);
                }
                if resource.body.contains("<title>Just a moment") {
                    return Err(CompetitionError::Verification);
                }
                all.push(resource);
            }
        }
        Ok(all)
    }
    async fn close(self) {
        let _ = timeout(
            Duration::from_secs(3),
            self.browser.dispose_browser_context(self.context.clone()),
        )
        .await;
    }

    /// Cambia únicamente el selector de modo; nunca pulsa los controles de guardar.
    async fn discover_score_branches(
        &self,
        url: &str,
        team: &Team,
        wods: &mut BTreeMap<i64, Wod>,
    ) -> Result<(), CompetitionError> {
        let url = Url::parse(url).map_err(|_| CompetitionError::InvalidPage)?;
        if url.origin() != Url::parse(ARENA_URL).unwrap().origin() {
            return Err(CompetitionError::InvalidPage);
        }
        self.page
            .goto(url.as_str())
            .await
            .map_err(|_| CompetitionError::Unavailable)?;
        for value in ["True", "False"] {
            let script = format!(
                r#"(()=>{{
                const selects=[...document.querySelectorAll('select')].filter(e=>e.name.endsWith('$IoComplete'));
                if (!selects.length) return false;
                document.documentElement.dataset.scoreSchemaPending='true';
                const manager=window.Sys?.WebForms?.PageRequestManager?.getInstance();
                if (manager) {{ const done=()=>{{ document.documentElement.removeAttribute('data-score-schema-pending'); manager.remove_endRequest(done); }}; manager.add_endRequest(done); }}
                selects.forEach(e=>e.value={value:?});
                selects[0].dispatchEvent(new Event('change',{{bubbles:true}}));
                return true;
            }})()"#
            );
            let found = self
                .page
                .evaluate(script)
                .await
                .map_err(|_| CompetitionError::Unavailable)?
                .into_value::<bool>()
                .map_err(|_| CompetitionError::InvalidPage)?;
            if !found {
                return Err(CompetitionError::InvalidPage);
            }
            let mut ready = false;
            for _ in 0..60 {
                sleep(Duration::from_millis(200)).await;
                ready = self
                    .page
                    .evaluate("!document.documentElement.dataset.scoreSchemaPending")
                    .await
                    .ok()
                    .and_then(|v| v.into_value::<bool>().ok())
                    .unwrap_or(false);
                if ready {
                    break;
                }
            }
            if !ready {
                return Err(CompetitionError::Timeout);
            }
            let html = self
                .page
                .content()
                .await
                .map_err(|_| CompetitionError::Unavailable)?;
            // No reemplaza los valores originales del equipo por los de la exploración.
            let mut temporary = team.clone();
            details::enrich_judge_form(&html, &mut temporary, wods);
            let suffix = if value == "True" {
                "$IoTimeMMSS"
            } else {
                "$IoReps"
            };
            let has_control = |wod: &Wod, suffix: &str| {
                wod.metadata["score_fields"]
                    .as_array()
                    .is_some_and(|fields| {
                        fields.iter().any(|f| {
                            f["attributes"]["name"]
                                .as_str()
                                .is_some_and(|n| n.ends_with(suffix))
                        })
                    })
            };
            if wods
                .values()
                .any(|w| has_control(w, "$IoComplete") && !has_control(w, suffix))
            {
                return Err(CompetitionError::InvalidPage);
            }
        }
        Ok(())
    }
}

impl ArenaBrowser {
    pub async fn competition_detail(
        &self,
        cookie: &AuthCookie,
        id: i64,
    ) -> Result<CompetitionDetail, CompetitionError> {
        let source = self
            .competitions(cookie)
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or(CompetitionError::AccessDenied)?;
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| CompetitionError::Busy)?;
        timeout(
            Duration::from_secs(15 * 60),
            self.load_detail(cookie, source),
        )
        .await
        .map_err(|_| CompetitionError::Timeout)?
    }

    async fn load_detail(
        &self,
        cookie: &AuthCookie,
        competition: Competition,
    ) -> Result<CompetitionDetail, CompetitionError> {
        let id = competition.id;
        let index = Connection::new(
            &self.endpoint,
            cookie,
            &format!("{ARENA_URL}competition.aspx?id={id}"),
        )
        .await?;
        let initial = details::parse_leaderboard(
            &index
                .page
                .content()
                .await
                .map_err(|_| CompetitionError::Unavailable)?,
        )?;
        let mut categories = initial.categories;
        let mut paths = Vec::new();
        if let Some(judge) = &competition.judge_url {
            paths.push(judge.clone());
        }
        if competition.admin_url.is_some() {
            paths.extend([
                format!("/mywodbusterarena/competitionadmin/divisions.aspx?id={id}"),
                format!("/mywodbusterarena/competitionadmin/wods.aspx?id={id}"),
                format!("/mywodbusterarena/competitionadmin/athletes.aspx?id={id}"),
            ]);
        }
        let resources = index.get_many(&paths).await?;
        let mut admin_categories = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        let mut source_pages = Vec::new();
        for resource in resources {
            source_pages.push(resource.metadata());
            if !resource.available() {
                continue;
            }
            if resource.requested.contains("competitionjudge.aspx") {
                for category in details::judge_categories(&resource.body) {
                    if !categories.iter().any(|(id, _)| *id == category.0) {
                        categories.push(category);
                    }
                }
            } else if resource.requested.contains("wods.aspx") {
                definitions = details::admin_wods(&resource.body);
            } else {
                for (name, info) in details::admin_category_metadata(&resource.body) {
                    admin_categories
                        .entry(name)
                        .or_insert_with(Vec::new)
                        .push(json!({"source_url":resource.requested,"data":info}));
                }
            }
        }
        index.close().await;
        let mut snapshot = CompetitionDetail {
            id,
            name: competition.name,
            categories: Vec::new(),
            metadata: json!({"judge_url":competition.judge_url,"admin_url":competition.admin_url,"leaderboard_url":format!("{ARENA_URL}competition.aspx?id={id}"),"sources":source_pages}),
            loaded_at: None,
        };
        for (category_id, name) in categories {
            let mut category = timeout(
                Duration::from_secs(100),
                self.load_category(cookie, id, category_id, name.clone(), &definitions),
            )
            .await
            .map_err(|_| CompetitionError::Timeout)??;
            if let Some(metadata) = admin_categories.get(&name) {
                category.metadata["admin"] = json!(metadata);
            }
            snapshot.categories.push(category);
        }
        crate::scoring::prepare_detail(&mut snapshot);
        Ok(snapshot)
    }

    async fn load_category(
        &self,
        cookie: &AuthCookie,
        competition_id: i64,
        id: i64,
        name: String,
        definitions: &BTreeMap<i64, Wod>,
    ) -> Result<Category, CompetitionError> {
        let source_url = format!("{ARENA_URL}competition.aspx?id={competition_id}&idd={id}");
        let connection = Connection::new(&self.endpoint, cookie, &source_url).await?;
        let board = details::parse_leaderboard(
            &connection
                .page
                .content()
                .await
                .map_err(|_| CompetitionError::Unavailable)?,
        )?;
        let mut wods = BTreeMap::new();
        for (order, (wod_id, wod_name)) in board.wods.into_iter().enumerate() {
            wods.insert(wod_id,definitions.get(&wod_id).cloned().unwrap_or(Wod{ score_format: None,id:wod_id,name:wod_name,description:String::new(),scoring:String::new(),metadata:json!({"order":order,"leaderboard_url":format!("{source_url}&idw={wod_id}")})}));
        }
        let mut teams = board.teams;
        let team_paths = teams
            .iter()
            .map(|team| format!("/api/CompeticionUI/GetInfoAtleta/{}", team.reference))
            .collect::<Vec<_>>();
        for (team, resource) in teams
            .iter_mut()
            .zip(connection.get_many(&team_paths).await?)
        {
            if !resource.available() {
                return Err(CompetitionError::Unavailable);
            }
            let info: Value =
                serde_json::from_str(&resource.body).map_err(|_| CompetitionError::InvalidPage)?;
            if info["categoria"]
                .as_str()
                .is_some_and(|category| category.trim() != name.trim())
            {
                return Err(CompetitionError::InvalidPage);
            }
            details::apply_team_info(team, info, &mut wods)?;
            for score in &mut team.scores {
                score.metadata["scorecard_url"] = json!(format!(
                    "{ARENA_URL}api/CompeticionUI/GetScoreCard?id={competition_id}&idWod={}&idUsuarioCategoria={}",
                    score.wod_id, team.reference
                ));
            }
        }
        // La vista de juez aporta gid y estado de revisión, incluso antes de publicarse los WODs.
        let judge_path = format!("/mywodbusterarena/competitionjudge.aspx?idc={id}");
        let judge = connection
            .get_many(std::slice::from_ref(&judge_path))
            .await?
            .remove(0);
        let judge_status = judge.metadata();
        if judge.available() {
            for mut judged in details::judge_teams(&judge.body) {
                let gid = judged
                    .gid
                    .as_deref()
                    .unwrap_or_default()
                    .replace('-', "")
                    .to_lowercase();
                let matches = teams
                    .iter()
                    .enumerate()
                    .filter(|(_, team)| {
                        team.members.as_array().is_some_and(|members| {
                            members.iter().any(|member| {
                                member["Id"].as_str().is_some_and(|id| {
                                    id.replace('-', "").eq_ignore_ascii_case(&gid)
                                })
                            })
                        })
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let matched = if matches.len() == 1 {
                    Some(matches[0])
                } else {
                    let names = teams
                        .iter()
                        .enumerate()
                        .filter(|(_, team)| {
                            team.name.trim().eq_ignore_ascii_case(judged.name.trim())
                        })
                        .map(|(index, _)| index)
                        .collect::<Vec<_>>();
                    if names.len() == 1 {
                        Some(names[0])
                    } else {
                        None
                    }
                };
                if let Some(index) = matched {
                    teams[index].gid = judged.gid;
                    teams[index].metadata["judge"] = judged.metadata;
                } else {
                    judged.metadata["judge"] = judged.metadata.clone();
                    judged.scores = wods
                        .keys()
                        .map(|id| Score {
                            value: None,
                            wod_id: *id,
                            result: None,
                            rank: None,
                            points: None,
                            metadata: json!({}),
                        })
                        .collect();
                    teams.push(judged);
                }
            }
            let positions = teams
                .iter()
                .enumerate()
                .filter(|(_, team)| team.gid.is_some())
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let paths = positions
                .iter()
                .map(|index| {
                    format!(
                        "/mywodbusterarena/competitionjudgedivision.aspx?id={id}&gid={}",
                        teams[*index].gid.as_deref().unwrap()
                    )
                })
                .collect::<Vec<_>>();
            let mut format_source = None;
            for (position, form) in positions.iter().zip(connection.get_many(&paths).await?) {
                teams[*position].metadata["judge_form_source"] = form.metadata();
                if form.available() {
                    details::enrich_judge_form(&form.body, &mut teams[*position], &mut wods);
                    if format_source.is_none() && form.body.contains("IoComplete") {
                        format_source = Some((*position, form.requested.clone()));
                    }
                }
            }
            if let Some((position, url)) = format_source {
                connection
                    .discover_score_branches(&url, &teams[position], &mut wods)
                    .await?;
            }
        }
        // Añade definiciones aunque una categoría no tenga inscritos.
        for definition in definitions
            .values()
            .filter(|wod| wod.metadata["category"].as_str() == Some(&name))
        {
            wods.entry(definition.id)
                .or_insert_with(|| definition.clone());
        }
        let mut breakdown_targets = Vec::new();
        for (team_index, team) in teams.iter_mut().enumerate() {
            if let Some(registration) = team.registration_id {
                for (score_index, score) in team.scores.iter_mut().enumerate() {
                    let path = format!(
                        "/api/CompeticionUI/GetDesglosePuntos?id={competition_id}&idWod={}&idUsuarioCategoria={registration}",
                        score.wod_id
                    );
                    score.metadata["breakdown_url"] =
                        json!(format!("{}{}", ARENA_URL.trim_end_matches('/'), path));
                    breakdown_targets.push((team_index, score_index, path));
                }
            }
        }
        let mut breakdown_source = json!(null);
        if let Some((team_index, score_index, path)) = breakdown_targets.first() {
            let pilot = connection
                .get_many(std::slice::from_ref(path))
                .await?
                .remove(0);
            breakdown_source = pilot.metadata();
            let data = serde_json::from_str::<Value>(&pilot.body)
                .ok()
                .filter(|value| value["Filas"].is_array());
            if pilot.available()
                && let Some(data) = data
            {
                teams[*team_index].scores[*score_index].metadata["breakdown"] = data;
                let paths = breakdown_targets
                    .iter()
                    .skip(1)
                    .map(|(_, _, path)| path.clone())
                    .collect::<Vec<_>>();
                for ((team_index, score_index, _), resource) in breakdown_targets
                    .iter()
                    .skip(1)
                    .zip(connection.get_many(&paths).await?)
                {
                    let score = &mut teams[*team_index].scores[*score_index];
                    score.metadata["breakdown_source"] = resource.metadata();
                    if resource.available()
                        && let Ok(data) = serde_json::from_str::<Value>(&resource.body)
                    {
                        score.metadata["breakdown"] = data;
                    }
                }
            }
        }
        connection.close().await;
        Ok(Category {
            id,
            name,
            wods: wods.into_values().collect(),
            teams,
            metadata: json!({"leaderboard_url":source_url,"judge_page":judge_status,"breakdown_endpoint":breakdown_source}),
        })
    }
}

#[cfg(test)]
mod scoring_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Comprueba con Browserless que descubrir los modos no guarda puntuaciones en Arena"]
    async fn discovering_modes_does_not_save_scores() -> topcoat::Result<()> {
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env()?;
        let pool = sqlx::PgPool::connect(&config.database_url).await?;
        let id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT id FROM wodbuster_sessions ORDER BY saved_at DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await?;
        let store = crate::storage::SessionStore::new(pool, &config.session_secret);
        let saved = store.load(id).await?.unwrap();
        let snapshot = store
            .competition_detail(&saved.username, 3910)
            .await?
            .unwrap();
        let category = snapshot
            .categories
            .iter()
            .find(|c| c.name == "INT Mixto")
            .unwrap();
        let team = category
            .teams
            .iter()
            .find(|t| t.gid.is_some() && t.registration_id.is_some())
            .unwrap();
        let url = team.metadata["judge_form_source"]["requested_url"]
            .as_str()
            .unwrap();
        let api = format!(
            "/api/CompeticionUI/GetInfoAtleta/{}",
            team.registration_id.unwrap()
        );
        let connection = Connection::new(&config.browser_url, &saved.cookie, url).await?;
        let before = connection
            .get_many(std::slice::from_ref(&api))
            .await?
            .remove(0);
        let mut wods = category.wods.iter().cloned().map(|w| (w.id, w)).collect();
        let result = connection
            .discover_score_branches(url, team, &mut wods)
            .await;
        let after = connection
            .get_many(std::slice::from_ref(&api))
            .await?
            .remove(0);
        connection.close().await;
        result?;
        assert!(before.available() && after.available());
        let scores = |body: &str| -> Vec<Value> {
            serde_json::from_str::<Value>(body).unwrap()["Wods"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| json!({"id":w["Id"],"score":w["Score"],"points":w["Points"]}))
                .collect()
        };
        assert_eq!(scores(&before.body), scores(&after.body));
        Ok(())
    }
}
