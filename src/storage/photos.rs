use super::*;
use crate::{
    photos::{NewPhoto, Photo, PhotoFilter},
    scorecard::ExtractionResult,
};
use sqlx::{Connection, PgConnection};

const FIELDS: &str = "id,username,competition_id,session_id,media_type,width,height,octet_length(image)::int AS bytes,provider,model,state,revision,reading,prepared,validation_errors,error,result,expected_results,to_char(created_at AT TIME ZONE 'Europe/Madrid','DD/MM/YYYY HH24:MI:SS') AS created_at";
const CHECK_FIELDS: &str =
    "duplicate_count,is_latest,sync_status,sync_checked_at,arena_value,result_checks";

#[derive(sqlx::FromRow)]
pub struct PhotoCheckSummary {
    pub total: i64,
    pub synced: i64,
    pub different: i64,
    pub duplicates: i64,
    pub unavailable: i64,
}

impl SessionStore {
    pub async fn requeue_scorecard(
        &self,
        photo: &Photo,
        session_id: Uuid,
        extraction: &crate::scorecard::ScorecardExtraction,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query("UPDATE score_photos SET revision=revision+1,session_id=$4,state='parsed',expected_results=$8,reading=$5,prepared=$6,validation_errors='[]',error=NULL,result=NULL,updated_at=now() WHERE id=$1 AND username=$2 AND competition_id=$3 AND revision=$7 AND state NOT IN ('queued','parsing','parsed','submitting')")
            .bind(photo.id).bind(&photo.username).bind(photo.competition_id).bind(session_id)
            .bind(Json(extraction.reading.stored_reading())).bind(extraction.stored_prepared().map(Json))
            .bind(photo.revision).bind(extraction.reading.results.len() as i16).execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn parsed_scorecard(
        &self,
        photo: &Photo,
        result: &crate::scorecard::ScorecardExtraction,
    ) -> sqlx::Result<bool> {
        if result.reading.results.len() == 1 {
            return self
                .parsed_photo(
                    photo,
                    &ExtractionResult {
                        reading: result.reading.readings().remove(0),
                        ready_for_form: result.ready_for_form,
                        prepared: result
                            .prepared
                            .as_ref()
                            .and_then(|plans| plans.first().cloned()),
                        validation_errors: result.validation_errors.clone(),
                    },
                )
                .await;
        }
        let updated = sqlx::query("UPDATE score_photos SET state=$2,reading=$3,prepared=$4,validation_errors=$5,error=NULL,updated_at=now() WHERE id=$1 AND revision=$6 AND state=$7 AND state IN ('parsing','submitting')")
            .bind(photo.id).bind(if result.ready_for_form {"parsed"} else {"incomplete"})
            .bind(Json(result.reading.stored_reading())).bind(result.stored_prepared().map(Json))
            .bind(Json(&result.validation_errors)).bind(photo.revision).bind(&photo.state)
            .execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn reject_blocked_scorecard(
        &self,
        photo: &Photo,
        reading: &crate::scorecard::Scorecard,
        reason: &str,
    ) -> sqlx::Result<bool> {
        if reading.results.len() == 1 {
            return self
                .reject_blocked_photo(photo, &reading.readings()[0], reason)
                .await;
        }
        let updated = sqlx::query("UPDATE score_photos SET state='failed',reading=$3,prepared=NULL,validation_errors=$4,error=$5,updated_at=now() WHERE id=$1 AND revision=$2 AND state=$6 AND state IN ('parsing','submitting')")
            .bind(photo.id).bind(photo.revision).bind(Json(reading.stored_reading())).bind(Json(vec![reason]))
            .bind(reason).bind(&photo.state).execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn save_photo(&self, new: NewPhoto<'_>) -> sqlx::Result<Uuid> {
        // El UUID de la captura hace idempotente una repetición de la petición HTTP.
        sqlx::query("INSERT INTO score_photos (id,username,competition_id,session_id,image,media_type,width,height,provider,model,expected_results) VALUES ($1,lower($2),$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (id) DO NOTHING")
            .bind(new.id).bind(new.username).bind(new.competition_id).bind(new.session_id)
            .bind(new.image).bind(new.media_type).bind(new.width).bind(new.height)
            .bind(crate::ai::PROVIDER).bind(new.model).bind(new.expected_results).execute(&self.pool).await?;
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
        self.filtered_photos(username, competition, offset, &PhotoFilter::default())
            .await
    }

    pub async fn filtered_photos(
        &self,
        username: &str,
        competition: i64,
        offset: i64,
        filter: &PhotoFilter,
    ) -> sqlx::Result<Vec<Photo>> {
        sqlx::query_as(&format!("SELECT {FIELDS},{CHECK_FIELDS} FROM score_photo_checks AS score_photos WHERE username=lower($1) AND competition_id=$2 AND ($4::text IS NULL OR reading->>'category_id'=$4) AND ($5::text IS NULL OR EXISTS (SELECT 1 FROM score_photo_result_checks rc WHERE rc.id=score_photos.id AND rc.wod_id=$5)) AND ($6::text IS NULL OR reading->>'team_reference'=$6)
            AND (NOT $7::boolean
                OR NOT EXISTS (SELECT 1 FROM competition_categories c WHERE c.username=score_photos.username AND c.competition_id=score_photos.competition_id AND c.id::text=score_photos.reading->>'category_id')
                OR state IN ('incomplete','failed','unconfirmed','saved_local_failed')
                OR COALESCE(error,'')<>'' OR validation_errors<>'[]'::jsonb
                OR reading->>'needs_retake'='true' OR duplicate_count>1
                OR sync_status IN ('different','unavailable'))
            AND (NOT $8::boolean OR sync_status='different')
            ORDER BY score_photos.created_at DESC,id DESC LIMIT 20 OFFSET $3"))
            .bind(username).bind(competition).bind(offset)
            .bind(filter.category_id.map(|id|id.to_string()))
            .bind(filter.wod_id.map(|id|id.to_string()))
            .bind(&filter.team_reference).bind(filter.issues).bind(filter.unsynced).fetch_all(&self.pool).await
    }

    pub async fn photo_check_summary(
        &self,
        username: &str,
        competition: i64,
    ) -> sqlx::Result<PhotoCheckSummary> {
        sqlx::query_as("SELECT count(*) AS total,count(*) FILTER (WHERE sync_status='synced') AS synced,count(*) FILTER (WHERE sync_status='different') AS different,count(*) FILTER (WHERE duplicate_count>1) AS duplicates,count(*) FILTER (WHERE sync_status IN ('unchecked','unavailable')) AS unavailable FROM score_photo_checks WHERE username=lower($1) AND competition_id=$2")
            .bind(username).bind(competition).fetch_one(&self.pool).await
    }

    pub async fn delete_photo(&self, photo: &Photo) -> sqlx::Result<bool> {
        Ok(sqlx::query("DELETE FROM score_photos WHERE id=$1 AND username=$2 AND competition_id=$3 AND revision=$4 AND state NOT IN ('queued','parsing','parsed','submitting')")
            .bind(photo.id).bind(&photo.username).bind(photo.competition_id).bind(photo.revision)
            .execute(&self.pool).await?.rows_affected()==1)
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
        // Invalida también un parser antiguo si la misma foto vuelve a parsing.
        sqlx::query("UPDATE score_photos SET revision=revision+1,state=CASE WHEN result IS NOT NULL AND (NOT reading ? 'results' OR jsonb_array_length(result)>=jsonb_array_length(reading->'results')) THEN 'saved_local_failed' WHEN state='parsing' THEN 'queued' ELSE 'unconfirmed' END,error=CASE WHEN result IS NOT NULL AND (NOT reading ? 'results' OR jsonb_array_length(result)>=jsonb_array_length(reading->'results')) THEN 'Arena confirmó la puntuación; se interrumpió la actualización local. Refresca la competición; no repitas el envío.' WHEN state='submitting' THEN 'El servidor se interrumpió durante el envío de la scorecard. Puede haber resultados guardados; comprueba Arena antes de repetirlo.' ELSE NULL END,updated_at=now() WHERE state IN ('parsing','submitting')")
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
        photo: &Photo,
        result: &ExtractionResult,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query("UPDATE score_photos SET state=$2,reading=$3,prepared=$4,validation_errors=$5,error=NULL,updated_at=now() WHERE id=$1 AND revision=$6 AND state=$7 AND state IN ('parsing','submitting')")
            .bind(photo.id).bind(if result.ready_for_form {"parsed"} else {"incomplete"})
            .bind(Json(&result.reading)).bind(result.prepared.as_ref().map(Json))
            .bind(Json(&result.validation_errors)).bind(photo.revision).bind(&photo.state)
            .execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn reject_blocked_photo(
        &self,
        photo: &Photo,
        reading: &crate::scorecard::ScorecardReading,
        reason: &str,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query("UPDATE score_photos SET state='failed',reading=$3,prepared=NULL,validation_errors=$4,error=$5,updated_at=now() WHERE id=$1 AND revision=$2 AND state=$6 AND state IN ('parsing','submitting')")
            .bind(photo.id).bind(photo.revision).bind(Json(reading)).bind(Json(vec![reason]))
            .bind(reason).bind(&photo.state).execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Conserva la confirmación remota sin liberar la foto para nuevas ediciones.
    pub(crate) async fn confirm_photo(
        &self,
        photo: &Photo,
        score: &crate::details::Score,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query("UPDATE score_photos SET result=CASE WHEN reading ? 'results' THEN COALESCE((SELECT jsonb_agg(saved.value) FROM jsonb_array_elements(COALESCE(result,'[]'::jsonb)) saved(value) WHERE saved.value->>'wod_id'<>$3::jsonb->>'wod_id'),'[]'::jsonb) || jsonb_build_array($3::jsonb) ELSE $3::jsonb END,updated_at=now() WHERE id=$1 AND revision=$2 AND state='submitting'")
            .bind(photo.id).bind(photo.revision).bind(Json(score))
            .execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn finish_photo(
        &self,
        photo: &Photo,
        state: &str,
        error: Option<&str>,
        score: Option<&crate::details::Score>,
    ) -> sqlx::Result<bool> {
        let updated = sqlx::query(
            "UPDATE score_photos SET state=$2,error=$3,result=COALESCE($4,result),updated_at=now() WHERE id=$1 AND revision=$5 AND state=$6 AND state='submitting'",
        )
        .bind(photo.id)
        .bind(state)
        .bind(error)
        .bind(score.map(Json))
        .bind(photo.revision)
        .bind(&photo.state)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub(crate) async fn fail_photo(
        &self,
        photo: &Photo,
        state: &str,
        error: &str,
    ) -> sqlx::Result<bool> {
        // Un fallo posterior a la confirmación remota nunca borra esa confirmación.
        let updated = sqlx::query("UPDATE score_photos SET state=CASE WHEN result IS NOT NULL AND $2<>'saved_local_failed' THEN CASE WHEN reading ? 'results' AND jsonb_array_length(result)<jsonb_array_length(reading->'results') THEN 'unconfirmed' ELSE 'saved_local_failed' END ELSE $2 END,error=$3,updated_at=now() WHERE id=$1 AND revision=$4 AND state=$5 AND state IN ('parsing','submitting')")
            .bind(photo.id).bind(state).bind(error).bind(photo.revision).bind(&photo.state)
            .execute(&self.pool).await?;
        Ok(updated.rows_affected() == 1)
    }
}
