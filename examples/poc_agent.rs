//! Imagen → lectura tipada con Rig → plan de rellenado de Arena.
//! Con --submit guarda una puntuación en Arena y actualiza su copia en Postgres.
//!
//! cargo run --example poc_agent -- foto.png contexto.json
//! cargo run --example poc_agent -- --validate lectura.json contexto.json
//! cargo run --example poc_agent -- --submit foto.png contexto.json
//!
//! contexto.json es un CompetitionDetail real, por ejemplo el snapshot de Postgres.
//! Crea la sesión OAuth con `cargo run --example chatgpt_login`.
//! CHATGPT_AUTH_FILE elige el archivo; CHATGPT_MODEL es opcional (gpt-6-luna por defecto).
//! stdout contiene exclusivamente un documento JSON; los diagnósticos van a stderr.

use anyhow::{Context, ensure};
use wodbuster_jueces::{
    browser::ArenaBrowser,
    config::Config,
    details::CompetitionDetail,
    scorecard::{ScorecardReading, evaluate_reading},
    storage::{SavedSession, SessionStore},
};

fn reading_from_json(json: serde_json::Value) -> anyhow::Result<ScorecardReading> {
    let mut reading = json.get("reading").cloned().unwrap_or(json);
    // Permite reutilizar salidas antiguas de --validate sin volver a pedir la imagen.
    if let Some(fields) = reading.as_object_mut() {
        for removed in ["is_confident", "score_raw", "score_notes"] {
            fields.remove(removed);
        }
    }
    Ok(serde_json::from_value(reading)?)
}

#[derive(Debug, PartialEq)]
struct Args {
    offline: bool,
    submit: bool,
    session: Option<uuid::Uuid>,
    input: String,
    context: String,
}

fn parse_args(args: Vec<String>) -> anyhow::Result<Args> {
    let mut offline = false;
    let mut submit = false;
    let mut session = None;
    let mut paths = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--validate" if !offline => offline = true,
            "--submit" if !submit => submit = true,
            "--session" if session.is_none() => {
                session = Some(
                    args.next()
                        .context("Falta el UUID de --session")?
                        .parse()
                        .context("--session debe ser un UUID válido")?,
                );
            }
            _ if arg.starts_with('-') => {
                anyhow::bail!("Flag desconocida o repetida: {arg}. Usa --help.")
            }
            _ => paths.push(arg),
        }
    }
    ensure!(
        paths.len() == 2,
        "Indica la imagen/lectura y el contexto. Usa --help."
    );
    ensure!(session.is_none() || submit, "--session requiere --submit");
    Ok(Args {
        offline,
        submit,
        session,
        input: paths.remove(0),
        context: paths.remove(0),
    })
}

struct Submission {
    store: SessionStore,
    browser: ArenaBrowser,
    saved: SavedSession,
    detail: CompetitionDetail,
}

