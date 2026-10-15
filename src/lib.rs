pub mod ai;
pub mod browser;
pub mod competitions;
pub mod config;
pub mod details;
pub mod loads;
pub mod photos;
pub mod scorecard;
pub mod scoring;
pub mod storage;
pub mod web;

use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use topcoat::cookie::Key;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<storage::SessionStore>,
    pub browser: Arc<browser::ArenaBrowser>,
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
            store: Arc::new(storage::SessionStore::new(pool, &config.session_secret)),
            browser: Arc::new(browser::ArenaBrowser::new(config.browser_url)),
            cookie_key: Key::derive_from(config.session_secret.as_bytes()),
            secure_cookies: config.secure_cookies,
        })
    }
}
