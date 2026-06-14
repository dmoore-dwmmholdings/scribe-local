-- Scribe pipeline scratch artifacts: transient stage-to-stage hand-offs.
--
-- The transcribe and diarize stages each write their raw result here as jsonb;
-- the merge stage reads both back to assign a speaker to every word (design §8).
-- These rows are not durable products — they exist only to decouple the DAG
-- stages, which run as independent jobs. On reprocessing they are overwritten
-- (upsert on the (recording_id, kind) primary key).

CREATE TABLE recording_artifacts (
  recording_id uuid NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  kind text NOT NULL,              -- 'transcript' | 'diarization'
  data jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (recording_id, kind)
);
