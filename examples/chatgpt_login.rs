//! Crea o renueva la sesión OAuth de ChatGPT usada por la aplicación.
//! Ejecuta: cargo run --example chatgpt_login
//! CHATGPT_AUTH_FILE permite elegir el archivo de credenciales.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    wodbuster_jueces::ai::login().await?;
    eprintln!("Sesión de ChatGPT lista para usar.");
    Ok(())
}
