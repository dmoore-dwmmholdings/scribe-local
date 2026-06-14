-- Scribe speech + retrieval schema: speakers, diarization mapping, the
-- transcript, retrieval chunks, and LLM summaries. See design §10.
--
-- Dimension notes (these are schema commitments — design §8/§9):
--   * speakers.embedding   vector(192)   — speaker-embedding model dim
--                                          (NeMo TitaNet / 3D-Speaker class).
--   * chunks.embedding     halfvec(768)  — text-embedding model dim. 768 matches
--                                          nomic-embed-text (the lightweight
--                                          default). Switching to a 1024-d model
--                                          (e.g. Qwen3-Embedding-0.6B) means
--                                          ALTERing this column + `reindex`.

-- Known/enrolled speakers (cross-recording identity).
CREATE TABLE speakers (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  display_name  text NOT NULL,
  embedding     vector(192),
  created_at    timestamptz NOT NULL DEFAULT now()
);

-- Per-recording diarization result + mapping to known speakers.
CREATE TABLE recording_speakers (
  recording_id  uuid NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  local_idx     int  NOT NULL,                          -- 'Speaker 0' within this recording
  speaker_id    uuid REFERENCES speakers(id) ON DELETE SET NULL,  -- null until named/matched
  embedding     vector(192),
  PRIMARY KEY (recording_id, local_idx)
);
CREATE INDEX recording_speakers_speaker_idx ON recording_speakers (speaker_id);

-- The transcript: one row per utterance / speaker turn.
CREATE TABLE utterances (
  id            bigserial PRIMARY KEY,
  recording_id  uuid NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  local_idx     int,                                    -- which diarized speaker
  start_ms      bigint NOT NULL,
  end_ms        bigint NOT NULL,
  text          text   NOT NULL,
  words         jsonb  NOT NULL DEFAULT '[]'::jsonb,     -- [{w,start_ms,end_ms,conf}, …]
  -- Keyword search index, maintained automatically from `text`.
  tsv           tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED
);
CREATE INDEX utterances_recording_idx ON utterances (recording_id, start_ms);
CREATE INDEX utterances_tsv_idx       ON utterances USING gin (tsv);

-- Retrieval chunks for semantic search / RAG.
CREATE TABLE chunks (
  id            bigserial PRIMARY KEY,
  recording_id  uuid NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
  start_ms      bigint,
  end_ms        bigint,
  local_idx     int,
  text          text NOT NULL,
  tsv           tsvector GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED,
  embedding     halfvec(768)
);
CREATE INDEX chunks_recording_idx ON chunks (recording_id, start_ms);
CREATE INDEX chunks_tsv_idx       ON chunks USING gin (tsv);
-- HNSW over the half-precision vector (design §9: halves index size, ~no recall loss).
CREATE INDEX chunks_embedding_idx ON chunks USING hnsw (embedding halfvec_cosine_ops);

-- LLM-generated metadata.
CREATE TABLE summaries (
  recording_id  uuid PRIMARY KEY REFERENCES recordings(id) ON DELETE CASCADE,
  title         text,
  summary       text,
  action_items  jsonb NOT NULL DEFAULT '[]'::jsonb,
  topics        jsonb NOT NULL DEFAULT '[]'::jsonb,
  decisions     jsonb NOT NULL DEFAULT '[]'::jsonb,
  model         text,
  created_at    timestamptz NOT NULL DEFAULT now()
);
