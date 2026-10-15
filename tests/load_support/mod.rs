use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

pub async fn wait_for_job(
    client: &Client,
    base: &str,
    headers: &str,
    location: &str,
) -> topcoat::Result<Value> {
    let id = location
        .split("?job=")
        .nth(1)
        .expect("Redirección con identificador de carga");
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let response = client
                .get(format!("{base}/loads/{id}"))
                .header("Cookie", headers)
                .send()
                .await?;
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["cache-control"], "no-store");
            let job: Value = serde_json::from_str(&response.text().await?)?;
            if job["state"] != "running" {
                return Ok::<_, topcoat::Error>(job);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| topcoat::Error::msg("La carga no terminó durante la prueba"))?
}
