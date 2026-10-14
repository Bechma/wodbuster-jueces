use std::env;
use url::Url;

// Deliberately not Debug: configuration contains service credentials.
pub struct Config {
    pub database_url: String,
    pub browser_url: Url,
    pub session_secret: String,
    pub secure_cookies: bool,
}

impl Config {
    pub fn from_env() -> topcoat::Result<Self> {
        let database_url = required("DATABASE_URL")?;
        let mut browser_url = Url::parse(&required("BROWSER_URL")?)
            .map_err(|_| topcoat::Error::msg("BROWSER_URL debe ser una URL válida"))?;
        if !matches!(browser_url.scheme(), "ws" | "wss") {
            return Err(topcoat::Error::msg("BROWSER_URL debe usar ws:// o wss://"));
        }
        let launch = serde_json::json!({
            "headless": false,
            "stealth": true,
            "args": ["--window-size=1920,1080", "--lang=es-ES"]
        });
        browser_url
            .query_pairs_mut()
            .append_pair("token", &required("BROWSER_TOKEN")?)
            .append_pair("launch", &launch.to_string());
        let session_secret = required("SESSION_SECRET")?;
        if session_secret.len() < 32 {
            return Err(topcoat::Error::msg(
                "SESSION_SECRET debe tener al menos 32 bytes",
            ));
        }
        let secure_cookies = match env::var("COOKIE_SECURE").as_deref() {
            Ok("true") => true,
            Ok("false") | Err(_) => false,
            _ => return Err(topcoat::Error::msg("COOKIE_SECURE debe ser true o false")),
        };
        Ok(Self {
            database_url,
            browser_url,
            session_secret,
            secure_cookies,
        })
    }
}

fn required(name: &str) -> topcoat::Result<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| topcoat::Error::msg(format!("Falta la variable {name}")))
}
