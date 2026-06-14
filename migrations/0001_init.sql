-- Scribe initial schema: recordings, upload segments, and the job queue.
-- See design §10. Requires the pgvector extension (vectors land in 0002).

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pgcrypto;   -- gen_random_uuid()

-- A captured meeting.
CREATE TABLE recordings (
  id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title                 text,
  created_at            timestamptz NOT NULL DEFAULT now(),
  device_id             text,
  duration_ms           bigint,
  status                text NOT NULL DEFAULT 'uploading',  -- uploading|processing|ready|failed
  participants_expected int,                                -- helps diarization
  audio_format          text,                               -- e.g. 'aac'
  sample_rate           int,
  storage_key           text                                -- path/key prefix in the blob store
);
CREATE INDEX recordings_status_idx  ON recordings (status);
CREATE INDEX recordings_created_idx ON recordings (created_at DESC);

-- Chunked-upload pieces (the tus / segment story).
CREATE TABLE segments (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  recording_id  uuid NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  seq           int  NOT NULL,
  storage_key   text NOT NULL,
  start_ms      bigint,
  duration_ms   bigint,
  bytes         bigint,
  sha256        bytea,
  uploaded_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (recording_id, seq)
);

-- The work queue (design §7). Claimed via SELECT … FOR UPDATE SKIP LOCKED.
CREATE TABLE jobs (
  id            bigserial PRIMARY KEY,
  recording_id  uuid REFERENCES recordings(id) ON DELETE CASCADE,
  kind          text NOT NULL,                    -- transcode|diarize|transcribe|merge|embed|summarize
  state         text NOT NULL DEFAULT 'queued',   -- queued|running|done|failed
  priority      int  NOT NULL DEFAULT 0,
  attempts      int  NOT NULL DEFAULT 0,
  run_after     timestamptz NOT NULL DEFAULT now(),
  locked_by     text,
  locked_at     timestamptz,
  payload       jsonb NOT NULL DEFAULT '{}'::jsonb,
  error         text,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now()
);
-- Supports the claim query's WHERE state/kind/run_after + ORDER BY priority, created_at.
CREATE INDEX jobs_claim_idx ON jobs (state, kind, run_after, priority DESC, created_at);
-- Reaper scan over stuck `running` jobs.
CREATE INDEX jobs_locked_idx ON jobs (state, locked_at);
-- One live job per (recording, kind): no duplicate stages enqueued.
CREATE UNIQUE INDEX jobs_recording_kind_live_idx
  ON jobs (recording_id, kind)
  WHERE state IN ('queued', 'running');

-- Keep updated_at fresh on every change.
CREATE OR REPLACE FUNCTION scribe_touch_updated_at() RETURNS trigger AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER jobs_touch_updated_at
  BEFORE UPDATE ON jobs
  FOR EACH ROW EXECUTE FUNCTION scribe_touch_updated_at();

-- LISTEN/NOTIFY: wake an idle worker the instant a job becomes runnable.
-- Workers `LISTEN scribe_jobs`; the payload is the job kind.
CREATE OR REPLACE FUNCTION scribe_notify_job() RETURNS trigger AS $$
BEGIN
  PERFORM pg_notify('scribe_jobs', NEW.kind);
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER jobs_notify_insert
  AFTER INSERT ON jobs
  FOR EACH ROW WHEN (NEW.state = 'queued')
  EXECUTE FUNCTION scribe_notify_job();

CREATE TRIGGER jobs_notify_requeue
  AFTER UPDATE OF state ON jobs
  FOR EACH ROW WHEN (NEW.state = 'queued' AND OLD.state IS DISTINCT FROM 'queued')
  EXECUTE FUNCTION scribe_notify_job();
