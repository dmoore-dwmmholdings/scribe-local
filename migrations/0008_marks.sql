-- Per-recording bookmark "marks": millisecond offsets into the audio captured
-- by the user tapping "Mark" while recording. Persisted on completion so the
-- client can render jump points. Empty array until the client supplies any.
ALTER TABLE recordings ADD COLUMN marks integer[] NOT NULL DEFAULT '{}';
