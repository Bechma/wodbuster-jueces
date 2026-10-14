use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::browser::AuthCookie;

pub struct SessionStore {
    pool: PgPool,
    cipher: Aes256Gcm,
}

pub struct SavedSession {
    pub username: String,
    pub cookie: AuthCookie,
}

impl SessionStore {
    pub fn new(pool: PgPool, secret: &str) -> Self {
        let key = Sha256::digest([b"wodbuster-cookie-v1:".as_slice(), secret.as_bytes()].concat());
        Self {
            pool,
            cipher: Aes256Gcm::new(&key),
        }
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
