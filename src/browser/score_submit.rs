use super::{details_loader::Connection, *};
use crate::{
    details::{self, CompetitionDetail, Score},
    scorecard::{PreparedScore, ScorecardReading, prepare_score},
};
use anyhow::{Context, bail, ensure};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ScoreSubmissionError {
    /// Un fallo posterior a este punto puede haber guardado la puntuación en Arena.
    pub save_attempted: bool,
    pub busy: bool,
    message: String,
}

impl ArenaBrowser {
    /// Guarda una única puntuación y la verifica en una nueva carga del formulario.
    /// Nunca reintenta el clic de guardar: un fallo posterior puede ser un envío confirmado a medias.
    pub async fn submit_score(
        &self,
        cookie: &AuthCookie,
        detail: &CompetitionDetail,
        reading: &ScorecardReading,
    ) -> Result<Score, ScoreSubmissionError> {
        let mut save_attempted = false;
        let mut busy = false;
        let result: anyhow::Result<Score> = async {
            ensure!(
                cookie.is_valid(),
                "La sesión ha caducado; accede de nuevo a la aplicación."
            );
            let plan = prepare_score(detail, reading).map_err(anyhow::Error::msg)?;
            let _permit = self
                .slots
                .try_acquire()
                .inspect_err(|_| busy = true)
                .context("Browserless está ocupado")?;
            let connection = Connection::new(&self.endpoint, cookie, &plan.form.url)
                .await
                .context("No se pudo abrir el formulario autenticado")?;
            let result = timeout(
                Duration::from_secs(100),
                submit(&connection, detail, &plan, &mut save_attempted),
            )
            .await;
            connection.close().await;
            result.context("Tiempo agotado al procesar el formulario")?
        }
        .await;
        result.map_err(|error| ScoreSubmissionError {
            save_attempted,
            busy,
            message: format!("{error:#}"),
        })
    }
}

async fn command(page: &Page, request: Value) -> anyhow::Result<String> {
    // Los errores CDP pueden contener datos de sesión; se devuelve un mensaje fijo.
    let script = format!(
        "(async()=>{{const request={}; try {{ return await (()=>{{ {} }})(); }} catch(error) {{const codes=['control','disabled','location','wod','scope','validation-target','validation-unavailable','save','option','input','invalid','saved-result'];return 'error:'+ (codes.includes(error.message)?error.message:'script');}} }})()",
        request,
        include_str!("score_form.js")
    );
    let state = page
        .evaluate(script)
        .await
        .ok()
        .and_then(|v| v.into_value::<String>().ok())
        .context("El navegador perdió el contexto del formulario durante la operación")?;
    if let Some(code) = state.strip_prefix("error:") {
        bail!(
            "El formulario no permite la operación {} (motivo: {code})",
            request["operation"].as_str().unwrap_or_default()
        );
    }
    Ok(state)
}

async fn wait_for_postback(page: &Page) -> anyhow::Result<()> {
    for _ in 0..150 {
        sleep(Duration::from_millis(200)).await;
        match command(page, json!({"operation":"wait"})).await.as_deref() {
            Ok("ready") => return Ok(()),
            Ok("error") => bail!("Arena devolvió un error al procesar el formulario"),
            // Durante una navegación completa desaparece temporalmente el contexto JS.
            _ => {}
        }
    }
    bail!("No se confirmó el postback. Comprueba Arena antes de volver a enviar")
}

async fn team_info(connection: &Connection, reference: &str) -> anyhow::Result<Value> {
    ensure!(
        reference.chars().all(|c| c.is_ascii_digit()) && !reference.is_empty(),
        "Referencia de equipo inválida"
    );
    let resource = connection
        .get_many(&[format!("/api/CompeticionUI/GetInfoAtleta/{reference}")])
        .await
        .context("No se pudo consultar la puntuación del equipo")?
        .remove(0);
    ensure!(resource.available(), "El API del equipo no está disponible");
    serde_json::from_str(&resource.body).context("Respuesta de equipo inválida")
}

