//! Prueba optativa: sube la imagen del PoC y guarda realmente su lectura en Arena.
use cookie::CookieJar;
use reqwest::{
    Client,
    multipart::{Form, Part},
};
use uuid::Uuid;
use wodbuster_jueces::{AppState, config::Config};

#[tokio::test]
#[ignore = "Requiere una sesión de ChatGPT, Postgres, Browserless y una sesión de Arena; envía la puntuación de examples/poc_agent.png"]
async fn photo_upload_is_parsed_and_submitted_by_the_single_worker() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
    let pool = sqlx::PgPool::connect(&config.database_url).await?;
    let id=sqlx::query_scalar::<_,Uuid>("SELECT s.id FROM wodbuster_sessions s JOIN competition_details d ON d.username=lower(s.username) WHERE d.competition_id=3910 ORDER BY s.saved_at DESC LIMIT 1")
        .fetch_one(&pool).await?;
    let state = AppState::connect(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let saved = state
        .store
        .load(id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("Falta una sesión válida"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(topcoat::serve(
        listener,
        wodbuster_jueces::web::router(state.clone()),
    ));
    let mut jar = CookieJar::new();
    jar.signed_mut(&state.cookie_key)
        .add(cookie::Cookie::new("wb_session", id.to_string()));
    jar.signed_mut(&state.cookie_key)
        .add(cookie::Cookie::new("wb_csrf", "live-photo-test"));
    let headers = format!(
        "wb_session={}; wb_csrf={}",
        jar.get("wb_session").unwrap().value(),
        jar.get("wb_csrf").unwrap().value()
    );
    let photo = Uuid::new_v4();
    let body = Form::new()
        .text("csrf", "live-photo-test")
        .text("capture_id", photo.to_string())
        .part(
            "photo",
            Part::bytes(include_bytes!("../examples/poc_agent.png").to_vec())
                .file_name("poc.png")
                .mime_str("image/png")?,
        );
    let client = Client::new();
    let response = client
        .post(format!("{base}/competitions/3910/photos"))
        .header("Cookie", &headers)
        .multipart(body)
        .send()
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "La subida ha fallado: {}",
        response.status()
    );
    let worker = wodbuster_jueces::photos::start(state.clone());
    let result=async {
        let mut last=String::new();
        for _ in 0..360 {
            let photos=state.store.photos(&saved.username,3910,0).await?;
            let status=photos.iter().find(|p|p.id==photo).unwrap();
            if status.state!=last { eprintln!("Foto {photo}: {} ({})",status.label(),status.model);last=status.state.clone(); }
            match status.state.as_str() {
                "succeeded"=> {
                    let reading=status.reading.as_ref().unwrap();
                    anyhow::ensure!(status.provider==wodbuster_jueces::ai::PROVIDER&&status.model==wodbuster_jueces::ai::model_name(),"Proveedor/modelo incorrecto");
                    anyhow::ensure!(reading["score_value"]["repetitions"]==2,"La lectura no coincide con la imagen de prueba");
                    let local:serde_json::Value=sqlx::query_scalar("SELECT data->'value' FROM competition_scores WHERE username=lower($1) AND competition_id=3910 AND category_id=16907 AND team_reference='298682' AND wod_id=64459")
                        .bind(&saved.username).fetch_one(&pool).await?;
                    anyhow::ensure!(local["repetitions"]==2,"La copia local no contiene la puntuación confirmada");
                    let gallery=client.get(format!("{base}/competitions/3910/photos")).header("Cookie",&headers).send().await?.text().await?;
                    anyhow::ensure!(gallery.contains("Guardada y verificada")&&gallery.contains(&status.model),"La galería no refleja el resultado");
                    eprintln!("Foto guardada, IA persistida con su modelo y puntuación confirmada en Arena y Postgres.");
                    return Ok::<_,anyhow::Error>(());
                },
                "failed"|"incomplete"|"unconfirmed"|"saved_local_failed"=>anyhow::bail!("{}: {:?}; validación: {}",status.label(),status.error,status.validation_errors),
                _=>{},
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::bail!("La foto no ha terminado de procesarse")
    }.await;
    worker.abort();
    server.abort();
    result
}