impl Submission {
    async fn connect(competition_id: i64, session: Option<uuid::Uuid>) -> anyhow::Result<Self> {
        let config = Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&config.database_url)
            .await
            .context("No se pudo conectar con Postgres")?;
        let ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT s.id FROM wodbuster_sessions s JOIN competition_details d ON d.username=lower(s.username) AND d.competition_id=$1 WHERE ($2::uuid IS NULL OR s.id=$2) ORDER BY s.saved_at DESC",
        ).bind(competition_id).bind(session).fetch_all(&pool).await?;
        let store = SessionStore::new(pool, &config.session_secret);
        let mut selected: Option<SavedSession> = None;
        for id in ids {
            if let Some(saved) = store.load(id).await.map_err(|e| anyhow::anyhow!("{e}"))? {
                if let Some(previous) = &selected {
                    ensure!(
                        previous.username.eq_ignore_ascii_case(&saved.username),
                        "Hay varias cuentas con esta competición. Elige una con --session <UUID>"
                    );
                } else {
                    selected = Some(saved);
                }
            }
        }
        let saved = selected.context("No hay una sesión válida con esta competición cargada. Accede y carga sus detalles en la aplicación")?;
        let detail = store
            .competition_detail(&saved.username, competition_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .context("Carga los detalles de la competición en la aplicación")?;
        Ok(Self {
            store,
            browser: ArenaBrowser::new(config.browser_url),
            saved,
            detail,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] || args == ["-h"] {
        eprintln!(
            "Uso:\n  cargo run --example poc_agent -- [--submit] [--session <UUID>] <foto.png> <contexto.json>\n  cargo run --example poc_agent -- --validate [--submit] [--session <UUID>] <lectura.json> <contexto.json>\n\n--submit guarda realmente en Arena usando la cookie de Postgres y actualiza la puntuación local.\nSin --submit solo genera el plan. --validate reutiliza una lectura sin llamar a la IA."
        );
        return Ok(());
    }
    let args = parse_args(args)?;
    let mut detail: CompetitionDetail = serde_json::from_str(
        &std::fs::read_to_string(&args.context)
            .context("No se pudo leer el contexto de competición")?,
    )
    .context("El contexto debe ser un CompetitionDetail real en JSON")?;
    wodbuster_jueces::scoring::prepare_detail(&mut detail);
    let submission = if args.submit {
        Some(Submission::connect(detail.id, args.session).await?)
    } else {
        None
    };
    if let Some(submission) = &submission {
        // Para escribir, tanto la extracción como el plan usan el contexto de la cuenta autenticada.
        detail = submission.detail.clone();
    }
    let reading = if args.offline {
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args.input)?)?;
        reading_from_json(json)?
    } else {
        let bytes = std::fs::read(&args.input).context("No se pudo leer la imagen")?;
        let model = wodbuster_jueces::ai::model_name();
        eprintln!("Proveedor: ChatGPT nativo de Rig; modelo: {model}.");
        wodbuster_jueces::ai::extract(bytes, &detail, &model).await?
    };
    let result = evaluate_reading(&detail, reading);
    let mut output = serde_json::to_value(&result)?;
    if let Some(submission) = submission {
        let Some(prepared) = &result.prepared else {
            output["submission"] = serde_json::json!({"status":"not_submitted"});
            println!("{}", serde_json::to_string_pretty(&output)?);
            anyhow::bail!("La lectura no es válida; no se ha enviado ninguna puntuación");
        };
        eprintln!(
            "Enviando: {} / {} / {}…",
            prepared.category_name, prepared.wod_name, prepared.team_name
        );
        match submission
            .browser
            .submit_score(&submission.saved.cookie, &detail, &result.reading)
            .await
        {
            Ok(score) => {
                output["submission"] = serde_json::json!({"status":"saved","verified":true,"local_updated":false,"score":score});
                if let Err(error) = submission
                    .store
                    .save_confirmed_score(
                        &submission.saved.username,
                        detail.id,
                        prepared.category_id,
                        &prepared.team_reference,
                        &score,
                    )
                    .await
                {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                    return Err(anyhow::anyhow!("{error}").context("Guardado en Arena, pero falló la actualización local. Refresca la competición; no repitas el envío"));
                }
                output["submission"]["local_updated"] = serde_json::json!(true);
                eprintln!("Puntuación guardada y verificada en Arena; copia local actualizada.");
            }
            Err(error) => {
                let status = if error.save_attempted {
                    "unconfirmed"
                } else {
                    "not_submitted"
                };
                let message = if error.save_attempted {
                    "No se confirmó el envío. Comprueba Arena antes de volver a ejecutarlo"
                } else {
                    "El envío se detuvo antes de pulsar Guardar; no se ha actualizado Arena"
                };
                output["submission"] =
                    serde_json::json!({"status":status,"error":error.to_string()});
                println!("{}", serde_json::to_string_pretty(&output)?);
                return Err(anyhow::Error::new(error).context(message));
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_readings_accept_the_new_contract_and_older_results() {
        let data = serde_json::json!({"category_id":16907,"wod_id":64459,"team_reference":"298682",
            "score_value":{"kind":"not_completed","repetitions":2,"tie_break":null}});
        assert!(
            reading_from_json(data.clone())
                .unwrap()
                .score_value
                .is_some()
        );
        let mut old = data;
        old["is_confident"] = serde_json::json!(false);
        old["score_raw"] = serde_json::json!(null);
        old["score_notes"] = serde_json::json!("Salida antigua");
        let reading = reading_from_json(serde_json::json!({"reading":old})).unwrap();
        let serialized = serde_json::to_value(reading).unwrap();
        for removed in ["is_confident", "score_raw", "score_notes"] {
            assert!(serialized.get(removed).is_none());
        }
    }

    #[test]
    fn submission_is_explicit_and_can_reuse_a_reading() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|s| s.to_string()).collect());
        assert!(!parse(&["image.png", "context.json"]).unwrap().submit);
        let args = parse(&["--submit", "--validate", "result.json", "context.json"]).unwrap();
        assert!(args.submit && args.offline);
        assert!(parse(&["--submt", "image.png", "context.json"]).is_err());
        assert!(parse(&["--submit", "--submit", "image.png", "context.json"]).is_err());
        assert!(parse(&["--session", "123", "image.png", "context.json"]).is_err());
    }
}
