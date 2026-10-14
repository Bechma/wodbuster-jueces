#[tokio::main]
async fn main() -> topcoat::Result<()> {
    dotenvy::dotenv().ok();
    let config = wodbuster_jueces::config::Config::from_env()?;
    let state = wodbuster_jueces::AppState::connect(config).await?;
    topcoat::start(wodbuster_jueces::web::router(state)).await?;
    Ok(())
}
