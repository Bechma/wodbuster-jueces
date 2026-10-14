use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{browser::AuthCookie, storage::SessionStore};

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn persists_encrypted_cookie_and_metadata(pool: PgPool) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-session-secret-with-at-least-32-bytes");
    let id = Uuid::new_v4();
    let cookie = AuthCookie {
        name: ".WBAuth".into(),
        value: "sensitive-test-cookie".into(),
        domain: ".wodbuster.com".into(),
        path: "/".into(),
        expires: -1.0,
        http_only: true,
        secure: true,
        session: true,
        same_site: Some("Lax".into()),
    };
    store.save(id, "test@example.com", &cookie).await?;
    // Otra instancia representa una nueva petición o un reinicio del servidor.
    let reopened = SessionStore::new(pool.clone(), "test-session-secret-with-at-least-32-bytes");
    let saved = reopened.load(id).await?.expect("Sesión guardada");
    assert_eq!(saved.username, "test@example.com");
    assert!(saved.cookie == cookie);
    let encrypted: Vec<u8> =
        sqlx::query_scalar("SELECT auth_cookie FROM wodbuster_sessions WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert!(
        !encrypted
            .windows(cookie.value.len())
            .any(|value| value == cookie.value.as_bytes())
    );
    assert!(reopened.load(Uuid::new_v4()).await?.is_none());
    let wrong_key = SessionStore::new(pool.clone(), "different-secret-with-at-least-32-bytes");
    assert!(wrong_key.load(id).await.is_err());
    let mut updated = cookie.clone();
    updated.value = "updated-auth-cookie".into();
    store.save(id, "test@example.com", &updated).await?;
    assert!(store.load(id).await?.unwrap().cookie == updated);
    updated.session = false;
    updated.expires = 1.0;
    store.save(id, "test@example.com", &updated).await?;
    assert!(store.load(id).await?.is_none());
    // Impide mover un ciphertext a otra sesión: el id forma parte del AAD.
    let other = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO wodbuster_sessions (id, username, auth_cookie) VALUES ($1, 'other', $2)",
    )
    .bind(other)
    .bind(encrypted)
    .execute(&pool)
    .await?;
    assert!(store.load(other).await.is_err());
    Ok(())
}
