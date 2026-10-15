//! Autenticación OAuth de ChatGPT y persistencia de la sesión gestionadas por Rig.
use anyhow::Context;
use rig::providers::{
    chatgpt::{
        self,
        auth::{AuthSource, Authenticator, DeviceCodeHandler},
    },
    openai::OpenAI,
};
use std::path::{Path, PathBuf};

static AUTH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn auth_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CHATGPT_AUTH_FILE").filter(|s| !s.is_empty()) {
        return Ok(path.into());
    }
    let home = std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .context("Configura HOME o CHATGPT_AUTH_FILE para guardar la sesión de ChatGPT")?;
    Ok(PathBuf::from(home).join(".config/wodbuster-jueces/auth.json"))
}

pub(super) async fn client(allow_device_flow: bool) -> anyhow::Result<OpenAI> {
    let _guard = AUTH_LOCK.lock().await;
    authenticate(&auth_path()?, allow_device_flow).await
}

async fn authenticate(path: &Path, allow_device_flow: bool) -> anyhow::Result<OpenAI> {
    let authenticator = Authenticator::new(
        AuthSource::OAuth,
        Some(path.to_owned()),
        DeviceCodeHandler::default(),
        allow_device_flow,
    );
    chatgpt::new("")
        .authenticate(&authenticator)
        .await
        .map_err(|_| anyhow::anyhow!(
            "No se pudo autenticar con ChatGPT. Ejecuta cargo run --example chatgpt_login o revisa CHATGPT_AUTH_FILE"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn reuses_rig_session_from_custom_file_without_modifying_it() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("custom-auth.json");
        let bytes = serde_json::to_vec(&json!({
            "access_token":"private-access-test",
            "refresh_token":"private-refresh-test",
            "expires_at":4102444800_i64,
            "account_id":"account-test"
        }))?;
        std::fs::write(&path, &bytes)?;

        // El mismo archivo sirve para el comando de login y para el worker.
        for allow_device_flow in [true, false] {
            let client = authenticate(&path, allow_device_flow).await?;
            assert_eq!(client.config().api_key.expose(), "private-access-test");
            assert_eq!(client.config().account_id.as_deref(), Some("account-test"));
            assert_eq!(std::fs::read(&path)?, bytes);
        }
        Ok(())
    }

    #[tokio::test]
    async fn worker_requires_login_without_starting_device_flow_or_changing_files()
    -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("auth.json");
        assert!(authenticate(&path, false).await.is_err());
        assert!(!path.exists());

        for bytes in [b"{}".as_slice(), b"invalid-private-session"] {
            std::fs::write(&path, bytes)?;
            let error = authenticate(&path, false).await.err().unwrap().to_string();
            assert!(error.contains("cargo run --example chatgpt_login"));
            assert!(!error.contains("private-session"));
            assert_eq!(std::fs::read(&path)?, bytes);
        }
        Ok(())
    }
}
