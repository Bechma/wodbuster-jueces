ALTER TABLE score_submission_blocks ADD COLUMN wod_name TEXT NOT NULL DEFAULT '';
ALTER TABLE score_submission_blocks DROP CONSTRAINT score_submission_blocks_check;
ALTER TABLE score_submission_blocks ADD CHECK (category_id > 0 OR wod_id > 0 OR wod_name <> '');
ALTER TABLE score_submission_blocks ADD CHECK (wod_id = 0 OR wod_name = '');
ALTER TABLE score_submission_blocks DROP CONSTRAINT score_submission_blocks_pkey;
ALTER TABLE score_submission_blocks ADD PRIMARY KEY (username, competition_id, category_id, wod_id, wod_name);

-- Convierte los bloqueos previos al grupo por nombre cuando está en la caché.
INSERT INTO score_submission_blocks (username, competition_id, category_id, wod_name)
SELECT DISTINCT b.username, b.competition_id, b.category_id,
    lower(trim(regexp_replace(w.name, '\s+', ' ', 'g')))
FROM score_submission_blocks b
JOIN competition_wods w ON w.username = b.username AND w.competition_id = b.competition_id
    AND w.id = b.wod_id AND (b.category_id = 0 OR w.category_id = b.category_id)
WHERE b.wod_id > 0 AND trim(regexp_replace(w.name, '\s+', ' ', 'g')) <> ''
ON CONFLICT DO NOTHING;

-- Conserva las reglas antiguas cuyo ID ya no aparece en el detalle guardado.
DELETE FROM score_submission_blocks b
WHERE b.wod_id > 0 AND EXISTS (
    SELECT 1 FROM competition_wods w
    WHERE w.username = b.username AND w.competition_id = b.competition_id
        AND w.id = b.wod_id AND (b.category_id = 0 OR w.category_id = b.category_id)
        AND trim(regexp_replace(w.name, '\s+', ' ', 'g')) <> ''
);
