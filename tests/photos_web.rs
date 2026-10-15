use cookie::{CookieJar, Key};
use reqwest::{
    Client,
    multipart::{Form, Part},
    redirect::Policy,
};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    AppState,
    browser::{ArenaBrowser, AuthCookie},
    competitions::Competition,
    storage::SessionStore,
};
mod support;

fn editable_fixture() -> wodbuster_jueces::details::CompetitionDetail {
    let mut detail = support::fixture();
    let fields = serde_json::json!([
        {"attributes":{"name":"panel$IoComplete","id":"complete"},"options":[{"value":"True","label":"Sí"},{"value":"False","label":"No"}]},
        {"attributes":{"name":"panel$IoTimeMMSS","id":"time","required":""}},
        {"attributes":{"name":"panel$IoReps","id":"reps","required":""}},
        {"attributes":{"name":"panel$IoDesempate","id":"tie"}}
    ]);
    detail.categories[0].wods[0].score_format =
        wodbuster_jueces::scoring::ScoreFormat::from_fields(&fields);
    detail.categories[0].teams[0].scores[0].metadata =
        serde_json::json!({"fields":fields,"save_controls":[{"name":"panel$CtlSave","id":"save"}]});
    detail.categories[0].teams[0].scores[0].result = Some("2reps".into());
    detail
}

