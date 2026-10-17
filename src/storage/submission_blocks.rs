use super::*;
use crate::submission_blocks::SubmissionBlock;

fn lock_key(username: &str, competition: i64) -> String {
    format!(
        "score-submissions:{}:{competition}",
        username.to_lowercase()
    )
}

impl SessionStore {
    pub async fn submission_blocks(
        &self,
        username: &str,
        competition: i64,
    ) -> sqlx::Result<Vec<SubmissionBlock>> {
        sqlx::query_as("SELECT category_id,wod_id,wod_name FROM score_submission_blocks WHERE username=lower($1) AND competition_id=$2 ORDER BY category_id,wod_name,wod_id")
            .bind(username).bind(competition).fetch_all(&self.pool).await
    }

    pub async fn set_submission_block(
        &self,
        username: &str,
        competition: i64,
        block: SubmissionBlock,
        blocked: bool,
    ) -> sqlx::Result<()> {
        let block = SubmissionBlock {
            wod_name: crate::submission_blocks::normalize_wod_name(&block.wod_name),
            ..block
        };
        let mut tx = self.pool.begin().await?;
        // Espera a los envíos en curso: al confirmar el bloqueo ya no queda ninguno activo.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(lock_key(username, competition))
            .execute(&mut *tx)
            .await?;
        if blocked {
            sqlx::query("INSERT INTO score_submission_blocks (username,competition_id,category_id,wod_id,wod_name) VALUES (lower($1),$2,$3,$4,$5) ON CONFLICT DO NOTHING")
                .bind(username).bind(competition).bind(block.category_id).bind(block.wod_id).bind(&block.wod_name).execute(&mut *tx).await?;
        } else {
            sqlx::query("DELETE FROM score_submission_blocks WHERE username=lower($1) AND competition_id=$2 AND category_id=$3 AND wod_id=$4 AND wod_name=$5")
                .bind(username).bind(competition).bind(block.category_id).bind(block.wod_id).bind(&block.wod_name).execute(&mut *tx).await?;
        }
        tx.commit().await
    }

    /// Mantener la transacción hasta terminar el intento remoto serializa el envío y los cambios de bloqueo.
    pub(crate) async fn submission_guard(
        &self,
        username: &str,
        competition: i64,
    ) -> sqlx::Result<(sqlx::Transaction<'_, sqlx::Postgres>, Vec<SubmissionBlock>)> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1, 0))")
            .bind(lock_key(username, competition))
            .execute(&mut *tx)
            .await?;
        let blocks = sqlx::query_as("SELECT category_id,wod_id,wod_name FROM score_submission_blocks WHERE username=lower($1) AND competition_id=$2 ORDER BY category_id,wod_name,wod_id")
            .bind(username).bind(competition).fetch_all(&mut *tx).await?;
        Ok((tx, blocks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    #[sqlx::test]
    #[ignore = "Solo Postgres local; sin navegador, IA ni Arena"]
    async fn block_confirmation_waits_for_active_submission_and_next_attempt_reads_new_rules(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        let store = Arc::new(SessionStore::new(pool, "local-submission-block-lock-test"));
        store
            .save_competitions("judge@example.com", &[Competition::new(3910, "Fixture")])
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let (guard, blocks) = store.submission_guard("JUDGE@example.com", 3910).await?;
        assert!(blocks.is_empty());
        let block = SubmissionBlock {
            category_id: 16907,
            wod_id: 0,
            wod_name: String::new(),
        };
        let writer_store = store.clone();
        let writer_block = block.clone();
        let mut writer = tokio::spawn(async move {
            writer_store
                .set_submission_block("judge@example.com", 3910, writer_block, true)
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut writer)
                .await
                .is_err()
        );
        // Otros ámbitos no están retenidos por el envío de esta cuenta/competición.
        let (other_guard, other_blocks) = store.submission_guard("other@example.com", 3910).await?;
        assert!(other_blocks.is_empty());
        other_guard.commit().await?;
        guard.commit().await?;
        tokio::time::timeout(Duration::from_secs(5), writer).await???;
        let (guard, blocks) = store.submission_guard("judge@example.com", 3910).await?;
        assert_eq!(blocks, vec![block]);
        guard.commit().await?;
        Ok(())
    }
}
