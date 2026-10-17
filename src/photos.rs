//! Cola persistente de fotos: un único worker global extrae y envía puntuaciones.
use crate::{
    AppState,
    scorecard::{ScorecardReading, evaluate_reading},
};
use anyhow::{Context, ensure};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use std::{io::Cursor, time::Duration};
use uuid::Uuid;

pub const MAX_IMAGE_BYTES: usize = 1_572_864;

/// Asignación actual de una foto, según su lectura extraída o corregida.
#[derive(Default)]
pub struct PhotoFilter {
    pub category_id: Option<i64>,
    pub wod_id: Option<i64>,
    pub team_reference: Option<String>,
    pub issues: bool,
    pub unsynced: bool,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Photo {
    pub id: Uuid,
    #[serde(skip)]
    pub username: String,
    pub competition_id: i64,
    #[serde(skip)]
    pub session_id: Uuid,
    pub media_type: String,
    pub width: i32,
    pub height: i32,
    pub bytes: i32,
    pub provider: String,
    pub model: String,
    pub state: String,
    pub revision: i64,
    pub reading: Option<Value>,
    pub prepared: Option<Value>,
    pub validation_errors: Value,
    pub error: Option<String>,
    pub created_at: String,
    #[sqlx(default)]
    pub duplicate_count: i64,
    #[sqlx(default)]
    pub is_latest: bool,
    #[sqlx(default)]
    pub sync_status: String,
    #[sqlx(default)]
    pub sync_checked_at: Option<String>,
    #[sqlx(default)]
    pub arena_value: Option<Value>,
}

impl Photo {
    pub fn sync_label(&self) -> &'static str {
        match self.sync_status.as_str() {
            "synced" => "Sincronizada con Wodbuster",
            "different" => "La puntuación no coincide con Wodbuster",
            "superseded" => "Se compara la última foto añadida para este equipo y WOD",
            "unavailable" => "No se puede comparar: faltan datos o el formato no es compatible",
            _ => "Sin comprobar: consulta los datos de Wodbuster",
        }
    }

    pub fn duplicate_label(&self) -> &'static str {
        if self.is_latest {
            "Hay varias fotos para esta categoría, WOD y equipo. Esta es la última añadida y se utiliza para comprobar la sincronización."
        } else {
            "Hay varias fotos para esta categoría, WOD y equipo. Se utiliza la última añadida para comprobar la sincronización."
        }
    }
    pub fn progress(&self) -> &'static str {
        match self.state.as_str() {
            "queued" => "Foto en cola. El análisis comenzará cuando el worker esté disponible.",
            "parsing" => "Analizando la imagen con IA…",
            "parsed" => "Datos preparados. Puntuación en cola para guardar en Wodbuster.",
            "submitting" => "Guardando y verificando la puntuación en Wodbuster…",
            "succeeded" => "Proceso completado. Puntuación guardada y verificada en Wodbuster.",
            "incomplete" => {
                "Análisis terminado: faltan datos o no son válidos. Puedes corregirlos o volver a analizar la foto."
            }
            "unconfirmed" => {
                "No se pudo confirmar el guardado. Comprueba Arena antes de repetirlo."
            }
            "saved_local_failed" => {
                "Arena confirmó la puntuación; falta actualizar la copia local."
            }
            _ => "El procesamiento ha fallado. Consulta el error antes de volver a intentarlo.",
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.state.as_str(),
            "queued" | "parsing" | "parsed" | "submitting"
        )
    }
    pub fn warning(&self) -> bool {
        matches!(self.state.as_str(), "succeeded" | "saved_local_failed")
            && self.prepared.is_some()
            && self
                .reading
                .as_ref()
                .is_some_and(|r| r["needs_retake"] == true)
    }
    pub fn label(&self) -> &'static str {
        match self.state.as_str() {
            "queued" => "En cola",
            "parsing" => "Analizando con IA",
            "parsed" => "Pendiente de guardar",
            "submitting" => "Guardando en Wodbuster",
            "succeeded" => "Guardada y verificada",
            "incomplete" => "Faltan datos o no son válidos",
            "unconfirmed" => "Envío sin confirmar: comprueba Arena",
            "saved_local_failed" => "Guardada en Arena; falta actualizar la copia local",
            _ => "Error",
        }
    }
}

