-- Las acciones manuales usan esta versión para no sobrescribir otra edición/envío.
ALTER TABLE score_photos ADD COLUMN revision BIGINT NOT NULL DEFAULT 0;
