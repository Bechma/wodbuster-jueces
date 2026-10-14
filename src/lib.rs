pub mod browser;
pub mod competitions;
pub mod config;
pub mod details;
pub mod storage;
pub mod web;

use sqlx::postgres::PgPoolOptions;
use topcoat::cookie::Key;

pub struct AppState {
    pub store: storage::SessionStore,
    pub browser: browser::ArenaBrowser,
    pub cookie_key: Key,
    pub secure_cookies: bool,
}

impl AppState {
    pub async fn connect(config: config::Config) -> topcoat::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&config.database_url)
            .await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self {
            store: storage::SessionStore::new(pool, &config.session_secret),
            browser: browser::ArenaBrowser::new(config.browser_url),
            cookie_key: Key::derive_from(config.session_secret.as_bytes()),
            secure_cookies: config.secure_cookies,
        })
    }
}