pub struct NewPhoto<'a> {
    pub id: Uuid,
    pub username: &'a str,
    pub competition_id: i64,
    pub session_id: Uuid,
    pub image: &'a [u8],
    pub media_type: &'a str,
    pub width: i32,
    pub height: i32,
    pub model: &'a str,
}

/// Comprueba el contenido, sin confiar en la extensión ni en el MIME del navegador.
pub fn validate_image(bytes: &[u8]) -> anyhow::Result<(&'static str, i32, i32)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
        "La foto supera el máximo de 1,5 MB"
    );
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let media_type = match reader.format() {
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::WebP) => "image/webp",
        _ => anyhow::bail!("Formato de imagen no admitido"),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(80 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().context("La imagen no se puede leer")?;
    Ok((media_type, decoded.width() as i32, decoded.height() as i32))
}

pub fn start(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match state.store.photo_worker_lock().await {
                Ok(Some(mut connection)) => {
                    match std::panic::AssertUnwindSafe(run_locked(&state, &mut connection))
                        .catch_unwind()
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => eprintln!("Worker de fotos detenido: {error}"),
                        Err(_) => eprintln!("Worker de fotos interrumpido; se recuperará la cola"),
                    }
                    // No devolver a un pool una sesión con un advisory lock.
                    let _ = connection.close().await;
                }
                Ok(None) => {}
                Err(_) => eprintln!("Worker de fotos: Postgres no está disponible"),
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    })
}

async fn run_locked(state: &AppState, connection: &mut PgConnection) -> anyhow::Result<()> {
    state.store.recover_photos().await?;
    loop {
        // Si se pierde la sesión que posee el lock se cancela la operación en curso.
        let work = async {
            if let Some(photo) = state.store.claim_photo().await? {
                let failure_state = if photo.state == "submitting" {
                    "unconfirmed"
                } else {
                    "failed"
                };
                if let Err(error) = process(state, &photo).await {
                    state
                        .store
                        .fail_photo(&photo, failure_state, &error.to_string())
                        .await?;
                }
            } else {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::pin!(work);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                result=&mut work=>{result?;break;},
                _=heartbeat.tick()=>{tokio::time::timeout(Duration::from_secs(3),connection.ping()).await??;},
            }
        }
    }
}

