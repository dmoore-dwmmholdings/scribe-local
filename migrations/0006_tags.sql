-- Free-form organization tags on recordings (no join table; a Postgres text[]).
ALTER TABLE recordings ADD COLUMN tags text[] NOT NULL DEFAULT '{}';
