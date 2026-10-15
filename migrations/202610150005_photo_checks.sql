-- Las comprobaciones se calculan sobre la lectura actual y el último detalle
-- consultado en Arena. No modifican fotos ni envían puntuaciones.
CREATE VIEW score_photo_checks AS
WITH assignments AS (
    SELECT id,
        CASE WHEN NULLIF(reading->>'category_id','') IS NOT NULL
            AND NULLIF(reading->>'wod_id','') IS NOT NULL
            AND NULLIF(reading->>'team_reference','') IS NOT NULL
            THEN count(*) OVER assigned ELSE 1 END AS duplicate_count,
        row_number() OVER assigned = 1 AS is_latest
    FROM score_photos
    WINDOW assigned AS (
        PARTITION BY username,competition_id,reading->>'category_id',reading->>'wod_id',reading->>'team_reference'
        ORDER BY created_at DESC,id DESC
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    )
)
SELECT p.*, a.duplicate_count, a.is_latest,
    NULLIF(s.data->'value','null'::jsonb) AS arena_value,
    to_char(d.loaded_at AT TIME ZONE 'Europe/Madrid','DD/MM/YYYY HH24:MI:SS') AS sync_checked_at,
    CASE
        WHEN a.duplicate_count>1 AND NOT a.is_latest THEN 'superseded'
        WHEN d.competition_id IS NULL THEN 'unchecked'
        WHEN s.wod_id IS NULL OR NULLIF(p.reading->'score_value','null'::jsonb) IS NULL THEN 'unavailable'
        WHEN s.result IS NULL THEN 'different'
        WHEN NULLIF(s.data->'value','null'::jsonb) IS NULL THEN 'unavailable'
        WHEN p.reading->'score_value'=s.data->'value' THEN 'synced'
        ELSE 'different'
    END AS sync_status
FROM score_photos p
JOIN assignments a ON a.id=p.id
LEFT JOIN competition_details d ON d.username=p.username AND d.competition_id=p.competition_id
LEFT JOIN competition_scores s ON s.username=p.username AND s.competition_id=p.competition_id
    AND s.category_id::text=p.reading->>'category_id'
    AND s.wod_id::text=p.reading->>'wod_id'
    AND s.team_reference=p.reading->>'team_reference';
