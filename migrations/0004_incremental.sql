-- Incremental ("live") transcription support.
--
-- As segments upload during a recording, a `transcribe_segment` job transcribes
-- each into provisional utterances so the user sees text immediately; the full
-- diarized pass on `complete` later replaces them.

-- Per-segment transcription progress: NULL until that segment has been
-- transcribed into provisional utterances. The partial index makes
-- "next untranscribed segment" cheap.
ALTER TABLE segments ADD COLUMN transcribed_at timestamptz;
CREATE INDEX segments_untranscribed_idx ON segments (recording_id, seq)
  WHERE transcribed_at IS NULL;

-- Whether the title was auto-generated. The continuous title regeneration and
-- the final summary may overwrite an auto title, but never a user-provided one.
ALTER TABLE recordings ADD COLUMN title_auto boolean NOT NULL DEFAULT false;
