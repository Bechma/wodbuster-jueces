use super::*;
use crate::{
    photos::{NewPhoto, Photo},
    scorecard::ExtractionResult,
};
use sqlx::{Connection, PgConnection};

const FIELDS: &str = "id,username,competition_id,session_id,media_type,width,height,octet_length(image)::int AS bytes,provider,model,state,revision,reading,prepared,validation_errors,error,to_char(created_at AT TIME ZONE 'Europe/Madrid','DD/MM/YYYY HH24:MI:SS') AS created_at";

impl SessionStore {
    pub async fn save_photo(&self, new: NewPhoto<'_>) -> sqlx::Result<Uuid> {
        // El UUID de la captura hace idempotente una repetición de la petición HTTP.
        sqlx::query("INSERT INTO score_photos (id,username,competition_id,session_id,image,media_type,width,height,provider,model) VALUES ($1,lower($2),$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (id) DO NOTHING")
            .bind(new.id).bind(new.username).bind(new.competition_id).bind(new.session_id)
            .bind(new.image).bind(new.media_type).bind(new.width).bind(new.height)
            .bind(crate::ai::PROVIDER).bind(new.model).execute(&self.pool).await?;
        sqlx::query_scalar(
            "SELECT id FROM score_photos WHERE id=$1 AND username=lower($2) AND competition_id=$3",
        )
        .bind(new.id)
        .bind(new.username)
        .bind(new.competition_id)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn photos(
        &self,
        username: &str,
        competition: i64,
        offset: i64,
    ) -> sqlx::Result<Vec<Photo>> {
        sqlx::query_as(&format!("SELECT {FIELDS} FROM score_photos WHERE username=lower($1) AND competition_id=$2 ORDER BY created_at DESC,id DESC LIMIT 20 OFFSET $3"))
            .bind(username).bind(competition).bind(offset).fetch_all(&self.pool).await
    }

    pub async fn photo_image(
        &self,
        username: &str,
        competition: i64,
        id: Uuid,
    ) -> sqlx::Result<Option<(Vec<u8>, String)>> {
        sqlx::query_as("SELECT image,media_type FROM score_photos WHERE username=lower($1) AND competition_id=$2 AND id=$3")
            .bind(username).bind(competition).bind(id).fetch_optional(&self.pool).await
    }

    pub async fn photo(
        &self,
        username: &str,
        competition: i64,
        id: Uuid,
    ) -> sqlx::Result<Option<Photo>> {
        sqlx::query_as(&format!("SELECT {FIELDS} FROM score_photos WHERE username=lower($1) AND competition_id=$2 AND id=$3"))
            .bind(username).bind(competition).bind(id).fetch_optional(&self.pool).await
    }

    /// El estado y la revisión se comparan en la misma escritura que encola el trabajo.
    /// Un worker activo o una segunda pestaña nunca pierden sus datos por esta acción.
    pub async fn requeue_photo(
        &self,
        photo: &Photo,
        session_id: Uuid,
        extraction: Option<&ExtractionResult>,
        model: &str,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query("UPDATE score_photos SET revision=revision+1,session_id=$4,state=$5,reading=$6,prepared=$7,validation_errors='[]',error=NULL,result=NULL,model=$8,provider=CASE WHEN $5='queued' THEN $10 ELSE provider END,updated_at=now() WHERE id=$1 AND username=$2 AND competition_id=$3 AND revision=$9 AND state NOT IN ('queued','parsing','parsed','submitting')")
            .bind(photo.id).bind(&photo.username).bind(photo.competition_id).bind(session_id)
            .bind(if extraction.is_some() { "parsed" } else { "queued" })
            .bind(extraction.map(|e| Json(&e.reading))).bind(extraction.and_then(|e|e.prepared.as_ref()).map(Json))
            .bind(model).bind(photo.revision).bind(crate::ai::PROVIDER).execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Conexión dedicada: al caer/cancelarse el worker, se cierra y libera el bloqueo.
    pub(crate) async fn photo_worker_lock(&self) -> sqlx::Result<Option<PgConnection>> {
        let mut connection = PgConnection::connect_with(&self.pool.connect_options()).await?;
        let acquired: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock(24561278592898127::bigint)")
                .fetch_one(&mut connection)
                .await?;
        Ok(acquired.then_some(connection))
    }

    pub(crate) async fn recover_photos(&self) -> sqlx::Result<()> {
        sqlx::query("UPDATE score_photos SET state=CASE WHEN state='parsing' THEN 'queued' ELSE 'unconfirmed' END, error=CASE WHEN state='submitting' THEN 'El servidor se interrumpió durante el envío. Comprueba Arena antes de repetirlo.' ELSE NULL END,updated_at=now() WHERE state IN ('parsing','submitting')")
            .execute(&self.pool).await?;
        Ok(())
    }

    pub(crate) async fn claim_photo(&self) -> sqlx::Result<Option<Photo>> {
        sqlx::query_as(&format!("UPDATE score_photos SET model=CASE WHEN state='queued' AND provider<>$1 THEN $2 ELSE model END,provider=CASE WHEN state='queued' THEN $1 ELSE provider END,state=CASE WHEN state='queued' THEN 'parsing' ELSE 'submitting' END,updated_at=now() WHERE id=(SELECT id FROM score_photos WHERE state IN ('queued','parsed') ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING {FIELDS}"))
            .bind(crate::ai::PROVIDER).bind(crate::ai::model_name())
            .fetch_optional(&self.pool).await
    }

    pub(crate) async fn parsed_photo(
        &self,
        id: Uuid,
        result: &ExtractionResult,
    ) -> sqlx::Result<()> {
        sqlx::query("UPDATE score_photos SET state=$2,reading=$3,prepared=$4,validation_errors=$5,error=NULL,updated_at=now() WHERE id=$1")
            .bind(id).bind(if result.ready_for_form {"parsed"} else {"incomplete"})
            .bind(Json(&result.reading)).bind(result.prepared.as_ref().map(Json))
            .bind(Json(&result.validation_errors)).execute(&self.pool).await?;
        Ok(())
    }

    pub(crate) async fn finish_photo(
        &self,
        id: Uuid,
        state: &str,
        error: Option<&str>,
        score: Option<&crate::details::Score>,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE score_photos SET state=$2,error=$3,result=$4,updated_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(state)
        .bind(error)
        .bind(score.map(Json))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn fail_photo(&self, id: Uuid, state: &str, error: &str) -> sqlx::Result<()> {
        // Un fallo posterior a la confirmación remota nunca borra esa confirmación.
        sqlx::query("UPDATE score_photos SET state=CASE WHEN result IS NOT NULL THEN 'saved_local_failed' ELSE $2 END,error=$3,updated_at=now() WHERE id=$1")
            .bind(id).bind(state).bind(error).execute(&self.pool).await?;
        Ok(())
    }
}
