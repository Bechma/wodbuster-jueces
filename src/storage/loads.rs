use super::*;
use crate::loads::LoadJob;

impl SessionStore {
    async fn expire_loads(&self, username: &str) -> topcoat::Result<()> {
        sqlx::query("UPDATE background_loads SET state='failed',error_code='interrupted',finished_at=now() WHERE username=$1 AND state='running' AND heartbeat_at < now() - interval '60 seconds'")
            .bind(username.to_lowercase()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn begin_load(
        &self,
        username: &str,
        competition_id: Option<i64>,
    ) -> topcoat::Result<(LoadJob, bool)> {
        self.expire_loads(username).await?;
        let username = username.to_lowercase();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&username)
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query_as::<_, LoadJob>("SELECT id,competition_id,state,error_code FROM background_loads WHERE username=$1 AND competition_id IS NOT DISTINCT FROM $2 AND state='running'")
            .bind(&username).bind(competition_id).fetch_optional(&mut *tx).await?;
        if let Some(job) = existing {
            tx.commit().await?;
            return Ok((job, false));
        }
        let job = sqlx::query_as("INSERT INTO background_loads (id,username,competition_id,state) VALUES ($1,$2,$3,'running') RETURNING id,competition_id,state,error_code")
            .bind(Uuid::new_v4()).bind(username).bind(competition_id).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok((job, true))
    }

    pub async fn load_job(&self, username: &str, id: Uuid) -> topcoat::Result<Option<LoadJob>> {
        self.expire_loads(username).await?;
        Ok(sqlx::query_as("SELECT id,competition_id,state,error_code FROM background_loads WHERE username=$1 AND id=$2")
            .bind(username.to_lowercase()).bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn running_load(
        &self,
        username: &str,
        competition_id: Option<i64>,
    ) -> topcoat::Result<Option<LoadJob>> {
        self.expire_loads(username).await?;
        Ok(sqlx::query_as("SELECT id,competition_id,state,error_code FROM background_loads WHERE username=$1 AND competition_id IS NOT DISTINCT FROM $2 AND state='running'")
            .bind(username.to_lowercase()).bind(competition_id).fetch_optional(&self.pool).await?)
    }

    pub(crate) async fn touch_load(&self, id: Uuid) -> topcoat::Result<bool> {
        Ok(sqlx::query(
            "UPDATE background_loads SET heartbeat_at=now() WHERE id=$1 AND state='running'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    pub(crate) async fn finish_load(&self, id: Uuid, error: Option<&str>) -> topcoat::Result<()> {
        sqlx::query("UPDATE background_loads SET state=$2,error_code=$3,finished_at=now() WHERE id=$1 AND state='running'")
            .bind(id).bind(if error.is_some() {"failed"} else {"succeeded"}).bind(error).execute(&self.pool).await?;
        Ok(())
    }
}
