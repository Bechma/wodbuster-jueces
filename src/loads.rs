use crate::{AppState, competitions::CompetitionError, storage::SavedSession};
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe, time::Duration};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoadJob {
    pub id: Uuid,
    pub competition_id: Option<i64>,
    pub state: String,
    pub error_code: Option<String>,
}

impl LoadJob {
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
    pub fn refresh_url(&self) -> String {
        self.competition_id
            .map(|id| format!("/competitions/{id}"))
            .unwrap_or_else(|| "/competitions".into())
    }
    pub fn location(&self) -> String {
        format!("{}?job={}", self.refresh_url(), self.id)
    }
    pub fn message(&self) -> &'static str {
        match self.state.as_str() {
            "running" => {
                "La información se está cargando en el servidor. Puedes seguir usando la aplicación."
            }
            "succeeded" => {
                "La carga ha terminado. Actualiza la página para ver la información nueva."
            }
            _ => match self.error_code.as_deref() {
                Some("session") => "La sesión de Arena ha caducado. Vuelve a iniciar sesión.",
                Some("verification") => {
                    "Arena solicita una verificación adicional. Vuelve a intentarlo."
                }
                Some("access") => {
                    "Ya no tienes acceso como juez o administrador a esta competición."
                }
                Some("storage") => "No se pudo guardar la información. Vuelve a intentarlo.",
                Some("timeout") => "La carga ha tardado demasiado. Vuelve a intentarlo.",
                Some("interrupted") => "La carga se ha interrumpido. Vuelve a intentarlo.",
                _ => "No se pudo cargar la información de Arena. Vuelve a intentarlo.",
            },
        }
    }
}

pub fn start(state: AppState, session: SavedSession, job: &LoadJob) {
    let id = job.id;
    let target = job.competition_id;
    let worker_state = state.clone();
    tokio::spawn(supervise(state, id, async move {
        // Browserless puede estar ocupado por otra carga: espera sin bloquear la petición HTTP.
        loop {
            let result = if let Some(id) = target {
                match worker_state
                    .browser
                    .competition_detail(&session.cookie, id)
                    .await
                {
                    Ok(detail) => {
                        return worker_state
                            .store
                            .save_competition_detail(&session.username, &detail)
                            .await
                            .map_err(|_| "storage");
                    }
                    Err(error) => error,
                }
            } else {
                match worker_state.browser.competitions(&session.cookie).await {
                    Ok(competitions) => {
                        return worker_state
                            .store
                            .save_competitions(&session.username, &competitions)
                            .await
                            .map_err(|_| "storage");
                    }
                    Err(error) => error,
                }
            };
            if matches!(result, CompetitionError::Busy) {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            return Err(match result {
                CompetitionError::SessionExpired => "session",
                CompetitionError::Verification => "verification",
                CompetitionError::Timeout => "timeout",
                CompetitionError::AccessDenied => "access",
                _ => "unavailable",
            });
        }
    }));
}

async fn supervise(
    state: AppState,
    id: Uuid,
    work: impl Future<Output = Result<(), &'static str>> + Send,
) {
    let work = tokio::time::timeout(
        Duration::from_secs(16 * 60),
        AssertUnwindSafe(work).catch_unwind(),
    );
    tokio::pin!(work);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    let error = loop {
        tokio::select! {
            result = &mut work => break match result {
                Ok(Ok(result)) => result.err(),
                Ok(Err(_)) => Some("unavailable"),
                Err(_) => Some("timeout"),
            },
            _ = heartbeat.tick() => match state.store.touch_load(id).await {
                Ok(true) => {},
                Ok(false) => return,
                Err(_) => break Some("storage"),
            }
        }
    };
    // Si Postgres cae, el heartbeat vencido permite mostrar la interrupción al recuperarse.
    let _ = state.store.finish_load(id, error).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{browser::ArenaBrowser, competitions::Competition, storage::SessionStore};
    use sqlx::PgPool;

    #[sqlx::test]
    #[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
    async fn jobs_are_detached_deduplicated_private_and_complete_after_commit(
        pool: PgPool,
    ) -> topcoat::Result<()> {
        let store = std::sync::Arc::new(SessionStore::new(
            pool.clone(),
            "test-session-secret-with-at-least-32-bytes",
        ));
        let (first, second) = tokio::join!(
            store.begin_load("judge@example.com", None),
            store.begin_load("JUDGE@example.com", None)
        );
        let (first, first_created) = first?;
        let (second, second_created) = second?;
        assert_eq!(first.id, second.id);
        assert_ne!(first_created, second_created);
        let (detail, _) = store.begin_load("judge@example.com", Some(3910)).await?;
        assert_ne!(first.id, detail.id);
        let (other, _) = store.begin_load("other@example.com", None).await?;
        assert!(
            store
                .load_job("judge@example.com", other.id)
                .await?
                .is_none()
        );
        let state = AppState {
            store: store.clone(),
            browser: std::sync::Arc::new(ArenaBrowser::new("ws://127.0.0.1:1".parse().unwrap())),
            cookie_key: topcoat::cookie::Key::derive_from(
                b"test-session-secret-with-at-least-32-bytes",
            ),
            secure_cookies: false,
        };
        let (release, gate) = tokio::sync::oneshot::channel();
        let work_store = store.clone();
        let task = tokio::spawn(supervise(state, first.id, async move {
            gate.await.map_err(|_| "unavailable")?;
            work_store
                .save_competitions("judge@example.com", &[Competition::new(3910, "Loaded")])
                .await
                .map_err(|_| "storage")
        }));
        assert!(
            store
                .load_job("judge@example.com", first.id)
                .await?
                .unwrap()
                .is_running()
        );
        assert!(store.competitions("judge@example.com").await?.is_empty());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task).await??;
        assert_eq!(
            store
                .load_job("judge@example.com", first.id)
                .await?
                .unwrap()
                .state,
            "succeeded"
        );
        assert_eq!(
            store.competitions("judge@example.com").await?[0].name,
            "Loaded"
        );
        assert!(
            store
                .load_job("judge@example.com", detail.id)
                .await?
                .unwrap()
                .is_running()
        );
        sqlx::query(
            "UPDATE background_loads SET heartbeat_at=now()-interval '61 seconds' WHERE id=$1",
        )
        .bind(other.id)
        .execute(&pool)
        .await?;
        let stale = store
            .load_job("other@example.com", other.id)
            .await?
            .unwrap();
        assert_eq!(stale.state, "failed");
        assert_eq!(stale.error_code.as_deref(), Some("interrupted"));
        assert!(store.begin_load("other@example.com", None).await?.1);
        Ok(())
    }
}