#[sqlx::test]
#[ignore = "Requiere Postgres y Browserless de compose.dev.yml"]
async fn uploads_are_private_idempotent_and_gallery_shows_extraction_warnings(
    pool: PgPool,
) -> topcoat::Result<()> {
    let secret = "test-session-secret-with-at-least-32-bytes";
    let store = std::sync::Arc::new(SessionStore::new(pool.clone(), secret));
    let session = Uuid::new_v4();
    let key = Key::derive_from(secret.as_bytes());
    store
        .save(
            session,
            "judge@example.com",
            &AuthCookie {
                name: ".WBAuth".into(),
                value: "private-cookie".into(),
                domain: ".wodbuster.com".into(),
                path: "/".into(),
                expires: -1.,
                http_only: true,
                secure: true,
                session: true,
                same_site: None,
            },
        )
        .await?;
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Test")])
        .await?;
    store
        .save_competition_detail("judge@example.com", &editable_fixture())
        .await?;
    let state = AppState {
        store: store.clone(),
        browser: ArenaBrowser::new("ws://127.0.0.1:1".parse().unwrap()).into(),
        cookie_key: key.clone(),
        secure_cookies: false,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(topcoat::serve(
        listener,
        wodbuster_jueces::web::router(state),
    ));
    let result=async {
        let client=Client::builder().redirect(Policy::none()).build()?;
        let mut jar=CookieJar::new();
        jar.signed_mut(&key).add(cookie::Cookie::new("wb_session",session.to_string()));
        jar.signed_mut(&key).add(cookie::Cookie::new("wb_csrf","csrf-photo-test"));
        let headers=format!("wb_session={}; wb_csrf={}",jar.get("wb_session").unwrap().value(),jar.get("wb_csrf").unwrap().value());
        let detail=client.get(format!("{base}/competitions/3910")).header("Cookie",&headers).send().await?.text().await?;
        assert!(detail.contains("Tomar foto")&&detail.contains("capture=\"environment\"")&&detail.contains("Ver fotos"));
        assert!(detail.contains("Fotos sin categorizar o con problemas")&&detail.contains("issues=true&amp;page=0"));
        assert!(detail.contains("Comprobar sincronización")&&detail.contains("/competitions/3910/check-sync"));
        assert!(detail.contains("Ver fotos no sincronizadas")&&detail.contains("unsynced=true&amp;page=0"));
        for label in ["Ver fotos por categoría", "Ver fotos por WOD", "Ver fotos por equipo"] { assert!(detail.contains(label)); }
        for query in ["category_id=16907&amp;page=0", "category_id=16907&amp;wod_id=64459&amp;page=0", "category_id=16907&amp;team_reference=298682&amp;page=0"] { assert!(detail.contains(query), "{query}"); }
        let image=include_bytes!("../examples/poc_agent.png").to_vec();
        let photo_id=Uuid::new_v4();
        let upload=|csrf:&str,id:Uuid|Form::new().text("csrf",csrf.to_string()).text("capture_id",id.to_string()).part("photo",Part::bytes(image.clone()).file_name("photo.png").mime_str("image/png").unwrap());
        let denied=client.post(format!("{base}/competitions/3910/photos")).header("Cookie",&headers).multipart(upload("forged",photo_id)).send().await?;
        assert_eq!(denied.status(),403);
        for _ in 0..2 {
            let response=client.post(format!("{base}/competitions/3910/photos")).header("Cookie",&headers).multipart(upload("csrf-photo-test",photo_id)).send().await?;
            assert_eq!(response.status(),200);
        }
        let photos=store.photos("judge@example.com",3910,0).await?;
        assert_eq!(photos.len(),1);assert_eq!(photos[0].state,"queued");
        assert_eq!(store.photo_image("judge@example.com",3910,photo_id).await?.unwrap().0,image);
        let binary=format!("{base}/competitions/3910/photos/{photo_id}/image");
        let delete_url=format!("{base}/competitions/3910/photos/{photo_id}/delete");
        assert_eq!(client.get(&binary).send().await?.status(),401);
        let response=client.get(&binary).header("Cookie",&headers).send().await?;
        assert_eq!(response.headers()["content-type"],"image/png");
        assert_eq!(response.headers()["cache-control"],"no-store");
        let reading=serde_json::json!({"category_id":16907,"wod_id":64459,"team_reference":"298682","score_value":{"kind":"not_completed","repetitions":2,"tie_break":null},"needs_retake":true,"retake_reason":"Revisar imagen"});
        sqlx::query("UPDATE score_photos SET reading=$2,prepared=$3,state='succeeded' WHERE id=$1").bind(photo_id).bind(sqlx::types::Json(&reading)).bind(sqlx::types::Json(serde_json::json!({"team_name":"Team <unsafe>"}))).execute(&pool).await?;
        let html=client.get(format!("{base}/competitions/3910/photos")).header("Cookie",&headers).send().await?.text().await?;
        assert!(html.contains("Campos extraídos")&&html.contains("Guardada y verificada"));
        assert!(html.contains("photo-fields")&&html.contains("Guardar cambios y encolar")&&html.contains("data-photo-refresh"));
        assert!(html.contains("data-photo-delete-dialog")&&html.contains("Borrar foto definitivamente")&&html.contains("Sincronizada con Wodbuster"));
        assert!(!html.contains("<pre"));
        assert!(html.contains("Team &lt;unsafe&gt;")&&!html.contains("private-cookie"));
        assert!(html.contains("role=\"alert\">Aviso: la puntuación se ha guardado"));
        let status:serde_json::Value=serde_json::from_str(&client.get(format!("{base}/competitions/3910/photos/status")).header("Cookie",&headers).send().await?.text().await?)?;
        assert_eq!(status[0]["warning"],true);assert_eq!(status[0]["reading"],reading);
        assert!(status[0].get("session_id").is_none());
        let sync_url=format!("{base}/competitions/3910/photos/status?unsynced=true");
        let synced:serde_json::Value=serde_json::from_str(&client.get(&sync_url).header("Cookie",&headers).send().await?.text().await?)?;
        assert!(synced.as_array().unwrap().is_empty());
        sqlx::query("UPDATE score_photos SET reading=jsonb_set(reading,'{score_value,repetitions}','3') WHERE id=$1").bind(photo_id).execute(&pool).await?;
        let mismatch:serde_json::Value=serde_json::from_str(&client.get(&sync_url).header("Cookie",&headers).send().await?.text().await?)?;
        assert_eq!(mismatch.as_array().unwrap().len(),1);assert_eq!(mismatch[0]["id"],photo_id.to_string());
        let mismatched_html=client.get(format!("{base}/competitions/3910/photos?unsynced=true")).header("Cookie",&headers).send().await?.text().await?;
        assert!(mismatched_html.contains("Fotos no sincronizadas con Wodbuster")&&mismatched_html.contains(&photo_id.to_string()));
        assert!(mismatched_html.contains("/photos/status?unsynced=true&amp;page=0")&&mismatched_html.contains("data-photo-filtered=\"true\""));
        sqlx::query("UPDATE score_photos SET reading=$2 WHERE id=$1").bind(photo_id).bind(sqlx::types::Json(&reading)).execute(&pool).await?;
        for query in ["category_id=16907", "category_id=16907&wod_id=64459", "category_id=16907&team_reference=298682", "issues=true"] {
            let filtered_html=client.get(format!("{base}/competitions/3910/photos?{query}")).header("Cookie",&headers).send().await?.text().await?;
            assert!(filtered_html.contains("data-photo-filtered=\"true\"")&&filtered_html.contains(&photo_id.to_string()));
            assert!(filtered_html.contains("Ver todas las fotos"));
            let response=client.get(format!("{base}/competitions/3910/photos/status?{query}")).header("Cookie",&headers).send().await?;
            assert_eq!(response.status(),200);
            let filtered:serde_json::Value=serde_json::from_str(&response.text().await?)?;
            assert_eq!(filtered.as_array().unwrap().len(),1);assert_eq!(filtered[0]["id"],photo_id.to_string());
        }
        // Una foto guardada sin avisos sale de la vista de problemas.
        sqlx::query("UPDATE score_photos SET reading=jsonb_set(reading,'{needs_retake}','false') WHERE id=$1").bind(photo_id).execute(&pool).await?;
        let resolved:serde_json::Value=serde_json::from_str(&client.get(format!("{base}/competitions/3910/photos/status?issues=true")).header("Cookie",&headers).send().await?.text().await?)?;
        assert!(resolved.as_array().unwrap().is_empty());
        let empty=client.get(format!("{base}/competitions/3910/photos?issues=true")).header("Cookie",&headers).send().await?.text().await?;
        assert!(!empty.contains(&format!("data-photo-id=\"{photo_id}\"")));
        sqlx::query("UPDATE score_photos SET reading=$2 WHERE id=$1").bind(photo_id).bind(sqlx::types::Json(&reading)).execute(&pool).await?;
        for (query,code) in [("wod_id=64459",400),("team_reference=298682",400),("category_id=1",404),("category_id=16907&wod_id=1",404),("category_id=16907&team_reference=unknown",404)] {
            for suffix in ["photos","photos/status"] {
                assert_eq!(client.get(format!("{base}/competitions/3910/{suffix}?{query}")).header("Cookie",&headers).send().await?.status(),code);
            }
        }
        browser_editor_checks(&html,&status,&detail).await.map_err(|e|topcoat::Error::msg(e.to_string()))?;
        let edit_url=format!("{base}/competitions/3910/photos/{photo_id}/edit");
        let refresh_url=format!("{base}/competitions/3910/photos/{photo_id}/refresh");
        let edit=vec![("csrf","csrf-photo-test"),("revision","0"),("category_id","16907"),("wod_id","64459"),("team_reference","298682"),("kind","not_completed"),("repetitions","5"),("needs_retake","true"),("retake_reason","Revisión manual")];
        let post_edit=|values:Vec<(&str,&str)>|client.post(&edit_url).header("Cookie",&headers).form(&values).send();
        let mut forged=edit.clone();forged[0].1="forged";
        assert_eq!(post_edit(forged).await?.status(),403);
        assert_eq!(client.get(&refresh_url).header("Cookie",&headers).send().await?.status(),405);
        for (name,value) in [("category_id","99999"),("team_reference","not-my-team"),("wod_id","1"),("kind","weight"),("repetitions","-1"),("repetitions","4294967296")] {
            let mut invalid=edit.clone();invalid.iter_mut().find(|(n,_)|*n==name).unwrap().1=value;
            assert_eq!(post_edit(invalid).await?.status(),400,"{name}={value}");
        }
        let mut invalid_time=edit.clone();invalid_time.iter_mut().find(|(n,_)|*n=="kind").unwrap().1="completed";invalid_time.push(("time","05:60"));
        assert_eq!(post_edit(invalid_time).await?.status(),400);
        assert_eq!(store.photo("judge@example.com",3910,photo_id).await?.unwrap().state,"succeeded");
        let (one,two)=tokio::join!(post_edit(edit.clone()),post_edit(edit.clone()));
        let mut codes=vec![one?.status().as_u16(),two?.status().as_u16()];codes.sort();assert_eq!(codes,[200,409]);
        let manual=store.photo("judge@example.com",3910,photo_id).await?.unwrap();
        assert_eq!(manual.state,"parsed");assert_eq!(manual.revision,1);
        assert_eq!(manual.reading.as_ref().unwrap()["score_value"]["repetitions"],5);
        assert_eq!(manual.prepared.as_ref().unwrap()["value"]["repetitions"],5);
        assert_eq!(manual.model,wodbuster_jueces::ai::model_name());assert_eq!(manual.provider,wodbuster_jueces::ai::PROVIDER);assert!(!manual.warning());
        assert_eq!(post_edit(edit.clone()).await?.status(),409);
        assert_eq!(client.post(&refresh_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test"),("revision","1")]).send().await?.status(),409);
        // Simula que el worker terminó: una acción explícita puede volver a encolarlo.
        sqlx::query("UPDATE score_photos SET state='failed',error='Previous error' WHERE id=$1").bind(photo_id).execute(&pool).await?;
        assert_eq!(post_edit(edit.clone()).await?.status(),409); // pestaña antigua
        assert_eq!(client.post(&refresh_url).header("Cookie",&headers).form(&[("csrf","forged"),("revision","1")]).send().await?.status(),403);
        assert_eq!(client.post(&refresh_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test"),("revision","1")]).send().await?.status(),200);
        let retry=store.photo("judge@example.com",3910,photo_id).await?.unwrap();
        assert_eq!(retry.state,"queued");assert_eq!(retry.revision,2);assert!(retry.reading.is_none()&&retry.prepared.is_none()&&retry.error.is_none());
        assert_eq!(client.get(&delete_url).header("Cookie",&headers).send().await?.status(),405);
        assert_eq!(client.post(&delete_url).form(&[("csrf","csrf-photo-test"),("revision","2")]).send().await?.status(),401);
        assert_eq!(client.post(&delete_url).header("Cookie",&headers).form(&[("csrf","forged"),("revision","2")]).send().await?.status(),403);
        assert_eq!(client.post(&delete_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test"),("revision","2")]).send().await?.status(),409);
        assert_eq!(store.photos("judge@example.com",3910,0).await?.len(),1);
        assert_eq!(store.photo_image("judge@example.com",3910,photo_id).await?.unwrap().0,image);
        let other=Uuid::new_v4();
        store.save(other,"other@example.com",&AuthCookie{name:".WBAuth".into(),value:"other-cookie".into(),domain:".wodbuster.com".into(),path:"/".into(),expires:-1.,http_only:true,secure:true,session:true,same_site:None}).await?;
        jar.signed_mut(&key).add(cookie::Cookie::new("wb_session",other.to_string()));
        let foreign=format!("wb_session={}; wb_csrf={}",jar.get("wb_session").unwrap().value(),jar.get("wb_csrf").unwrap().value());
        assert_eq!(client.get(&binary).header("Cookie",&foreign).send().await?.status(),404);
        assert_eq!(client.post(&edit_url).header("Cookie",&foreign).form(&edit).send().await?.status(),404);
        assert_eq!(client.post(&refresh_url).header("Cookie",&foreign).form(&[("csrf","csrf-photo-test"),("revision","2")]).send().await?.status(),404);
        assert_eq!(client.post(&delete_url).header("Cookie",&foreign).form(&[("csrf","csrf-photo-test"),("revision","2")]).send().await?.status(),404);
        let invalid=Form::new().text("csrf","csrf-photo-test").text("capture_id",Uuid::new_v4().to_string()).part("photo",Part::bytes(b"not a photo".to_vec()).file_name("photo.jpg"));
        assert_eq!(client.post(format!("{base}/competitions/3910/photos")).header("Cookie",&headers).multipart(invalid).send().await?.status(),400);
        sqlx::query("UPDATE score_photos SET state='succeeded' WHERE id=$1").bind(photo_id).execute(&pool).await?;
        let snapshot_before=store.competition_detail("judge@example.com",3910).await?;
        assert_eq!(client.post(&delete_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test"),("revision","1")]).send().await?.status(),409);
        assert_eq!(client.post(&delete_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test"),("revision","2")]).send().await?.status(),200);
        assert!(store.photo_image("judge@example.com",3910,photo_id).await?.is_none());
        assert_eq!(store.competition_detail("judge@example.com",3910).await?,snapshot_before);
        // El botón consulta Arena mediante el trabajo asíncrono, protegido y deduplicado.
        let sync_url=format!("{base}/competitions/3910/check-sync");
        assert_eq!(client.post(&sync_url).header("Cookie",&headers).form(&[("csrf","forged")]).send().await?.status(),403);
        let (job,_)=store.begin_load("judge@example.com",Some(3910)).await?;
        let check=client.post(&sync_url).header("Cookie",&headers).form(&[("csrf","csrf-photo-test")]).send().await?;
        assert_eq!(check.status(),303);assert!(check.headers()["location"].to_str().unwrap().ends_with(&job.id.to_string()));
        Ok::<_,topcoat::Error>(())
    }.await;
    server.abort();
    result
}

/// Ejecuta el JavaScript real sobre el HTML real de la galería, con transporte simulado.
/// No llama a la IA ni guarda puntuaciones en Arena.
async fn browser_editor_checks(
    html: &str,
    photos: &serde_json::Value,
    detail_html: &str,
) -> anyhow::Result<()> {
    use chromiumoxide::{
        Browser,
        cdp::browser_protocol::target::{CreateBrowserContextParams, CreateTargetParams},
    };
    use futures_util::StreamExt;
    let config =
        wodbuster_jueces::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
    let (browser, mut handler) = Browser::connect(config.browser_url.as_str())
        .await
        .map_err(|_| anyhow::anyhow!("Browserless no disponible"))?;
    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });
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
    let result=async {
        page.set_content(html).await?;
        page.evaluate(format!("window.testPhotos={photos};")).await?;
        page.evaluate(r#"(()=>{
            window.testCalls=[];
            document.querySelectorAll('form[action]').forEach(form=>{form.action='http://127.0.0.1'+form.getAttribute('action');});
            window.setTimeout=(callback)=>{window.testPoll=callback;return 1;};
            window.fetch=async(url,options={})=>{
                if (options.method==='POST') {
                    const values=Object.fromEntries(options.body);
                    testCalls.push({url,values});
                    if (window.testHold) await new Promise(resolve=>{window.releaseRefresh=resolve;});
                    if (window.testReject) return {ok:false,status:409,json:async()=>({message:'Conflicto: recarga la página.'})};
                    if(url.endsWith('/delete')) {testPhotos=[];return {ok:true,json:async()=>({deleted:true,message:'Foto borrada. La puntuación de Wodbuster se conserva.'})};}
                    const photo=testPhotos[0];photo.revision++;
                    if(url.endsWith('/edit')) {photo.reading.score_value={kind:values.kind,repetitions:Number(values.repetitions),tie_break:null};photo.state='parsed';}
                    else {photo.reading=null;photo.state='queued';}
                    photo.busy=true;photo.warning=false;photo.label='En cola';
                    return {ok:true,json:async()=>({queued:true,message:'Encolado correctamente'})};
                }
                if(window.testOffline) throw new TypeError('Failed to fetch');
                const progress={queued:'Foto en cola',parsing:'Analizando la imagen con IA…',parsed:'Puntuación en cola para guardar',submitting:'Guardando y verificando la puntuación en Wodbuster…',succeeded:'Proceso completado. Puntuación guardada y verificada en Wodbuster.',failed:'El procesamiento ha fallado.',incomplete:'Análisis terminado: faltan datos.'};
                return {ok:true,json:async()=>testPhotos.map(photo=>({...structuredClone(photo),progress:progress[photo.state]}))};
            };
        })()"#).await?;
        page.evaluate(include_str!("../src/web/photos.js")).await?;
        let initial:serde_json::Value=page.evaluate("(()=>{const f=document.querySelector('[data-photo-edit]');return {category:f.elements.category_id.value,team:f.elements.team_reference.value,kind:f.elements.kind.value,reps:f.elements.repetitions.value,hidden:document.querySelector('[data-score-field=time]').hidden};})()").await?.into_value()?;
        assert_eq!(initial["category"],"16907");assert_eq!(initial["team"],"298682");
        assert_eq!(initial["kind"],"not_completed");assert_eq!(initial["reps"],"2");assert_eq!(initial["hidden"],true);
        let readonly:serde_json::Value=page.evaluate("({value:document.querySelector('[data-field-value=repetitions]').textContent,closed:document.querySelector('[data-field-control=repetitions]').hidden,saveDisabled:document.querySelector('[data-photo-save]').disabled,buttons:document.querySelectorAll('[data-field-edit]').length})").await?.into_value()?;
        assert_eq!(readonly["value"],"2");assert_eq!(readonly["closed"],true);assert_eq!(readonly["saveDisabled"],true);assert_eq!(readonly["buttons"],10);
        // Abrir y cancelar el modal no envía ninguna petición de borrado.
        page.evaluate("document.querySelector('[data-photo-delete-open]').click()").await?;
        assert!(page.evaluate("document.querySelector('[data-photo-delete-dialog]').open && testCalls.length===0").await?.into_value::<bool>()?);
        save_test_screenshot(&page,"photo-delete-modal.png").await?;
        page.evaluate("document.querySelector('[data-photo-delete-cancel]').click()").await?;
        assert!(page.evaluate("!document.querySelector('[data-photo-delete-dialog]').open && testCalls.length===0").await?.into_value::<bool>()?);
        page.evaluate("document.querySelector('[data-field-edit=repetitions]').click()").await?;
        assert!(!page.evaluate("document.querySelector('[data-field-control=repetitions]').hidden").await?.into_value::<bool>()?);
        assert!(page.evaluate("document.querySelector('[data-photo-save]').disabled").await?.into_value::<bool>()?);
        // Cambiar y deshacer activa/desactiva Guardar y marca únicamente la fila alterada.
        page.evaluate("document.querySelector('[name=repetitions]').value='7';document.querySelector('[name=repetitions]').dispatchEvent(new Event('input',{bubbles:true}));").await?;
        let changed:serde_json::Value=page.evaluate("({saveDisabled:document.querySelector('[data-photo-save]').disabled,rows:document.querySelectorAll('tr.is-modified').length,changed:document.querySelector('tr.is-modified').dataset.photoField,color:getComputedStyle(document.querySelector('tr.is-modified td')).backgroundColor})").await?.into_value()?;
        assert_eq!(changed["saveDisabled"],false);assert_eq!(changed["rows"],1);assert_eq!(changed["changed"],"repetitions");assert_eq!(changed["color"],"rgb(255, 240, 217)");
        page.evaluate("document.querySelector('[name=repetitions]').value='2';document.querySelector('[name=repetitions]').dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('[data-photo-edit]').requestSubmit()").await?;
        assert!(page.evaluate("document.querySelector('[data-photo-save]').disabled && !document.querySelector('tr.is-modified') && testCalls.length===0").await?.into_value::<bool>()?);
        page.evaluate("document.querySelector('[name=repetitions]').value='7';document.querySelector('[name=repetitions]').dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('[data-field-edit=repetitions]').click();testPhotos[0].reading.score_value.repetitions=9;").await?;
        assert!(page.evaluate("document.querySelector('[data-field-control=repetitions]').hidden && document.querySelector('[data-field-value=repetitions]').textContent==='7'").await?.into_value::<bool>()?);
        page.evaluate("testPoll()").await?;
        assert_eq!(page.evaluate("document.querySelector('[name=repetitions]').value").await?.into_value::<String>()?,"7");
        // El refresco no borra una corrección aún sin guardar.
        page.evaluate("document.querySelector('[data-photo-refresh]').requestSubmit()").await?;
        assert_eq!(page.evaluate("testCalls.length").await?.into_value::<u32>()?,0);
        page.evaluate("window.testReject=true;document.querySelector('[data-photo-edit]').requestSubmit()").await?;
        for _ in 0..30 {if page.evaluate("document.querySelector('[data-photo-action-message]').textContent.includes('Conflicto')").await?.into_value::<bool>()? {break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        assert_eq!(page.evaluate("document.querySelector('[name=repetitions]').value").await?.into_value::<String>()?,"7");
        page.evaluate("window.testReject=false;document.querySelector('[data-photo-edit]').requestSubmit()").await?;
        for _ in 0..30 {if page.evaluate("testCalls.length===2 && document.querySelector('[name=revision]').value==='1'").await?.into_value::<bool>()? {break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        let edited:serde_json::Value=page.evaluate("({calls:testCalls,locked:document.querySelector('fieldset').disabled,revision:document.querySelector('[name=revision]').value})").await?.into_value()?;
        assert_eq!(edited["calls"][1]["values"]["repetitions"],"7");assert_eq!(edited["calls"][1]["values"]["revision"],"0");
        assert_eq!(edited["calls"][1]["values"]["csrf"],"csrf-photo-test");assert_eq!(edited["locked"],true);assert_eq!(edited["revision"],"1");
        assert!(page.evaluate("document.querySelector('[data-photo-save]').disabled && !document.querySelector('tr.is-modified') && document.querySelector('[data-field-control=repetitions]').hidden").await?.into_value::<bool>()?);
        // El feedback se muestra junto al encabezado incluso antes de la respuesta del servidor.
        page.evaluate("testPhotos[0].busy=false;testPhotos[0].state='succeeded';testPhotos[0].label='Guardada y verificada';").await?;
        page.evaluate("testPoll()").await?;
        page.evaluate("window.testHold=true;document.querySelector('[data-photo-refresh]').requestSubmit()").await?;
        let pending:serde_json::Value=page.evaluate("({visible:!document.querySelector('[data-photo-action-message]').hidden,message:document.querySelector('[data-photo-action-message]').textContent,heading:document.querySelector('[data-photo-state]').textContent,aboveFields:!!(document.querySelector('[data-photo-action-message]').compareDocumentPosition(document.querySelector('[data-photo-edit]'))&Node.DOCUMENT_POSITION_FOLLOWING),spinner:getComputedStyle(document.querySelector('[data-photo-refresh-icon]')).animationName,locked:document.querySelector('[data-photo-refresh] button').disabled})").await?.into_value()?;
        assert_eq!(pending["visible"],true);assert!(pending["message"].as_str().unwrap().contains("Solicitando nuevo análisis"));
        assert_eq!(pending["heading"],"Solicitando nuevo análisis…");assert_eq!(pending["aboveFields"],true);assert_eq!(pending["spinner"],"photo-refresh-spin");assert_eq!(pending["locked"],true);
        page.evaluate("window.testHold=false;releaseRefresh()").await?;
        for _ in 0..30 {if page.evaluate("testCalls.length===3 && document.querySelector('[name=revision]').value==='2'").await?.into_value::<bool>()? {break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        let refreshed:serde_json::Value=page.evaluate("({call:testCalls[2],locked:document.querySelector('fieldset').disabled,kind:document.querySelector('[name=kind]').value,revision:document.querySelector('[name=revision]').value})").await?.into_value()?;
        assert!(refreshed["call"]["url"].as_str().unwrap().ends_with("/refresh"), "{refreshed}");assert_eq!(refreshed["call"]["values"]["revision"],"1");
        assert_eq!(refreshed["locked"],true);assert_eq!(refreshed["kind"],"");assert_eq!(refreshed["revision"],"2");
        assert_eq!(page.evaluate("document.querySelector('[data-photo-action-message]').textContent").await?.into_value::<String>()?,"Foto en cola");
        for (phase,expected) in [("parsing","Analizando la imagen"),("parsed","Puntuación en cola"),("submitting","Guardando y verificando")] {
            page.evaluate(format!("testPhotos[0].state='{phase}';")).await?;
            page.evaluate("testPoll()").await?;
            let progress=page.evaluate("document.querySelector('[data-photo-action-message]').textContent").await?.into_value::<String>()?;
            assert!(progress.contains(expected),"{progress}");
            assert!(page.evaluate("document.querySelector('[data-photo-refresh-icon]').classList.contains('is-processing')").await?.into_value::<bool>()?);
        }
        page.evaluate("window.testOffline=true").await?;
        page.evaluate("testPoll()").await?;
        assert!(page.evaluate("document.querySelector('[data-photo-action-message]').textContent.includes('perdido la conexión') && document.querySelector('[data-photo-refresh] button').disabled").await?.into_value::<bool>()?);
        page.evaluate("window.testOffline=false;testPhotos[0].busy=false;testPhotos[0].state='succeeded';testPhotos[0].label='Guardada y verificada';").await?;
        page.evaluate("testPoll()").await?;
        assert!(page.evaluate("document.querySelector('[data-photo-action-message]').classList.contains('success') && document.querySelector('[data-photo-action-message]').textContent.includes('Proceso completado') && !document.querySelector('[data-photo-refresh] button').disabled && !document.querySelector('[data-photo-refresh-icon]').classList.contains('is-processing')").await?.into_value::<bool>()?);
        // Un fallo al encolar también es visible y permite reintentar.
        page.evaluate("window.testReject=true;document.querySelector('[data-photo-refresh]').requestSubmit()").await?;
        for _ in 0..30 {if page.evaluate("document.querySelector('[data-photo-action-message]').textContent.includes('Conflicto')").await?.into_value::<bool>()? {break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        assert!(page.evaluate("document.querySelector('[data-photo-action-message]').classList.contains('error') && !document.querySelector('[data-photo-refresh] button').disabled && !document.querySelector('[data-photo-refresh-icon]').classList.contains('is-processing')").await?.into_value::<bool>()?);
        page.evaluate("window.testReject=false;testPhotos[0].state='failed';testPhotos[0].label='Error';testPhotos[0].error='Provider unavailable'").await?;
        page.evaluate("testPoll()").await?;
        assert!(page.evaluate("document.querySelector('[data-photo-action-message]').textContent.includes('ha fallado') && !document.querySelector('[data-photo-error]').hidden").await?.into_value::<bool>()?);
        // Completado cambia los campos visibles y exige mm:ss, sin repeticiones.
        page.evaluate("testPhotos[0].busy=false;testPhotos[0].state='succeeded';testPhotos[0].reading={category_id:16907,wod_id:64459,team_reference:'298682',score_value:{kind:'completed',time:{minutes:5,seconds:32},tie_break:null},needs_retake:false};").await?;
        page.evaluate("testPoll()").await?;
        let completed:serde_json::Value=page.evaluate("({time:document.querySelector('[name=time]').value,timeRequired:document.querySelector('[name=time]').required,repsDisabled:document.querySelector('[name=repetitions]').disabled})").await?.into_value()?;
        assert_eq!(completed["time"],"05:32");assert_eq!(completed["timeRequired"],true);assert_eq!(completed["repsDisabled"],true);
        // Una reasignación desaparece del filtro sin perder una edición pendiente.
        page.evaluate("document.querySelector('[data-photo-gallery]').dataset.photoFiltered='true';document.querySelector('[name=time]').value='06:00';document.querySelector('[name=time]').dispatchEvent(new Event('input',{bubbles:true}));testPhotos=[];").await?;
        page.evaluate("testPoll()").await?;
        assert!(page.evaluate("!!document.querySelector('[data-photo-id]') && !document.querySelector('[data-photo-list-changed]').hidden").await?.into_value::<bool>()?);
        page.evaluate("document.querySelector('[name=time]').value='05:32';document.querySelector('[name=time]').dispatchEvent(new Event('input',{bubbles:true}));").await?;
        page.evaluate("testPoll()").await?;
        assert!(page.evaluate("!document.querySelector('[data-photo-id]') && !document.querySelector('[data-photo-empty]').hidden").await?.into_value::<bool>()?);
        // Render nuevo: solo confirmar dentro del modal elimina la foto.
        page.set_content(html).await?;
        page.evaluate(format!("window.testPhotos={photos};window.testCalls=[];window.testReject=false;")).await?;
        page.evaluate("document.querySelectorAll('form[action]').forEach(form=>{form.action='http://127.0.0.1'+form.getAttribute('action');})").await?;
        page.evaluate(include_str!("../src/web/photos.js")).await?;
        page.evaluate("document.querySelector('[data-photo-delete]').requestSubmit()").await?;
        assert_eq!(page.evaluate("testCalls.length").await?.into_value::<u32>()?,0);
        page.evaluate("document.querySelector('[data-photo-delete-open]').click();document.querySelector('[data-photo-delete]').requestSubmit()").await?;
        for _ in 0..30 {if page.evaluate("!document.querySelector('[data-photo-id]')").await?.into_value::<bool>()? {break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        assert!(page.evaluate("testCalls.length===1 && testCalls[0].url.endsWith('/delete') && !document.querySelector('[data-photo-id]') && !document.querySelector('[data-photo-list-feedback]').hidden").await?.into_value::<bool>()?);
        page.set_content(detail_html).await?;
        assert!(page.evaluate("!!document.querySelector('.photo-tools button') && document.querySelector('.photo-tools button').textContent==='Comprobar sincronización'").await?.into_value::<bool>()?);
        save_test_screenshot(&page,"competition-photo-actions.png").await?;
        Ok::<_,anyhow::Error>(())
    }.await;
    let _ = browser.dispose_browser_context(context).await;
    driver.abort();
    result
}

async fn save_test_screenshot(page: &chromiumoxide::Page, name: &str) -> anyhow::Result<()> {
    if let Some(directory) = std::env::var_os("WODBUSTER_TEST_SCREENSHOTS") {
        std::fs::create_dir_all(&directory)?;
        page.save_screenshot(
            chromiumoxide::page::ScreenshotParams::builder()
                .full_page(true)
                .build(),
            std::path::PathBuf::from(directory).join(name),
        )
        .await?;
    }
    Ok(())
}
