use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    competitions::Competition,
    photos::{NewPhoto, PhotoFilter},
    storage::SessionStore,
};
mod support;

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn current_assignments_are_scoped_and_filtered_before_pagination(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-session-secret-with-at-least-32-bytes");
    for username in ["judge@example.com", "other@example.com"] {
        store
            .save_competitions(
                username,
                &[
                    Competition::new(3910, "Test"),
                    Competition::new(3911, "Other"),
                ],
            )
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    // 23 fotos del mismo WOD/equipo y otra foto del mismo equipo en otro WOD.
    let mut ids = Vec::new();
    for index in 0..29 {
        let (username, competition, category, wod, team) = match index {
            0..=22 => ("judge@example.com", 3910, 16907, 64459, "298682"),
            23 => ("judge@example.com", 3910, 16907, 64460, "298682"),
            24 => ("judge@example.com", 3910, 16907, 64459, "gid/&= equipo"),
            25 => ("judge@example.com", 3910, 16908, 64459, "298682"),
            26 => ("other@example.com", 3910, 16907, 64459, "298682"),
            27 => ("judge@example.com", 3911, 16907, 64459, "298682"),
            _ => ("judge@example.com", 3910, 16907, 64459, "298682"),
        };
        let id = store
            .save_photo(NewPhoto {
                id: Uuid::new_v4(),
                username,
                competition_id: competition,
                session_id: Uuid::new_v4(),
                image: b"image",
                media_type: "image/png",
                width: 1,
                height: 1,
                model: "test",
            })
            .await?;
        let reading = json!({"category_id":category,"wod_id":wod,"team_reference":team});
        // La última foto tiene un plan antiguo pero ninguna asignación actual.
        sqlx::query("UPDATE score_photos SET state='incomplete',reading=$2,prepared=$3,created_at='2026-09-30T12:00:00Z'::timestamptz + $4::int * interval '1 day' WHERE id=$1")
            .bind(id).bind(if index==28 {None} else {Some(sqlx::types::Json(&reading))}).bind(sqlx::types::Json(&reading)).bind(index).execute(&pool).await?;
        ids.push(id);
    }
    let category = PhotoFilter {
        category_id: Some(16907),
        ..Default::default()
    };
    let first = store
        .filtered_photos("JUDGE@example.com", 3910, 0, &category)
        .await?;
    let second = store
        .filtered_photos("judge@example.com", 3910, 20, &category)
        .await?;
    assert_eq!(first.len(), 20);
    assert_eq!(second.len(), 5);
    // Orden cronológico real, también al cambiar de mes.
    assert_eq!(first[0].id, ids[24]);
    assert_eq!(second.last().unwrap().id, ids[0]);
    let all = first
        .into_iter()
        .chain(second)
        .map(|p| p.id)
        .collect::<Vec<_>>();
    assert!(ids[..25].iter().all(|id| all.contains(id)));
    assert!(ids[25..].iter().all(|id| !all.contains(id)));
    let wod = PhotoFilter {
        category_id: Some(16907),
        wod_id: Some(64459),
        ..Default::default()
    };
    let team = PhotoFilter {
        category_id: Some(16907),
        team_reference: Some("298682".into()),
        ..Default::default()
    };
    assert_eq!(
        store
            .filtered_photos("judge@example.com", 3910, 20, &wod)
            .await?
            .len(),
        4
    );
    assert_eq!(
        store
            .filtered_photos("judge@example.com", 3910, 20, &team)
            .await?
            .len(),
        4
    );
    let special = PhotoFilter {
        category_id: Some(16907),
        team_reference: Some("gid/&= equipo".into()),
        ..Default::default()
    };
    assert_eq!(
        store
            .filtered_photos("judge@example.com", 3910, 0, &special)
            .await?[0]
            .id,
        ids[24]
    );
    // Una corrección manual mueve inmediatamente la foto aunque prepared sea antiguo.
    sqlx::query(
        "UPDATE score_photos SET reading=jsonb_set(reading,'{category_id}','16908') WHERE id=$1",
    )
    .bind(ids[24])
    .execute(&pool)
    .await?;
    assert!(
        store
            .filtered_photos("judge@example.com", 3910, 0, &special)
            .await?
            .is_empty()
    );
    let moved = PhotoFilter {
        category_id: Some(16908),
        team_reference: Some("gid/&= equipo".into()),
        ..Default::default()
    };
    assert_eq!(
        store
            .filtered_photos("judge@example.com", 3910, 0, &moved)
            .await?[0]
            .id,
        ids[24]
    );
    // La vista de incidencias incluye tanto errores como avisos y asignaciones inválidas.
    let mut detail = support::fixture();
    let mut category = detail.categories[0].clone();
    category.id = 16908;
    detail.categories.push(category);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    sqlx::query("UPDATE score_photos SET state='succeeded'")
        .execute(&pool)
        .await?;
    for (index, state) in [
        (0, "incomplete"),
        (1, "failed"),
        (2, "unconfirmed"),
        (3, "saved_local_failed"),
        (26, "failed"),
        (27, "failed"),
    ] {
        sqlx::query("UPDATE score_photos SET state=$2 WHERE id=$1")
            .bind(ids[index])
            .bind(state)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "UPDATE score_photos SET reading=jsonb_set(reading,'{needs_retake}','true') WHERE id=$1",
    )
    .bind(ids[4])
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE score_photos SET validation_errors='[\"Puntuación inválida\"]' WHERE id=$1",
    )
    .bind(ids[5])
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE score_photos SET error='Error de procesamiento' WHERE id=$1")
        .bind(ids[6])
        .execute(&pool)
        .await?;
    sqlx::query(
        "UPDATE score_photos SET reading=jsonb_set(reading,'{category_id}','null') WHERE id=$1",
    )
    .bind(ids[7])
    .execute(&pool)
    .await?;
    sqlx::query(
        "UPDATE score_photos SET reading=jsonb_set(reading,'{category_id}','99999') WHERE id=$1",
    )
    .bind(ids[8])
    .execute(&pool)
    .await?;
    let issues = PhotoFilter {
        issues: true,
        ..Default::default()
    };
    let mut problematic = store
        .filtered_photos("judge@example.com", 3910, 0, &issues)
        .await?;
    problematic.extend(
        store
            .filtered_photos("judge@example.com", 3910, 20, &issues)
            .await?,
    );
    assert_eq!(problematic.len(), 27);
    for index in (0..=25).chain([28]) {
        assert!(problematic.iter().any(|photo| photo.id == ids[index]));
    }
    // Resolver un aviso no oculta fotos que siguen duplicadas o no son comparables.
    sqlx::query("UPDATE score_photos SET state='succeeded' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await?;
    sqlx::query(
        "UPDATE score_photos SET reading=jsonb_set(reading,'{needs_retake}','false') WHERE id=$1",
    )
    .bind(ids[4])
    .execute(&pool)
    .await?;
    let mut resolved = store
        .filtered_photos("judge@example.com", 3910, 0, &issues)
        .await?;
    resolved.extend(
        store
            .filtered_photos("judge@example.com", 3910, 20, &issues)
            .await?,
    );
    assert_eq!(resolved.len(), 27);
    assert!(
        resolved
            .iter()
            .any(|photo| photo.id == ids[0] && photo.duplicate_count > 1)
    );
    assert!(
        resolved
            .iter()
            .any(|photo| photo.id == ids[4] && photo.duplicate_count > 1)
    );
    assert!(
        !resolved
            .iter()
            .any(|photo| [ids[26], ids[27]].contains(&photo.id))
    );
    Ok(())
}
