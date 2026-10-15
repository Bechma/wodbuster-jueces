use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{competitions::Competition, photos::NewPhoto, storage::SessionStore};
mod support;

async fn photo_snapshot(pool: &PgPool) -> sqlx::Result<Vec<Value>> {
    sqlx::query_scalar("SELECT to_jsonb(p) FROM score_photos p ORDER BY id")
        .fetch_all(pool)
        .await
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml y DATABASE_URL"]
async fn migration_deletion_and_refresh_preserve_every_photo_and_its_processing_data(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    let competitions = [
        Competition::new(3910, "Selected"),
        Competition::new(1246, "Kept"),
    ];
    store
        .save_competitions("judge@example.com", &competitions)
        .await?;
    store
        .save_competitions("other@example.com", &competitions[..1])
        .await?;
    store
        .save_competition_detail("judge@example.com", &support::fixture())
        .await?;
    let image = include_bytes!("../examples/poc_agent.png");
    let states = [
        "queued",
        "parsing",
        "parsed",
        "submitting",
        "succeeded",
        "incomplete",
        "failed",
        "unconfirmed",
        "saved_local_failed",
    ];
    let mut selected_ids = Vec::new();
    for (username, competition, state) in states
        .iter()
        .map(|state| ("judge@example.com", 3910, *state))
        .chain([
            ("judge@example.com", 1246, "succeeded"),
            ("other@example.com", 3910, "succeeded"),
        ])
    {
        let id = store
            .save_photo(NewPhoto {
                id: Uuid::new_v4(),
                username,
                competition_id: competition,
                session_id: Uuid::new_v4(),
                image,
                media_type: "image/png",
                width: 100,
                height: 100,
                model: "original-model",
            })
            .await?;
        sqlx::query("UPDATE score_photos SET state=$2,reading=$3,prepared=$4,validation_errors=$5,result=$6,error=$7,revision=3 WHERE id=$1")
            .bind(id).bind(state)
            .bind(sqlx::types::Json(json!({"category_id":16907,"wod_id":64459,"team_reference":"298682","score_value":{"kind":"not_completed","repetitions":2},"needs_retake":true})))
            .bind(sqlx::types::Json(json!({"category_name":"RX","wod_name":"Giralda","team_name":"Original team","form":{"url":"original-form"}})))
            .bind(sqlx::types::Json(json!(["Original validation message"])))
            .bind(sqlx::types::Json(json!({"result":"2 reps"})))
            .bind("Original error").execute(&pool).await?;
        if username == "judge@example.com" && competition == 3910 {
            selected_ids.push(id);
        }
    }
    let expected = photo_snapshot(&pool).await?;
    assert_eq!(expected.len(), 11);
    // Simula la actualización de una base antigua con fotos ya guardadas.
    sqlx::query("ALTER TABLE score_photos ADD CONSTRAINT score_photos_username_competition_id_fkey FOREIGN KEY (username,competition_id) REFERENCES wodbuster_competitions(username,id) ON DELETE CASCADE")
        .execute(&pool).await?;
    sqlx::raw_sql(include_str!(
        "../migrations/202610150004_preserve_score_photos.sql"
    ))
    .execute(&pool)
    .await?;
    assert!(
        photo_snapshot(&pool).await? == expected,
        "La migración debe conservar las fotos existentes"
    );

    // Refrescar una lista que aún incluye la competición conserva todas las fotos.
    store
        .save_competitions("judge@example.com", &competitions)
        .await?;
    assert!(photo_snapshot(&pool).await? == expected);
    // Refrescar detalles sustituye categorías/equipos/WODs sin tocar sus fotos.
    let mut updated = support::fixture();
    updated.categories.clear();
    store
        .save_competition_detail("judge@example.com", &updated)
        .await?;
    assert!(photo_snapshot(&pool).await? == expected);

    assert!(store.delete_competition("JUDGE@example.com", 3910).await?);
    assert!(
        store
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_none()
    );
    assert!(
        photo_snapshot(&pool).await? == expected,
        "Borrar la competición no debe borrar ni modificar las fotos"
    );
    // Recargar la competición devuelve las mismas imágenes e identificadores.
    store
        .save_competitions("judge@example.com", &competitions)
        .await?;
    store
        .save_competition_detail("judge@example.com", &support::fixture())
        .await?;
    let photos = store.photos("judge@example.com", 3910, 0).await?;
    assert_eq!(photos.len(), states.len());
    assert!(
        selected_ids
            .iter()
            .all(|id| photos.iter().any(|photo| photo.id == *id))
    );
    for id in selected_ids {
        assert!(
            store
                .photo_image("judge@example.com", 3910, id)
                .await?
                .unwrap()
                .0
                == image
        );
    }
    assert!(photo_snapshot(&pool).await? == expected);

    // Arena puede omitir una competición al refrescar, o devolver la lista vacía.
    store
        .save_competitions("judge@example.com", &competitions[1..])
        .await?;
    assert!(
        store
            .competition_detail("judge@example.com", 3910)
            .await?
            .is_none()
    );
    assert!(photo_snapshot(&pool).await? == expected);
    store.save_competitions("judge@example.com", &[]).await?;
    assert!(store.competitions("judge@example.com").await?.is_empty());
    assert!(photo_snapshot(&pool).await? == expected);
    // Otras cuentas conservan su información y sus fotos.
    assert_eq!(store.competitions("other@example.com").await?.len(), 1);
    assert_eq!(store.photos("other@example.com", 3910, 0).await?.len(), 1);
    Ok(())
}
