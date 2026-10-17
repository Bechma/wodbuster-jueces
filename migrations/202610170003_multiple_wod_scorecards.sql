-- Las fotos históricas conservan todos sus datos y corresponden a WOD único.
ALTER TABLE score_photos ADD COLUMN expected_results SMALLINT NOT NULL DEFAULT 1
    CHECK (expected_results IN (1,2));

DROP VIEW score_photo_checks;

-- Expande solo para comparar y filtrar. La galería y las incidencias siguen
-- teniendo una fila por imagen, aunque cualquiera de sus WODs tenga problemas.
CREATE VIEW score_photo_result_checks AS
WITH results AS (
    SELECT p.id,p.username,p.competition_id,p.created_at,p.reading,
        r.value AS score_reading,r.ordinality AS result_index,
        p.reading->>'category_id' AS category_id,
        p.reading->>'team_reference' AS team_reference,
        r.value->>'wod_id' AS wod_id
    FROM score_photos p
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(p.reading->'results')='array'
                   AND jsonb_array_length(p.reading->'results')>0
             THEN p.reading->'results'
             ELSE jsonb_build_array(jsonb_build_object(
                 'wod_id',p.reading->'wod_id','score_value',p.reading->'score_value')) END
    ) WITH ORDINALITY AS r(value,ordinality)
), assignments AS (
    SELECT *,
        CASE WHEN NULLIF(category_id,'') IS NOT NULL AND NULLIF(wod_id,'') IS NOT NULL
            AND NULLIF(team_reference,'') IS NOT NULL
            THEN count(*) OVER assigned ELSE 1 END AS duplicate_count,
        row_number() OVER assigned=1 AS is_latest
    FROM results
    WINDOW assigned AS (
        PARTITION BY username,competition_id,category_id,wod_id,team_reference
        ORDER BY created_at DESC,id DESC,result_index
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    )
)
SELECT a.id,a.result_index,a.wod_id,a.duplicate_count,a.is_latest,
    NULLIF(s.data->'value','null'::jsonb) AS arena_value,
    to_char(d.loaded_at AT TIME ZONE 'Europe/Madrid','DD/MM/YYYY HH24:MI:SS') AS sync_checked_at,
    CASE
        WHEN a.duplicate_count>1 AND NOT a.is_latest THEN 'superseded'
        WHEN d.competition_id IS NULL THEN 'unchecked'
        WHEN s.wod_id IS NULL OR NULLIF(a.score_reading->'score_value','null'::jsonb) IS NULL THEN 'unavailable'
        WHEN s.result IS NULL THEN 'different'
        WHEN NULLIF(s.data->'value','null'::jsonb) IS NULL THEN 'unavailable'
        WHEN a.score_reading->'score_value'=s.data->'value' THEN 'synced'
        ELSE 'different'
    END AS sync_status
FROM assignments a
LEFT JOIN competition_details d ON d.username=a.username AND d.competition_id=a.competition_id
LEFT JOIN competition_scores s ON s.username=a.username AND s.competition_id=a.competition_id
    AND s.category_id::text=a.category_id AND s.wod_id::text=a.wod_id
    AND s.team_reference=a.team_reference;

CREATE VIEW score_photo_checks AS
SELECT p.*,c.duplicate_count,c.is_latest,c.arena_value,c.sync_checked_at,c.sync_status,c.result_checks
FROM score_photos p
JOIN (
    SELECT id,max(duplicate_count) AS duplicate_count,bool_or(is_latest) AS is_latest,
        CASE WHEN count(*)=1 THEN (jsonb_agg(arena_value)->0)
             ELSE jsonb_agg(jsonb_build_object('wod_id',wod_id,'value',arena_value) ORDER BY result_index) END AS arena_value,
        max(sync_checked_at) AS sync_checked_at,
        CASE WHEN bool_or(sync_status='different') THEN 'different'
             WHEN bool_or(sync_status='unavailable') THEN 'unavailable'
             WHEN bool_or(sync_status='unchecked') THEN 'unchecked'
             WHEN bool_or(sync_status='superseded') THEN 'superseded'
             ELSE 'synced' END AS sync_status,
        jsonb_agg(jsonb_build_object('wod_id',wod_id,'sync_status',sync_status,
            'duplicate_count',duplicate_count,'is_latest',is_latest) ORDER BY result_index) AS result_checks
    FROM score_photo_result_checks GROUP BY id
) c ON c.id=p.id;