async fn process(state: &AppState, photo: &Photo) -> anyhow::Result<()> {
    let detail = state
        .store
        .competition_detail(&photo.username, photo.competition_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .context("Carga los detalles de la competición antes de procesar fotos")?;
    if photo.state == "parsing" {
        let (bytes, _) = state
            .store
            .photo_image(&photo.username, photo.competition_id, photo.id)
            .await?
            .context("Foto no disponible")?;
        let mut reading = crate::ai::extract(bytes, &detail, &photo.model).await?;
        let blocks = state
            .store
            .submission_blocks(&photo.username, photo.competition_id)
            .await?;
        if reject_if_blocked(&state.store, photo, &detail, &mut reading, &blocks).await? {
            return Ok(());
        }
        let result = evaluate_reading(&detail, reading);
        state.store.parsed_photo(photo, &result).await?;
        return Ok(());
    }
    let mut reading: ScorecardReading =
        serde_json::from_value(photo.reading.clone().context("Falta la lectura de IA")?)?;
    let blocks = state
        .store
        .submission_blocks(&photo.username, photo.competition_id)
        .await?;
    if reject_if_blocked(&state.store, photo, &detail, &mut reading, &blocks).await? {
        return Ok(());
    }
    let prepared = evaluate_reading(&detail, reading.clone());
    let Some(plan) = &prepared.prepared else {
        state.store.parsed_photo(photo, &prepared).await?;
        return Ok(());
    };
    let session = state
        .store
        .load(photo.session_id)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .context("La sesión de Arena ha caducado. Inicia sesión de nuevo")?;
    ensure!(
        session.username.eq_ignore_ascii_case(&photo.username),
        "La sesión no corresponde a la foto"
    );
    let submission = loop {
        let (guard, blocks) = state
            .store
            .submission_guard(&photo.username, photo.competition_id)
            .await?;
        if let Some(reason) = crate::submission_blocks::enforce(&detail, &mut reading, &blocks) {
            // Ya no se enviará nada: libera la conexión antes de persistir el rechazo.
            guard.commit().await?;
            state
                .store
                .reject_blocked_photo(photo, &reading, &reason)
                .await?;
            return Ok(());
        }
        let result = state
            .browser
            .submit_score(&session.cookie, &detail, &prepared.reading)
            .await;
        // Libera también antes de esperar a Browserless: el siguiente intento relee los bloqueos.
        guard.commit().await?;
        if result.as_ref().is_err_and(|error| error.busy) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        break result;
    };
    match submission {
        Ok(score) => persist_confirmed_photo(&state.store, photo, plan, &score).await?,
        Err(error) => {
            state
                .store
                .finish_photo(
                    photo,
                    if error.save_attempted {
                        "unconfirmed"
                    } else {
                        "failed"
                    },
                    Some(&error.to_string()),
                    None,
                )
                .await?;
        }
    }
    Ok(())
}

async fn reject_if_blocked(
    store: &crate::storage::SessionStore,
    photo: &Photo,
    detail: &crate::details::CompetitionDetail,
    reading: &mut ScorecardReading,
    blocks: &[crate::submission_blocks::SubmissionBlock],
) -> anyhow::Result<bool> {
    let Some(reason) = crate::submission_blocks::enforce(detail, reading, blocks) else {
        return Ok(false);
    };
    store.reject_blocked_photo(photo, reading, &reason).await?;
    Ok(true)
}

async fn persist_confirmed_photo(
    store: &crate::storage::SessionStore,
    photo: &Photo,
    plan: &crate::scorecard::PreparedScore,
    score: &crate::details::Score,
) -> anyhow::Result<()> {
    if !store.confirm_photo(photo, score).await? {
        return Ok(()); // La revisión ya no pertenece a este worker.
    }
    if store
        .save_confirmed_photo_score(photo, plan.category_id, &plan.team_reference, score)
        .await
        .is_err()
    {
        store.fail_photo(photo, "saved_local_failed", "Arena confirmó la puntuación, pero falló su copia local. Refresca la competición; no repitas el envío.").await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        competitions::Competition,
        scoring::{ScoreFormat, ScoreValue},
        storage::SessionStore,
    };
    use serde_json::json;
    mod support {
        use crate as wodbuster_jueces;
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mod.rs"));
    }

    #[test]
    fn images_are_decoded_and_bounded() {
        let bytes = include_bytes!("../tests/fixtures/image.png");
        assert_eq!(validate_image(bytes).unwrap().0, "image/png");
        assert!(validate_image(b"not an image").is_err());
        assert!(validate_image(&bytes[..20]).is_err());
        assert!(validate_image(&vec![0; MAX_IMAGE_BYTES + 1]).is_err());
    }

    #[tokio::test]
    #[ignore = "Requiere Browserless; prueba cámara simulada y compresión sin llamar a IA ni Arena"]
    async fn browser_compresses_uploads_and_closes_the_camera() -> anyhow::Result<()> {
        use chromiumoxide::{
            Browser,
            cdp::browser_protocol::target::{CreateBrowserContextParams, CreateTargetParams},
        };
        use futures_util::StreamExt;
        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
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
            page.set_content(r#"<form action="/upload" data-photo-upload><input name="csrf" value="test-csrf" type="hidden"><input type="file" accept="image/*" data-photo-input><input type="file" accept="image/*" capture="environment" data-photo-camera-input><button type="button" data-photo-open>Foto</button><button type="button" data-photo-file>Galería o archivos</button><p data-photo-message></p><dialog data-photo-camera><video autoplay playsinline muted></video><p data-camera-message></p><button type="button" data-photo-capture>Capturar</button><button type="button" data-photo-cancel>Cancelar</button></dialog></form>"#).await?;
            page.evaluate(r#"(()=>{
                window.uploads=[];
                window.fetch=async(url,options)=>{const blob=options.body.get('photo');const bitmap=await createImageBitmap(blob);uploads.push({csrf:options.body.get('csrf'),id:options.body.get('capture_id'),bytes:blob.size,type:blob.type,width:bitmap.width,height:bitmap.height});bitmap.close();return {ok:true,json:async()=>({id:'test'})};};
                const source=document.createElement('canvas');source.width=3840;source.height=2160;
                const ctx=source.getContext('2d');ctx.fillStyle='white';ctx.fillRect(0,0,3840,2160);ctx.fillStyle='black';ctx.font='120px sans-serif';ctx.fillText('Equipo Test - Giralda - 2 reps',100,500);
                window.testCanvas=source;
                Object.defineProperty(navigator,'mediaDevices',{configurable:true,value:{getUserMedia:async()=>{window.testStream=source.captureStream(10);window.testFrames=setInterval(()=>ctx.fillRect(0,0,1,1),50);return testStream;}}});
                return true;
            })()"#).await?;
            page.evaluate(include_str!("web/photos.js")).await?;
            page.evaluate(r#"(async()=>{const blob=await new Promise(resolve=>testCanvas.toBlob(resolve,'image/png'));const files=new DataTransfer();files.items.add(new File([blob],'camera.png',{type:'image/png'}));const input=document.querySelector('input[type=file]');input.files=files.files;input.dispatchEvent(new Event('change'));return true;})()"#).await?;
            for _ in 0..100 {
                if page.evaluate("uploads.length").await?.into_value::<u32>()?==1 {break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let first:Value=page.evaluate("uploads[0]").await?.into_value()?;
            assert_eq!(first["width"],1920);assert_eq!(first["height"],1080);
            assert_eq!(first["csrf"],"test-csrf");assert!(first["bytes"].as_u64().unwrap()<350*1024);
            assert!(first["type"]=="image/webp"||first["type"]=="image/jpeg");
            assert!(Uuid::parse_str(first["id"].as_str().unwrap()).is_ok());
            page.evaluate("document.querySelector('[data-photo-open]').click()").await?;
            for _ in 0..100 {
                if page.evaluate("!document.querySelector('[data-photo-capture]').disabled && document.querySelector('video').videoWidth>0").await?.into_value::<bool>()? {break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let camera:Value=page.evaluate("({open:document.querySelector('dialog').open,ready:!document.querySelector('[data-photo-capture]').disabled,width:document.querySelector('video').videoWidth,message:document.querySelector('[data-camera-message]').textContent})").await?.into_value()?;
            ensure!(camera["ready"]==true&&camera["width"].as_u64().unwrap_or(0)>0,"Cámara simulada no disponible: {camera}");
            page.evaluate("document.querySelector('[data-photo-capture]').click()").await?;
            for _ in 0..100 {
                if page.evaluate("uploads.length").await?.into_value::<u32>()?==2 {break;}
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert_eq!(page.evaluate("uploads.length").await?.into_value::<u32>()?,2);
            assert!(page.evaluate("!document.querySelector('dialog').open && testStream.getTracks().every(t=>t.readyState==='ended')").await?.into_value::<bool>()?);
            Ok::<_,anyhow::Error>(())
        }.await;
        let _ = browser.dispose_browser_context(context).await;
        driver.abort();
        result
    }

    #[sqlx::test]
    #[ignore = "Requiere Postgres de compose.dev.yml"]
    async fn singleton_queue_persists_results_recovers_and_never_retries_uncertain_saves(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let store = SessionStore::new(pool.clone(), "test-session-secret-with-at-least-32-bytes");
        for user in ["one@example.com", "two@example.com"] {
            store
                .save_competitions(user, &[Competition::new(3910, "Test")])
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        let first = store.photo_worker_lock().await?.unwrap();
        assert!(store.photo_worker_lock().await?.is_none());
        first.close().await?;
        let second = store.photo_worker_lock().await?.unwrap();
        let id = Uuid::new_v4();
        let bytes = include_bytes!("../tests/fixtures/image.png");
        let (_, w, h) = validate_image(bytes)?;
        for _ in 0..2 {
            store
                .save_photo(NewPhoto {
                    id,
                    username: "one@example.com",
                    competition_id: 3910,
                    session_id: Uuid::nil(),
                    image: bytes,
                    media_type: "image/png",
                    width: w,
                    height: h,
                    model: "test-model",
                })
                .await?;
        }
        assert_eq!(store.photos("one@example.com", 3910, 0).await?.len(), 1);
        assert!(store.photos("two@example.com", 3910, 0).await?.is_empty());
        assert!(
            store
                .photo_image("two@example.com", 3910, id)
                .await?
                .is_none()
        );
        let job = store.claim_photo().await?.unwrap();
        assert_eq!(job.state, "parsing");
        assert!(store.claim_photo().await?.is_none());
        store.recover_photos().await?;
        let parsing = store.claim_photo().await?.unwrap();
        assert_eq!(parsing.state, "parsing");
        let mut detail = support::fixture();
        let fields = json!([{"attributes":{"name":"panel$IoReps","id":"reps"},"options":[]}]);
        detail.categories[0].wods[0].score_format = ScoreFormat::from_fields(&fields);
        detail.categories[0].teams[0].scores[0].metadata =
            json!({"fields":fields,"save_controls":[{"name":"panel$CtlSave","id":"save"}]});
        let reading = ScorecardReading {
            category_id: Some(16907),
            wod_id: Some(64459),
            team_reference: Some("298682".into()),
            score_value: Some(ScoreValue::Repetitions { repetitions: 2 }),
            judge_name: None,
            needs_retake: true,
            retake_reason: Some("Revisar foto".into()),
        };
        let extracted = evaluate_reading(&detail, reading);
        assert!(extracted.ready_for_form);
        store.parsed_photo(&parsing, &extracted).await?;
        let photo = store.photos("one@example.com", 3910, 0).await?.remove(0);
        assert_eq!(photo.model, "test-model");
        assert!(photo.reading.is_some());
        assert!(!photo.warning());
        let submitting = store.claim_photo().await?.unwrap();
        assert_eq!(submitting.state, "submitting");
        store.recover_photos().await?;
        assert!(store.claim_photo().await?.is_none());
        assert_eq!(
            store.photos("one@example.com", 3910, 0).await?[0].state,
            "unconfirmed"
        );
        let score = detail.categories[0].teams[0].scores[0].clone();
        let recovered = store.photo("one@example.com", 3910, id).await?.unwrap();
        assert!(
            store
                .requeue_photo(&recovered, Uuid::nil(), Some(&extracted), "test-model")
                .await?
        );
        let submitting = store.claim_photo().await?.unwrap();
        assert!(store.confirm_photo(&submitting, &score).await?);
        assert!(
            store
                .fail_photo(&submitting, "unconfirmed", "Falló la copia local")
                .await?
        );
        sqlx::query("UPDATE score_photos SET provider='google-gemini' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        let photo = store.photos("one@example.com", 3910, 0).await?.remove(0);
        assert_eq!(photo.state, "saved_local_failed");
        assert!(photo.warning());
        assert!(
            store
                .requeue_photo(&photo, Uuid::nil(), Some(&extracted), "test-model")
                .await?
        );
        assert!(
            !store
                .requeue_photo(&photo, Uuid::nil(), None, "other-model")
                .await?
        );
        let queued = store.claim_photo().await?.unwrap();
        assert_eq!(queued.state, "submitting");
        assert_eq!(queued.provider, "google-gemini"); // La edición manual no atribuye a otra IA la lectura histórica.
        assert_eq!(queued.revision, photo.revision + 1);
        assert_eq!(
            queued.reading.as_ref().unwrap()["score_value"]["repetitions"],
            2
        );
        assert!(
            !store
                .requeue_photo(&queued, Uuid::nil(), None, "other-model")
                .await?
        );
        store.fail_photo(&queued, "failed", "Before saving").await?;
        let done = store.photo("one@example.com", 3910, id).await?.unwrap();
        assert_eq!(done.state, "failed"); // No conserva la confirmación de un intento anterior.
        assert!(
            store
                .requeue_photo(&done, Uuid::nil(), None, "new-model")
                .await?
        );
        let parsing = store.claim_photo().await?.unwrap();
        assert_eq!(parsing.state, "parsing");
        assert_eq!(parsing.revision, queued.revision + 1);
        assert_eq!(parsing.model, "new-model");
        assert_eq!(parsing.provider, crate::ai::PROVIDER);
        assert!(parsing.reading.is_none());
        let serialized = serde_json::to_value(photo)?;
        assert!(serialized.get("username").is_none() && serialized.get("session_id").is_none());
        let legacy = Uuid::new_v4();
        store
            .save_photo(NewPhoto {
                id: legacy,
                username: "one@example.com",
                competition_id: 3910,
                session_id: Uuid::nil(),
                image: bytes,
                media_type: "image/png",
                width: w,
                height: h,
                model: "gemini-3.8-flash",
            })
            .await?;
        sqlx::query("UPDATE score_photos SET provider='google-gemini' WHERE id=$1")
            .bind(legacy)
            .execute(&pool)
            .await?;
        let migrated = store.claim_photo().await?.unwrap();
        assert_eq!(migrated.id, legacy);
        assert_eq!(migrated.provider, crate::ai::PROVIDER);
        assert_eq!(migrated.model, crate::ai::model_name());
        second.close().await?;
        Ok(())
    }
}

#[cfg(test)]
mod regression_tests;
