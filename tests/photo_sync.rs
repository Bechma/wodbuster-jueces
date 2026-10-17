use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wodbuster_jueces::{
    competitions::Competition,
    photos::{NewPhoto, PhotoFilter},
    scoring::ScoreFormat,
    storage::SessionStore,
};
mod support;

fn field(name: &str, value: &str) -> Value {
    json!({"attributes":{"name":format!("panel${name}")},"value":value})
}

async fn add_photo(store: &SessionStore) -> sqlx::Result<Uuid> {
    store
        .save_photo(NewPhoto {
            id: Uuid::new_v4(),
            username: "judge@example.com",
            competition_id: 3910,
            session_id: Uuid::new_v4(),
            image: b"image",
            media_type: "image/png",
            width: 1,
            height: 1,
            model: "test",
            expected_results: 1,
        })
        .await
}

#[sqlx::test]
#[ignore = "Requiere Postgres de compose.dev.yml"]
async fn scores_duplicates_latest_photo_and_explicit_deletion_are_consistent(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-secret-with-at-least-32-bytes");
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Test")])
        .await?;
    let cases = [
        (
            json!([
                field("IoComplete", "False"),
                field("IoReps", "2"),
                field("IoDesempate", "03:21")
            ]),
            Some("2reps"),
            json!({"kind":"not_completed","repetitions":2,"tie_break":{"minutes":3,"seconds":21}}),
            "synced",
        ),
        (
            json!([field("IoComplete", "True"), field("IoTimeMMSS", "05:32")]),
            Some("05:32"),
            json!({"kind":"completed","time":{"minutes":5,"seconds":32},"tie_break":null}),
            "synced",
        ),
        (
            json!([field("IoWeight", "52.5")]),
            Some("52.5kg"),
            json!({"kind":"weight","kilograms":52.5}),
            "synced",
        ),
        (
            json!([field("IoReps", "0")]),
            Some("0reps"),
            json!({"kind":"repetitions","repetitions":0}),
            "synced",
        ),
        (
            json!([field("IoTimeMMSS", "09:10")]),
            Some("09:10"),
            json!({"kind":"time","time":{"minutes":9,"seconds":10}}),
            "synced",
        ),
        (
            json!([field("IoReps", "0")]),
            None,
            json!({"kind":"repetitions","repetitions":0}),
            "different",
        ),
        (
            json!([]),
            Some("unknown"),
            json!({"kind":"repetitions","repetitions":0}),
            "unavailable",
        ),
    ];
    let mut detail = support::fixture();
    let template = detail.categories[0].clone();
    detail.categories.clear();
    for (index, (fields, result, _, _)) in cases.iter().enumerate() {
        let mut category = template.clone();
        category.id += index as i64;
        category.wods[0].score_format = ScoreFormat::from_fields(fields);
        let score = &mut category.teams[0].scores[0];
        score.result = result.map(str::to_owned);
        score.metadata = json!({"fields":fields});
        detail.categories.push(category);
    }
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    let mut ids = Vec::new();
    for (index, (_, _, value, _)) in cases.iter().enumerate() {
        let category = 16907 + index as i64;
        let id = add_photo(&store).await?;
        sqlx::query("UPDATE score_photos SET state='succeeded',reading=$2 WHERE id=$1")
            .bind(id).bind(sqlx::types::Json(json!({"category_id":category,"wod_id":64459,"team_reference":"298682","score_value":value}))).execute(&pool).await?;
        ids.push(id);
    }
    let photos = store.photos("judge@example.com", 3910, 0).await?;
    for (index, (_, _, _, expected)) in cases.iter().enumerate() {
        let photo = photos.iter().find(|p| p.id == ids[index]).unwrap();
        assert_eq!(photo.sync_status, *expected, "case {index}");
        assert_eq!(photo.duplicate_count, 1);
        assert!(photo.sync_checked_at.is_some());
    }
    let summary = store.photo_check_summary("judge@example.com", 3910).await?;
    assert_eq!(summary.synced, 5);
    assert_eq!(summary.different, 1);
    assert_eq!(summary.unavailable, 1);
    let unsynced = PhotoFilter {
        unsynced: true,
        ..Default::default()
    };
    let different = store
        .filtered_photos("judge@example.com", 3910, 0, &unsynced)
        .await?;
    assert_eq!(different.len(), 1);
    assert_eq!(different[0].id, ids[5]);

    // Un cambio de Arena se detecta tras refrescar su detalle; no se modifica la foto.
    detail.categories[3].teams[0].scores[0].result = Some("1reps".into());
    detail.categories[3].teams[0].scores[0].metadata = json!({"fields":[field("IoReps","1")]});
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    assert_eq!(
        store
            .photos("judge@example.com", 3910, 0)
            .await?
            .iter()
            .find(|p| p.id == ids[3])
            .unwrap()
            .sync_status,
        "different"
    );

    // El primer grupo cruza el límite de 20 fotos por página.
    let mut duplicates = vec![ids[0]];
    for index in 0..25 {
        let value = cases[0].2.clone();
        let id = add_photo(&store).await?;
        sqlx::query("UPDATE score_photos SET state='succeeded',reading=$2,created_at='2030-01-01T00:00:00Z'::timestamptz + $3::int * interval '1 second' WHERE id=$1")
            .bind(id).bind(sqlx::types::Json(json!({"category_id":16907,"wod_id":64459,"team_reference":"298682","score_value":value}))).bind(index).execute(&pool).await?;
        duplicates.push(id);
    }
    let latest = *duplicates.last().unwrap();
    sqlx::query("UPDATE score_photos SET reading=jsonb_set(reading,'{score_value,repetitions}','5') WHERE id=$1").bind(latest).execute(&pool).await?;
    // Actualizar una foto antigua no la convierte en la última agregada.
    sqlx::query("UPDATE score_photos SET updated_at='2040-01-01' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await?;
    let filter = PhotoFilter {
        category_id: Some(16907),
        ..Default::default()
    };
    let mut group = store
        .filtered_photos("judge@example.com", 3910, 0, &filter)
        .await?;
    group.extend(
        store
            .filtered_photos("judge@example.com", 3910, 20, &filter)
            .await?,
    );
    assert_eq!(group.len(), 26);
    assert!(group.iter().all(|p| p.duplicate_count == 26));
    assert_eq!(group.iter().filter(|p| p.is_latest).count(), 1);
    assert!(group.iter().all(|p| p.sync_status
        == if p.id == latest {
            "different"
        } else {
            "superseded"
        }));
    let issues = PhotoFilter {
        category_id: Some(16907),
        issues: true,
        ..Default::default()
    };
    assert_eq!(
        store
            .filtered_photos("judge@example.com", 3910, 20, &issues)
            .await?
            .len(),
        6
    );

    let different = store
        .filtered_photos("judge@example.com", 3910, 0, &unsynced)
        .await?;
    assert_eq!(different.len(), 3);
    assert!(
        different
            .iter()
            .all(|p| [latest, ids[3], ids[5]].contains(&p.id))
    );
    assert!(
        different
            .iter()
            .all(|p| p.is_latest && p.sync_status == "different")
    );
    let latest_photo = store
        .photo("judge@example.com", 3910, latest)
        .await?
        .unwrap();
    assert!(store.delete_photo(&latest_photo).await?);
    assert!(
        store
            .photo_image("judge@example.com", 3910, latest)
            .await?
            .is_none()
    );
    let remaining = store
        .filtered_photos("judge@example.com", 3910, 0, &filter)
        .await?;
    assert_eq!(remaining[0].id, duplicates[24]);
    assert!(remaining[0].is_latest);
    assert_eq!(remaining[0].sync_status, "synced");
    assert!(remaining.iter().all(|p| p.duplicate_count == 25));
    let different = store
        .filtered_photos("judge@example.com", 3910, 0, &unsynced)
        .await?;
    assert_eq!(different.len(), 2);
    assert!(different.iter().all(|p| [ids[3], ids[5]].contains(&p.id)));
    assert_eq!(
        store
            .competition_detail("judge@example.com", 3910)
            .await?
            .unwrap()
            .categories[0]
            .teams[0]
            .scores[0]
            .result
            .as_deref(),
        Some("2reps")
    );
    // Un trabajo activo o una revisión antigua no se puede borrar.
    let id = duplicates[24];
    let stale = store.photo("judge@example.com", 3910, id).await?.unwrap();
    sqlx::query("UPDATE score_photos SET revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    assert!(!store.delete_photo(&stale).await?);
    sqlx::query("UPDATE score_photos SET state='submitting' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let busy = store.photo("judge@example.com", 3910, id).await?.unwrap();
    assert!(!store.delete_photo(&busy).await?);
    Ok(())
}

#[sqlx::test]
#[ignore = "Solo Postgres local; sin IA ni Arena"]
async fn multiple_wods_filter_compare_and_resolve_duplicates_as_one_photo(
    pool: PgPool,
) -> topcoat::Result<()> {
    let store = SessionStore::new(pool.clone(), "test-multiple-scorecards-secret");
    store
        .save_competitions("judge@example.com", &[Competition::new(3910, "Test")])
        .await?;
    let mut detail = support::fixture();
    detail.categories[0].wods[0].score_format =
        ScoreFormat::from_fields(&json!([field("IoReps", "2")]));
    let mut wod = detail.categories[0].wods[0].clone();
    wod.id = 64460;
    wod.name = "Segundo".into();
    detail.categories[0].wods.push(wod);
    let first = &mut detail.categories[0].teams[0].scores[0];
    first.result = Some("2reps".into());
    first.metadata = json!({"fields":[field("IoReps", "2")]});
    let mut second = first.clone();
    second.wod_id = 64460;
    second.result = Some("7reps".into());
    second.metadata = json!({"fields":[field("IoReps", "7")]});
    detail.categories[0].teams[0].scores.push(second);
    store
        .save_competition_detail("judge@example.com", &detail)
        .await?;
    let reading = json!({"category_id":16907,"team_reference":"298682","judge_name":"Ana",
        "results":[{"wod_id":64459,"score_value":{"kind":"repetitions","repetitions":2}},
                   {"wod_id":64460,"score_value":{"kind":"repetitions","repetitions":7}}],
        "needs_retake":false,"retake_reason":null});
    let old = add_photo(&store).await?;
    sqlx::query(
        "UPDATE score_photos SET reading=$2,state='succeeded',expected_results=2 WHERE id=$1",
    )
    .bind(old)
    .bind(sqlx::types::Json(&reading))
    .execute(&pool)
    .await?;
    for wod in [64459, 64460] {
        let filtered = store
            .filtered_photos(
                "judge@example.com",
                3910,
                0,
                &PhotoFilter {
                    category_id: Some(16907),
                    wod_id: Some(wod),
                    ..Default::default()
                },
            )
            .await?;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, old);
        assert_eq!(filtered[0].sync_status, "synced");
        assert_eq!(filtered[0].result_checks.as_array().unwrap().len(), 2);
    }
    assert!(store.photos("other@example.com", 3910, 0).await?.is_empty());
    let latest = add_photo(&store).await?;
    let mut different = reading.clone();
    // Un único WOD distinto basta para llevar toda la scorecard a incidencias.
    different["results"][1]["score_value"]["repetitions"] = json!(8);
    sqlx::query("UPDATE score_photos SET reading=$2,state='succeeded',expected_results=2,created_at='2030-01-01' WHERE id=$1")
        .bind(latest).bind(sqlx::types::Json(&different)).execute(&pool).await?;
    let photos = store.photos("judge@example.com", 3910, 0).await?;
    assert_eq!(photos.len(), 2);
    assert!(photos.iter().all(|photo| photo.duplicate_count == 2));
    assert_eq!(
        photos
            .iter()
            .find(|photo| photo.id == old)
            .unwrap()
            .sync_status,
        "superseded"
    );
    let unsynced = PhotoFilter {
        unsynced: true,
        ..Default::default()
    };
    let differences = store
        .filtered_photos("judge@example.com", 3910, 0, &unsynced)
        .await?;
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].id, latest);
    assert_eq!(differences[0].result_checks[0]["sync_status"], "synced");
    assert_eq!(differences[0].result_checks[1]["sync_status"], "different");
    assert!(store.delete_photo(&differences[0]).await?);
    let remaining = store.photos("judge@example.com", 3910, 0).await?;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].duplicate_count, 1);
    assert_eq!(remaining[0].sync_status, "synced");
    assert_eq!(
        store
            .photo_check_summary("judge@example.com", 3910)
            .await?
            .total,
        1
    );
    assert!(
        store
            .filtered_photos("judge@example.com", 3910, 0, &unsynced)
            .await?
            .is_empty()
    );
    Ok(())
}
