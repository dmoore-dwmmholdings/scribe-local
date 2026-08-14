-- Multidimensional summaries: keep one summary row PER TEMPLATE per recording.
--
-- Previously `summaries` had exactly one row per recording (PRIMARY KEY on
-- `recording_id`, created in 0002_speech.sql; the default constraint name is
-- `summaries_pkey`). Re-summarizing with a new template overwrote the prior
-- view. We now retain every generated view: uniqueness moves to
-- `(recording_id, template)` so each template keeps its own row, and
-- re-summarizing with a new template ADDS a view rather than replacing one.

-- Backfill rows written before the `template` column existed (NULL) to the
-- default template id, so the NOT NULL + uniqueness change below holds.
UPDATE summaries SET template = 'general' WHERE template IS NULL;

-- `template` is now a required, defaulted dimension of the key.
ALTER TABLE summaries ALTER COLUMN template SET DEFAULT 'general';
ALTER TABLE summaries ALTER COLUMN template SET NOT NULL;

-- Drop the single-row-per-recording primary key and key on
-- (recording_id, template) instead, allowing multiple rows per recording.
-- `recording_id` is still NOT NULL (carried over from the old PK column def),
-- and the FK ON DELETE CASCADE from 0002 is independent of this constraint.
ALTER TABLE summaries DROP CONSTRAINT summaries_pkey;
ALTER TABLE summaries ADD CONSTRAINT summaries_pkey PRIMARY KEY (recording_id, template);
