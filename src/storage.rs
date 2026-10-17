use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::browser::AuthCookie;
use crate::competitions::Competition;
use crate::details::CompetitionDetail;
use sqlx::types::Json;
mod loads;
mod photos;
mod submission_blocks;

pub struct SessionStore {
    pool: PgPool,
    cipher: Aes256Gcm,
}

pub struct SavedSession {
    pub username: String,
    pub cookie: AuthCookie,
}

impl SessionStore {
    pub async fn competitions(&self, username: &str) -> topcoat::Result<Vec<Competition>> {
        Ok(sqlx::query_as(
            "SELECT id, name, judge_url, admin_url FROM wodbuster_competitions WHERE username = $1 ORDER BY name, id",
        )
        .bind(username.to_lowercase())
        .fetch_all(&self.pool)
        .await?)
    }

    /// Reemplaza la lista solo después de una carga autenticada y válida.
    pub async fn save_competitions(
        &self,
        username: &str,
        competitions: &[Competition],
    ) -> topcoat::Result<()> {
        let username = username.to_lowercase();
        let mut transaction = self.pool.begin().await?;
        // Serializa las cargas de una misma cuenta, sin bloquear las de otros usuarios.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&username)
            .execute(&mut *transaction)
            .await?;
        let ids = competitions.iter().map(|c| c.id).collect::<Vec<_>>();
        sqlx::query(
            "DELETE FROM wodbuster_competitions WHERE username = $1 AND NOT (id = ANY($2))",
        )
        .bind(&username)
        .bind(ids)
        .execute(&mut *transaction)
        .await?;
        for competition in competitions {
            sqlx::query(
                "INSERT INTO wodbuster_competitions (username, id, name, judge_url, admin_url) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (username, id) DO UPDATE SET name=EXCLUDED.name, judge_url=EXCLUDED.judge_url, admin_url=EXCLUDED.admin_url",
            )
            .bind(&username)
            .bind(competition.id)
            .bind(&competition.name)
            .bind(&competition.judge_url).bind(&competition.admin_url)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Borra la información cargada de la competición para esta cuenta; conserva las fotos.
    pub async fn delete_competition(&self, username: &str, id: i64) -> topcoat::Result<bool> {
        let username = username.to_lowercase();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&username)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query("DELETE FROM wodbuster_competitions WHERE username=$1 AND id=$2")
            .bind(&username)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        // Las claves foráneas borran el detalle, categorías, equipos, WODs y puntuaciones.
        // Las fotos no dependen de esta caché y conservan su cuenta y competition_id.
        tx.commit().await?;
        Ok(deleted)
    }

    pub async fn competition_detail(
        &self,
        username: &str,
        id: i64,
    ) -> topcoat::Result<Option<CompetitionDetail>> {
        let row = sqlx::query_as::<_,(Json<CompetitionDetail>,String)>(
            "SELECT snapshot, to_char(loaded_at AT TIME ZONE 'Europe/Madrid', 'DD/MM/YYYY HH24:MI') FROM competition_details WHERE username=$1 AND competition_id=$2")
            .bind(username.to_lowercase()).bind(id).fetch_optional(&self.pool).await?;
        Ok(row.map(|(Json(mut snapshot), loaded_at)| {
            crate::scoring::prepare_detail(&mut snapshot);
            snapshot.loaded_at = Some(loaded_at);
            snapshot
        }))
    }

    pub async fn save_competition_detail(
        &self,
        username: &str,
        detail: &CompetitionDetail,
    ) -> topcoat::Result<()> {
        let mut prepared = detail.clone();
        crate::scoring::prepare_detail(&mut prepared);
        let detail = &prepared;
        let username = username.to_lowercase();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&username)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM competition_details WHERE username=$1 AND competition_id=$2")
            .bind(&username)
            .bind(detail.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO competition_details (username,competition_id,snapshot) VALUES ($1,$2,$3)",
        )
        .bind(&username)
        .bind(detail.id)
        .bind(Json(detail))
        .execute(&mut *tx)
        .await?;
        for category in &detail.categories {
            sqlx::query("INSERT INTO competition_categories (username,competition_id,id,name,metadata) VALUES ($1,$2,$3,$4,$5)")
                .bind(&username).bind(detail.id).bind(category.id).bind(&category.name).bind(Json(&category.metadata)).execute(&mut *tx).await?;
            for wod in &category.wods {
                sqlx::query("INSERT INTO competition_wods (username,competition_id,category_id,id,name,data) VALUES ($1,$2,$3,$4,$5,$6)")
                    .bind(&username).bind(detail.id).bind(category.id).bind(wod.id).bind(&wod.name).bind(Json(wod)).execute(&mut *tx).await?;
            }
            for team in &category.teams {
                sqlx::query("INSERT INTO competition_teams (username,competition_id,category_id,reference,registration_id,gid,name,data) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
                    .bind(&username).bind(detail.id).bind(category.id).bind(&team.reference).bind(team.registration_id).bind(&team.gid).bind(&team.name).bind(Json(team)).execute(&mut *tx).await?;
                for score in &team.scores {
                    sqlx::query("INSERT INTO competition_scores (username,competition_id,category_id,team_reference,wod_id,result,rank,points,data) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                        .bind(&username).bind(detail.id).bind(category.id).bind(&team.reference).bind(score.wod_id).bind(&score.result).bind(&score.rank).bind(&score.points).bind(Json(score)).execute(&mut *tx).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub fn new(pool: PgPool, secret: &str) -> Self {
        let key = Sha256::digest([b"wodbuster-cookie-v1:".as_slice(), secret.as_bytes()].concat());
        Self {
            pool,
            cipher: Aes256Gcm::new(&key),
        }
    }

    /// Actualiza solo la puntuación confirmada, conservando el resto del snapshot más reciente.
    pub async fn save_confirmed_score(
        &self,
        username: &str,
        competition_id: i64,
        category_id: i64,
        team_reference: &str,
        score: &crate::details::Score,
    ) -> topcoat::Result<()> {
        let username = username.to_lowercase();
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&username)
            .execute(&mut *tx)
            .await?;
        Self::write_confirmed_score(
            &mut tx,
            &username,
            competition_id,
            category_id,
            team_reference,
            score,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// La copia local y el fin del trabajo pertenecen a la misma revisión y transacción.
    pub(crate) async fn save_confirmed_photo_score(
        &self,
        photo: &crate::photos::Photo,
        category_id: i64,
        team_reference: &str,
        score: &crate::details::Score,
    ) -> topcoat::Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&photo.username)
            .execute(&mut *tx)
            .await?;
        let active: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM score_photos WHERE id=$1 AND revision=$2 AND state='submitting' AND result IS NOT NULL FOR UPDATE",
        ).bind(photo.id).bind(photo.revision).fetch_optional(&mut *tx).await?;
        if active.is_none() {
            return Ok(false);
        }
        Self::write_confirmed_score(
            &mut tx,
            &photo.username,
            photo.competition_id,
            category_id,
            team_reference,
            score,
        )
        .await?;
        sqlx::query(
            "UPDATE score_photos SET state=CASE WHEN reading ? 'results' AND jsonb_array_length(result)<jsonb_array_length(reading->'results') THEN 'submitting' ELSE 'succeeded' END,error=NULL,updated_at=now() WHERE id=$1",
        )
        .bind(photo.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn write_confirmed_score(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        username: &str,
        competition_id: i64,
        category_id: i64,
        team_reference: &str,
        score: &crate::details::Score,
    ) -> topcoat::Result<()> {
        let snapshot = sqlx::query_scalar::<_, Json<CompetitionDetail>>(
            "SELECT snapshot FROM competition_details WHERE username=$1 AND competition_id=$2 FOR UPDATE",
        ).bind(username).bind(competition_id).fetch_optional(&mut **tx).await?;
        let mut detail = snapshot
            .ok_or_else(|| topcoat::Error::msg("La competición ya no está cargada"))?
            .0;
        let previous = detail
            .categories
            .iter_mut()
            .find(|c| c.id == category_id)
            .and_then(|c| c.teams.iter_mut().find(|t| t.reference == team_reference))
            .and_then(|t| t.scores.iter_mut().find(|s| s.wod_id == score.wod_id))
            .ok_or_else(|| {
                topcoat::Error::msg("La puntuación ya no está en la competición cargada")
            })?;
        let mut confirmed = score.clone();
        let mut metadata = previous.metadata.clone();
        if let (Some(old), Some(new)) = (metadata.as_object_mut(), score.metadata.as_object()) {
            old.extend(new.clone());
            confirmed.metadata = metadata;
        }
        *previous = confirmed.clone();
        let updated = sqlx::query("UPDATE competition_scores SET result=$6, rank=$7, points=$8, data=$9 WHERE username=$1 AND competition_id=$2 AND category_id=$3 AND team_reference=$4 AND wod_id=$5")
            .bind(username).bind(competition_id).bind(category_id).bind(team_reference).bind(score.wod_id)
            .bind(&confirmed.result).bind(&confirmed.rank).bind(&confirmed.points).bind(Json(&confirmed))
            .execute(&mut **tx).await?;
        if updated.rows_affected() != 1 {
            return Err(topcoat::Error::msg(
                "No se encontró la puntuación local para actualizar",
            ));
        }
        sqlx::query(
            "UPDATE competition_details SET snapshot=$3 WHERE username=$1 AND competition_id=$2",
        )
        .bind(username)
        .bind(competition_id)
        .bind(Json(detail))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn save(&self, id: Uuid, username: &str, cookie: &AuthCookie) -> topcoat::Result<()> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let json = serde_json::to_vec(cookie)?;
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &json,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| topcoat::Error::msg("No se pudo cifrar la cookie"))?;
        let encrypted = [nonce.as_slice(), &ciphertext].concat();
        sqlx::query(
            "INSERT INTO wodbuster_sessions (id, username, auth_cookie) VALUES ($1, $2, $3)
                     ON CONFLICT (id) DO UPDATE SET username = EXCLUDED.username,
                     auth_cookie = EXCLUDED.auth_cookie, saved_at = now()",
        )
        .bind(id)
        .bind(username)
        .bind(encrypted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Recupera la cookie y sus atributos para las próximas operaciones en Arena.
    pub async fn load(&self, id: Uuid) -> topcoat::Result<Option<SavedSession>> {
        let Some((username, encrypted)) = sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT username, auth_cookie FROM wodbuster_sessions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        if encrypted.len() < 12 {
            return Err(topcoat::Error::msg("Cookie almacenada no válida"));
        }
        let json = self
            .cipher
            .decrypt(
                Nonce::from_slice(&encrypted[..12]),
                Payload {
                    msg: &encrypted[12..],
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| topcoat::Error::msg("No se pudo descifrar la cookie almacenada"))?;
        let cookie: AuthCookie = serde_json::from_slice(&json)?;
        if !cookie.is_valid() {
            return Ok(None);
        }
        Ok(Some(SavedSession { username, cookie }))
    }
}