async fn open_editor(page: &Page, plan: &PreparedScore) -> anyhow::Result<()> {
    let state = command(page, json!({"operation":"open-edit","plan":plan})).await?;
    if state == "pending" {
        wait_for_postback(page).await?;
    }
    command(page, json!({"operation":"check","plan":plan})).await?;
    Ok(())
}

async fn submit(
    connection: &Connection,
    detail: &CompetitionDetail,
    plan: &PreparedScore,
    save_attempted: &mut bool,
) -> anyhow::Result<Score> {
    let team = detail
        .categories
        .iter()
        .find(|c| c.id == plan.category_id)
        .and_then(|c| c.teams.iter().find(|t| t.reference == plan.team_reference))
        .context("Equipo no disponible en el contexto")?;
    // El colector asocia paneles y WODs por orden. Rechaza un snapshot cuyo orden haya cambiado.
    let before = team_info(connection, &plan.team_reference).await?;
    let ids = before["Wods"]
        .as_array()
        .context("Falta la lista de WODs del equipo")?
        .iter()
        .map(|w| w["Id"].as_i64())
        .collect::<Vec<_>>();
    ensure!(
        ids == team
            .scores
            .iter()
            .map(|s| Some(s.wod_id))
            .collect::<Vec<_>>(),
        "Ha cambiado el orden de los WODs; refresca la competición antes de enviar"
    );
    command(&connection.page, json!({"operation":"check","plan":plan})).await?;
    open_editor(&connection.page, plan)
        .await
        .context("No se pudo abrir la edición del WOD")?;
    for step in &plan.form.steps {
        let state = command(
            &connection.page,
            json!({"operation":"step","plan":plan,"step":step}),
        )
        .await?;
        if state == "pending" {
            wait_for_postback(&connection.page).await?;
        }
    }
    ensure!(
        command(&connection.page, json!({"operation":"verify","plan":plan})).await? == "ready",
        "El formulario no contiene la puntuación preparada"
    );
    let validity = command(
        &connection.page,
        json!({"operation":"validate","plan":plan}),
    )
    .await?;
    ensure!(
        validity == "ready",
        "Arena no permite guardar la puntuación: {validity}"
    );
    // Único punto de escritura remota. No repetir este comando si falla.
    *save_attempted = true;
    let state = command(&connection.page, json!({"operation":"save","plan":plan})).await?;
    if state.starts_with("invalid:") {
        *save_attempted = false;
        bail!("Arena no permite guardar la puntuación: {state}");
    }
    wait_for_postback(&connection.page)
        .await
        .context("Se pulsó Guardar, pero no se pudo confirmar la respuesta. Comprueba Arena")?;
    connection
        .page
        .goto(plan.form.url.as_str())
        .await
        .ok()
        .context("Se pulsó Guardar, pero no se pudo recargar el formulario. Comprueba Arena")?;
    let saved = command(
        &connection.page,
        json!({"operation":"read-saved","plan":plan}),
    )
    .await
    .context("No se encontró la puntuación guardada en la vista de juez")?;
    let saved_result = saved
        .strip_prefix("saved:")
        .context("Falta la confirmación del resultado de juez")?;
    // Guardar sustituye los campos por el resultado y CtlEditar. Reabrir permite
    // verificar también modo, unidad y desempate sin volver a guardar.
    open_editor(&connection.page, plan).await?;
    ensure!(
        command(&connection.page, json!({"operation":"verify","plan":plan})).await? == "ready",
        "La puntuación releída no coincide. Comprueba Arena antes de repetir el envío"
    );
    let info = team_info(connection, &plan.team_reference).await?;
    let mut verified = detail.clone();
    let category = verified
        .categories
        .iter_mut()
        .find(|c| c.id == plan.category_id)
        .unwrap();
    let mut wods = category.wods.iter().cloned().map(|w| (w.id, w)).collect();
    let mut current = team.clone();
    current.scores.clear();
    details::apply_team_info(&mut current, info, &mut wods)
        .context("No se pudo interpretar la puntuación guardada")?;
    let html = connection
        .page
        .content()
        .await
        .ok()
        .context("No se pudo releer el formulario")?;
    details::enrich_judge_form(&html, &mut current, &mut wods);
    let confirmed = current
        .scores
        .iter_mut()
        .find(|s| s.wod_id == plan.wod_id)
        .context("No se encontró el WOD en la vista de juez")?;
    // El API público puede seguir en null hasta publicar/revisar la puntuación.
    // El resultado persistido del juez y sus campos releídos son la confirmación.
    confirmed.result = Some(saved_result.to_owned());
    confirmed.metadata["judge_result"] = json!(saved_result);
    confirmed.metadata["judge_verified_url"] = json!(plan.form.url);
    category.teams = vec![current];
    category.wods = wods.into_values().collect();
    crate::scoring::prepare_detail(&mut verified);
    let score = verified
        .categories
        .iter()
        .find(|c| c.id == plan.category_id)
        .unwrap()
        .teams[0]
        .scores
        .iter()
        .find(|s| s.wod_id == plan.wod_id)
        .context("No se encontró el WOD guardado")?;
    ensure!(
        score.value.as_ref() == Some(&plan.value),
        "La vista de juez no confirma la puntuación enviada. Comprueba Arena antes de repetir el envío"
    );
    Ok(score.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        scorecard::{FormStep, ScoreFormPlan},
        scoring::{ControlReference, FieldRole, ScoreValue},
    };

    fn control(name: &str) -> ControlReference {
        if name == "save" {
            return ControlReference {
                name: "wod$CtlSave".into(),
                id: "wod_CtlSave_0".into(),
            };
        }
        ControlReference {
            name: format!("wod${name}"),
            id: name.into(),
        }
    }

    #[tokio::test]
    #[ignore = "Reproduce solo el rellenado del caso real sin pulsar Guardar en Arena"]
    async fn live_prepare_does_not_save() -> anyhow::Result<()> {
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
        let pool = sqlx::PgPool::connect(&config.database_url).await?;
        let session = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT id FROM wodbuster_sessions ORDER BY saved_at DESC LIMIT 1",
        )
        .fetch_one(&pool)
        .await?;
        let store = crate::storage::SessionStore::new(pool.clone(), &config.session_secret);
        let saved = store
            .load(session)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .context("Falta una sesión válida")?;
        let detail = store
            .competition_detail(&saved.username, 3910)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .context("Falta el detalle")?;
        let reading = ScorecardReading {
            category_id: Some(16907),
            wod_id: Some(64459),
            team_reference: Some("298682".into()),
            score_value: Some(ScoreValue::NotCompleted {
                repetitions: 2,
                tie_break: None,
            }),
            judge_name: None,
            needs_retake: false,
            retake_reason: None,
        };
        let plan = prepare_score(&detail, &reading).map_err(anyhow::Error::msg)?;
        let connection =
            Connection::new(&config.browser_url, &saved.cookie, &plan.form.url).await?;
        let result=async {
            let before=team_info(&connection,&plan.team_reference).await?;
            eprintln!("Puntuación actual: {}", before["Wods"].as_array().unwrap().iter().find(|w|w["Id"]==plan.wod_id).map(|w|json!({"id":w["Id"],"score":w["Score"]})).unwrap());
            let persisted = command(&connection.page,json!({"operation":"read-saved","plan":plan})).await.ok();
            if let Some(judge_result) = &persisted {
                eprintln!("Resultado persistido de juez: {judge_result}");
                let cached = detail.categories.iter().find(|c|c.id==plan.category_id).unwrap()
                    .teams.iter().find(|t|t.reference==plan.team_reference).unwrap()
                    .scores.iter().find(|s|s.wod_id==plan.wod_id).unwrap();
                // Puede haber una edición externa posterior a la carga local.
                // La prueba verifica el resultado remoto antes/después sin guardarlo.
                let row = sqlx::query_scalar::<_,sqlx::types::Json<Score>>(
                    "SELECT data FROM competition_scores WHERE username=lower($1) AND competition_id=$2 AND category_id=$3 AND team_reference=$4 AND wod_id=$5")
                    .bind(&saved.username)
                    .bind(plan.competition_id).bind(plan.category_id).bind(&plan.team_reference).bind(plan.wod_id)
                    .fetch_one(&pool).await?;
                assert_eq!(row.0.result,cached.result);
                assert_eq!(row.0.value,cached.value);
            }
            open_editor(&connection.page,&plan).await?;
            for step in &plan.form.steps {
                let state=command(&connection.page,json!({"operation":"step","plan":plan,"step":step})).await?;
                if state=="pending" {wait_for_postback(&connection.page).await?;}
            }
            let diagnostic=format!(r#"(()=>{{const steps={};return steps.map(step=>{{const e=document.getElementById(step.control.id);return {{role:step.role,expected:step.value,actual:e?.value}};}});}})()"#,serde_json::to_string(&plan.form.steps)?);
            let fields=connection.page.evaluate(diagnostic).await.ok().context("No se pudo leer el diagnóstico")?.into_value::<Value>()?;
            eprintln!("Campos antes de Guardar: {}",fields);
            let valid = command(&connection.page,json!({"operation":"validate","plan":plan})).await?;
            ensure!(valid=="ready","La validación real de Arena ha fallado: {valid}");
            let state=command(&connection.page,json!({"operation":"verify","plan":plan})).await?;
            let after=team_info(&connection,&plan.team_reference).await?;
            let scores=|info:&Value| info["Wods"].as_array().unwrap().iter().map(|w|json!({"id":w["Id"],"score":w["Score"],"points":w["Points"]})).collect::<Vec<_>>();
            assert_eq!(scores(&before),scores(&after));
            ensure!(state=="ready","El formulario no coincide antes de Guardar");
            if let Some(persisted) = persisted {
                connection.page.goto(plan.form.url.as_str()).await.ok().context("No se pudo recargar el resultado")?;
                assert_eq!(command(&connection.page,json!({"operation":"read-saved","plan":plan})).await?,persisted);
            }

            Ok(())
        }.await;
        connection.close().await;
        result
    }

    #[tokio::test]
    #[ignore = "Prueba el ejecutor con Browserless sobre un formulario local; no accede a Arena"]
    async fn executes_conditional_fields_and_saves_only_the_selected_wod() -> anyhow::Result<()> {
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
        let (browser, mut handler) = Browser::connect(config.browser_url.as_str())
            .await
            .map_err(|_| anyhow::anyhow!("No se pudo conectar con Browserless"))?;
        let _driver = Driver(tokio::spawn(async move {
            while handler.next().await.is_some() {}
        }));
        let context = browser
            .create_browser_context(CreateBrowserContextParams {
                dispose_on_detach: Some(true),
                ..Default::default()
            })
            .await?;
        let page = browser
            .new_page(
                CreateTargetParams::builder()
                    .url("about:blank")
                    .browser_context_id(context.clone())
                    .build()
                    .unwrap(),
            )
            .await?;
        let result = async {
            page.set_content(r#"
                <h4>Giralda</h4><div id="CtlWodsResults_CtlUpMain"><div id="editor">
                  <select name="wod$complete" id="complete" onchange="postback(this)">
                    <option value="True" selected>Sí</option><option value="False">No</option>
                  </select>
                  <span id="conditional"><input name="wod$time" id="time" value="01:00"></span>
                  <input name="wod$tie" id="tie" value="03:00"
                    oninput="setTimeout(()=>{if(!this.value)this.value='__:__'},0)"
                    onblur="if(this.value==='__:__')this.value=''">
                  <input type="submit" name="wod$CtlSave" id="wod_CtlSave_0" value="Guardar" onclick='WebForm_DoPostBackWithOptions(new WebForm_PostBackOptions("wod$CtlSave", "", true, "selected", "", false, false));saveScore()' >
                </div></div>
                <h4>Otro WOD</h4><div id="CtlWodsResults_Other_CtlUpMain">
                  <input name="other$reps" id="other" value="99">
                  <input type="submit" name="other$save" id="otherSave" onclick="window.otherSaved=true">
                </div>
                <script>
                  window.callbacks=[]; window.busy=false; window.saves=0;
                  window.Page_Validators=[
                    {validationGroup:'selected',controltovalidate:'tie',isvalid:true,errormessage:'Tie-breaker: bad format.'},
                    {validationGroup:'other',isvalid:false,errormessage:'Another WOD is required.'}
                  ];
                  window.Page_ClientValidate=group=>{
                    if(group!=='selected')throw new Error('wrong validation group');
                    const v=Page_Validators[0];
                    v.isvalid=document.getElementById('tie').value==='' || /^\d{2}:\d{2}$/.test(document.getElementById('tie').value);
                    return v.isvalid;
                  };
                  window.WebForm_PostBackOptions=function(){};
                  window.WebForm_DoPostBackWithOptions=function(){};
                  window.Sys={WebForms:{PageRequestManager:{getInstance:()=>({
                    add_endRequest:cb=>callbacks.push(cb),
                    remove_endRequest:cb=>callbacks=callbacks.filter(c=>c!==cb),
                    get_isInAsyncPostBack:()=>busy
                  })}}};
                  function finish(){busy=false; [...callbacks].forEach(cb=>cb(null,{get_error:()=>null}));}
                  function postback(select){
                    busy=true;
                    setTimeout(()=>{
                      conditional.innerHTML=select.value==='False'
                        ? '<input name="wod$reps" id="reps" value="">'
                        : '<input name="wod$time" id="time" value="">';
                      finish();
                    },80);
                  }
                  function saveScore(){
                    busy=true; window.saves++;
                    window.saved={reps:document.getElementById('reps')?.value,tie:document.getElementById('tie').value};
                    setTimeout(()=>{
                      window.editorNode=document.getElementById('editor');editorNode.remove();
                      document.getElementById('CtlWodsResults_CtlUpMain').insertAdjacentHTML('beforeend',
                        '<div id="readonly"><input type="submit" name="wod$CtlEditar" id="wod_CtlEditar_0" value="Edit" onclick="editScore()"><div class="athleteresult"><div class="athlete"><div class="affiliate">2reps</div></div></div></div>');
                      finish();
                    },80);
                  }
                  function editScore(){
                    busy=true;
                    setTimeout(()=>{document.getElementById('readonly').remove();document.getElementById('CtlWodsResults_CtlUpMain').appendChild(editorNode);finish();},80);
                  }
                </script>
            "#).await?;
            let mut plan = PreparedScore {
                version:1, competition_id:1, category_id:2, category_name:"RX".into(), wod_id:3,
                wod_name:"Giralda".into(),team_reference:"4".into(),team_name:"Team".into(),
                registration_id:Some(4),gid:"fixture".into(),
                value:ScoreValue::NotCompleted {repetitions:2,tie_break:None},
                form:ScoreFormPlan {url:"about:blank".into(),save_control:control("save"),steps:vec![
                    FormStep::Select {role:FieldRole::Completed,control:control("complete"),value:"False".into(),wait_for_postback:true},
                    FormStep::Fill {role:FieldRole::Repetitions,control:control("reps"),value:"2".into()},
                    FormStep::Fill {role:FieldRole::TieBreak,control:control("tie"),value:"".into()},
                ]},
            };
            for step in &plan.form.steps {
                let state=command(&page,json!({"operation":"step","plan":plan,"step":step})).await?;
                if state=="pending" {wait_for_postback(&page).await?;}
            }
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"ready");
            assert_eq!(page.evaluate("document.getElementById('tie').value").await?.into_value::<String>()?,"");
            assert_eq!(command(&page,json!({"operation":"validate","plan":plan})).await?,"ready");
            // El marcador vacío de la máscara es válido; un cero real no equivale a vacío.
            page.evaluate("document.getElementById('tie').value='00:00'").await?;
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"mismatch");
            page.evaluate("document.getElementById('tie').value='__:__'").await?;
            assert!(command(&page,json!({"operation":"validate","plan":plan})).await?.starts_with("invalid:"));
            assert!(command(&page,json!({"operation":"save","plan":plan})).await?.starts_with("invalid:"));
            assert_eq!(page.evaluate("window.saves").await?.into_value::<u64>()?,0);
            page.evaluate("document.getElementById('tie').value=''").await?;
            assert_eq!(command(&page,json!({"operation":"validate","plan":plan})).await?,"ready");
            // Un selector ya correcto no debe esperar un postback que nunca ocurrirá.
            assert_eq!(command(&page,json!({"operation":"step","plan":plan,"step":plan.form.steps[0]})).await?,"ready");
            command(&page,json!({"operation":"save","plan":plan})).await?;
            wait_for_postback(&page).await?;
            let saved=page.evaluate("({saves, saved, otherSaved:!!window.otherSaved,other:document.getElementById('other').value})")
                .await?.into_value::<Value>()?;
            assert_eq!(saved,json!({"saves":1,"saved":{"reps":"2","tie":""},"otherSaved":false,"other":"99"}));
            assert_eq!(command(&page,json!({"operation":"read-saved","plan":plan})).await?,"saved:2reps");
            assert!(command(&page,json!({"operation":"verify","plan":plan})).await.is_err());
            open_editor(&page,&plan).await?;
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"ready");
            assert_eq!(page.evaluate("window.saves").await?.into_value::<u64>()?,1);
            let foreign=FormStep::Fill {role:FieldRole::Repetitions,control:ControlReference {name:"other$reps".into(),id:"other".into()},value:"0".into()};
            assert!(command(&page,json!({"operation":"step","plan":plan,"step":foreign})).await.is_err());
            // Rama de tiempo: el postback sustituye de nuevo el input anterior.
            plan.form.steps=vec![
                FormStep::Select {role:FieldRole::Completed,control:control("complete"),value:"True".into(),wait_for_postback:true},
                FormStep::Fill {role:FieldRole::Time,control:control("time"),value:"07:20".into()},
                FormStep::Fill {role:FieldRole::TieBreak,control:control("tie"),value:"03:05".into()},
            ];
            for step in &plan.form.steps {
                let state=command(&page,json!({"operation":"step","plan":plan,"step":step})).await?;
                if state=="pending" {wait_for_postback(&page).await?;}
            }
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"ready");
            // Peso: selecciona kg y conserva el decimal; rechaza opciones inexistentes.
            page.evaluate("document.getElementById('time').value='__:__'").await?;
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"mismatch");
            page.evaluate(r#"(()=>{document.getElementById('conditional').innerHTML='<select name="wod$unit" id="unit"><option value="20" selected>Lb</option><option value="10">Kg</option></select><input type="number" step="0.1" name="wod$weight" id="weight">';return true;})()"#).await?;
            plan.form.steps=vec![
                FormStep::Select {role:FieldRole::WeightUnit,control:control("unit"),value:"10".into(),wait_for_postback:false},
                FormStep::Fill {role:FieldRole::Weight,control:control("weight"),value:"82.5".into()},
            ];
            for step in &plan.form.steps {
                command(&page,json!({"operation":"step","plan":plan,"step":step})).await?;
            }
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"ready");
            let invalid=FormStep::Select {role:FieldRole::WeightUnit,control:control("unit"),value:"100".into(),wait_for_postback:false};
            assert!(command(&page,json!({"operation":"step","plan":plan,"step":invalid})).await.is_err());
            page.evaluate("document.getElementById('weight').value='12'").await?;
            assert_eq!(command(&page,json!({"operation":"verify","plan":plan})).await?,"mismatch");
            plan.wod_name="Otro WOD".into();
            assert!(command(&page,json!({"operation":"check","plan":plan})).await.is_err());
            Ok(())
        }.await;
        browser.dispose_browser_context(context).await?;
        result
    }
}
