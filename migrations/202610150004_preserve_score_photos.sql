-- Las fotos son datos persistentes de la cuenta, independientes de la caché de Arena.
-- Conservar username y competition_id permite recuperarlas al volver a cargar la
-- competición, sin borrarlas al eliminar o refrescar su información local.
ALTER TABLE score_photos
    DROP CONSTRAINT score_photos_username_competition_id_fkey;
