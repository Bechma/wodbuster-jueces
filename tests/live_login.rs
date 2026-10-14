use serde::Deserialize;
use wodbuster_jueces::{browser::ArenaBrowser, config::Config};

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[tokio::test]
#[ignore = "Acceso real a WodBuster con tmp/SECRET.json; puede requerir verificación externa"]
async fn authenticates_with_secret_file() -> topcoat::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let credentials: Credentials = serde_json::from_slice(&std::fs::read("tmp/SECRET.json")?)?;
    let browser = ArenaBrowser::new(config.browser_url);
    let cookie = browser
        .login(&credentials.username, &credentials.password)
        .await?;
    assert!(cookie.is_valid());
    Ok(())
}
