//! End-to-end smoke test against a live Postgres (pgvector).
//!
//! Skipped unless `DATABASE_URL` is set. Run it with a clean schema:
//!
//! ```text
//! docker exec -i scribe-pg psql -U scribe -d scribe \
//!   -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public; \
//!       CREATE EXTENSION vector; CREATE EXTENSION pgcrypto;"
//! DATABASE_URL=postgres://scribe:scribe@localhost:5433/scribe \
//!   cargo test -p scribe-db --test smoke -- --nocapture
//! ```
//!
//! It exercises the whole surface: recordings, segments, the queue (enqueue →
//! claim → heartbeat → complete → predecessors_done), transcript, chunks,
//! summaries, and hybrid search.

use std::time::Duration;

use scribe_core::config::DatabaseConfig;
use scribe_core::types::{JobKind, RecordingStatus, Word};
use scribe_db::chunks::NewChunk;
use scribe_db::search::SearchFilters;
use scribe_db::transcript::NewUtterance;
use scribe_db::Db;

#[tokio::test]
async fn smoke_full_pipeline() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping scribe-db smoke test");
        return;
    };

    let cfg = DatabaseConfig {
        url,
        max_connections: 5,
    };

    let db = Db::connect(&cfg).await.expect("connect");
    db.run_migrations().await.expect("run migrations");

    // --- recordings ---------------------------------------------------------
    let rec = db
        .create_recording(
            Some("Q3 offsite"),
            Some("device-abc"),
            Some(3),
            Some("aac"),
            Some(16_000),
        )
        .await
        .expect("create recording");
    assert_eq!(rec.status, RecordingStatus::Uploading);
    assert_eq!(rec.title.as_deref(), Some("Q3 offsite"));

    let fetched = db.get_recording(rec.id).await.expect("get recording");
    assert_eq!(fetched.id, rec.id);

    // NotFound for a random id.
    let missing = db.get_recording(uuid::Uuid::new_v4()).await;
    assert!(matches!(missing, Err(scribe_core::Error::NotFound(_))));

    db.set_recording_status(rec.id, RecordingStatus::Processing)
        .await
        .expect("set status");
    db.set_recording_duration(rec.id, 3_600_000)
        .await
        .expect("set duration");
    db.set_recording_storage_key(rec.id, &format!("{}/", rec.id))
        .await
        .expect("set storage key");

    let listed = db.list_recordings(10, 0).await.expect("list recordings");
    assert!(listed.iter().any(|r| r.id == rec.id));

    // --- segments (idempotent upsert) --------------------------------------
    let seg0 = db
        .insert_segment(rec.id, 0, "seg/000000.m4a", Some(0), Some(30_000), Some(1234), None)
        .await
        .expect("insert segment 0");
    let seg1 = db
        .insert_segment(
            rec.id,
            1,
            "seg/000001.m4a",
            Some(30_000),
            Some(30_000),
            Some(5678),
            Some(&[1u8, 2, 3, 4]),
        )
        .await
        .expect("insert segment 1");
    assert_eq!(seg0.seq, 0);
    assert_eq!(seg1.seq, 1);

    // Re-upsert seg0 with a corrected length — must not duplicate.
    let seg0b = db
        .insert_segment(rec.id, 0, "seg/000000.m4a", Some(0), Some(31_000), Some(2000), None)
        .await
        .expect("upsert segment 0");
    assert_eq!(seg0b.id, seg0.id, "upsert kept the same row id");
    assert_eq!(seg0b.duration_ms, Some(31_000));

    let segs = db
        .list_segments_by_recording(rec.id)
        .await
        .expect("list segments");
    assert_eq!(segs.len(), 2, "two distinct segments after re-upsert");

    // --- queue --------------------------------------------------------------
    let job = db
        .enqueue(rec.id, JobKind::Transcode, serde_json::json!({"foo": "bar"}))
        .await
        .expect("enqueue transcode");
    assert_eq!(job.kind, JobKind::Transcode);

    // Idempotent: a second enqueue of the same live (recording, kind) returns it.
    let job_again = db
        .enqueue(rec.id, JobKind::Transcode, serde_json::json!({}))
        .await
        .expect("re-enqueue transcode");
    assert_eq!(job_again.id, job.id, "enqueue is idempotent vs live job");

    let claimed = db
        .claim_one("worker-1", &[JobKind::Transcode])
        .await
        .expect("claim")
        .expect("a job to claim");
    assert_eq!(claimed.id, job.id);
    assert_eq!(claimed.state, scribe_core::types::JobState::Running);
    assert_eq!(claimed.locked_by.as_deref(), Some("worker-1"));

    // Nothing else to claim now.
    let none = db
        .claim_one("worker-1", &[JobKind::Transcode])
        .await
        .expect("claim again");
    assert!(none.is_none(), "queue empty after claiming the only job");

    assert!(
        db.heartbeat(claimed.id, "worker-1").await.expect("heartbeat"),
        "heartbeat by the owning worker succeeds"
    );
    assert!(
        !db.heartbeat(claimed.id, "other-worker")
            .await
            .expect("heartbeat guard"),
        "heartbeat by a non-owner is rejected"
    );

    db.complete(claimed.id).await.expect("complete");
    let done = db.get_job(claimed.id).await.expect("get job").expect("job");
    assert_eq!(done.state, scribe_core::types::JobState::Done);

    // predecessors_done: transcode is done, so diarize's predecessors are met.
    assert!(
        db.predecessors_done(rec.id, JobKind::Diarize)
            .await
            .expect("preds diarize"),
        "diarize ready once transcode done"
    );
    // merge needs diarize + transcribe, which are not done yet.
    assert!(
        !db.predecessors_done(rec.id, JobKind::Merge)
            .await
            .expect("preds merge"),
        "merge not ready (diarize/transcribe missing)"
    );
    // transcode itself has no predecessors.
    assert!(db
        .predecessors_done(rec.id, JobKind::Transcode)
        .await
        .expect("preds transcode"));

    // Enqueue diarize and check it's now claimable for a worker handling it.
    let diarize = db
        .enqueue(rec.id, JobKind::Diarize, serde_json::json!({}))
        .await
        .expect("enqueue diarize");
    assert_eq!(diarize.kind, JobKind::Diarize);

    // fail() path: fail diarize once → requeued with backoff (attempts=1 < max).
    let requeued = db
        .fail(diarize.id, "boom", 5, Duration::from_millis(1))
        .await
        .expect("fail diarize");
    assert!(requeued, "first failure requeues");
    let after_fail = db.get_job(diarize.id).await.unwrap().unwrap();
    assert_eq!(after_fail.attempts, 1);
    assert_eq!(after_fail.state, scribe_core::types::JobState::Queued);

    // reap_stuck: no running jobs are stuck (diarize was requeued), expect 0.
    let reaped = db
        .reap_stuck(Duration::from_secs(3600))
        .await
        .expect("reap");
    assert_eq!(reaped, 0);

    // --- speakers + recording_speakers -------------------------------------
    let spk_embedding: Vec<f32> = (0..192).map(|i| (i as f32) / 192.0).collect();
    let speaker = db
        .create_speaker("Dawson", Some(spk_embedding.clone()))
        .await
        .expect("create speaker");
    assert_eq!(speaker.display_name, "Dawson");

    let matched = db
        .match_speaker_by_embedding(&spk_embedding, 0.9)
        .await
        .expect("match speaker");
    let (m_spk, sim) = matched.expect("self-match above threshold");
    assert_eq!(m_spk.id, speaker.id);
    assert!(sim > 0.99, "self cosine similarity ~1.0, got {sim}");

    db.upsert_recording_speaker(rec.id, 0, Some(speaker.id), Some(spk_embedding.clone()))
        .await
        .expect("upsert recording_speaker 0");
    db.upsert_recording_speaker(rec.id, 1, None, None)
        .await
        .expect("upsert recording_speaker 1");

    let rspeakers = db
        .list_recording_speakers(rec.id)
        .await
        .expect("list recording speakers");
    assert_eq!(rspeakers.len(), 2);
    assert_eq!(rspeakers[0].display_name.as_deref(), Some("Dawson"));
    assert_eq!(rspeakers[1].display_name.as_deref(), Some("Speaker 1"));

    // --- transcript ---------------------------------------------------------
    let utt = NewUtterance {
        local_idx: Some(0),
        start_ms: 0,
        end_ms: 5_000,
        text: "We agreed to ship the pgvector hybrid search next sprint.".to_string(),
        words: vec![
            Word {
                text: "We".into(),
                start_ms: 0,
                end_ms: 200,
                conf: 0.99,
                local_idx: Some(0),
            },
            Word {
                text: "agreed".into(),
                start_ms: 200,
                end_ms: 600,
                conf: 0.98,
                local_idx: Some(0),
            },
        ],
    };
    let n = db
        .insert_utterances(rec.id, &[utt])
        .await
        .expect("insert utterances");
    assert_eq!(n, 1);

    let utts = db
        .list_utterances_by_recording(rec.id)
        .await
        .expect("list utterances");
    assert_eq!(utts.len(), 1);
    assert_eq!(utts[0].speaker_name.as_deref(), Some("Dawson"));
    assert_eq!(utts[0].words.len(), 2);

    // --- chunks (768-d dummy embedding) ------------------------------------
    let emb: Vec<f32> = (0..768).map(|i| ((i % 7) as f32) * 0.01).collect();
    let chunk = NewChunk {
        start_ms: Some(0),
        end_ms: Some(5_000),
        local_idx: Some(0),
        text: "We agreed to ship the pgvector hybrid search next sprint.".to_string(),
        embedding: emb.clone(),
    };
    let cn = db
        .insert_chunks(rec.id, &[chunk])
        .await
        .expect("insert chunks");
    assert_eq!(cn, 1);

    let clist = db
        .list_chunks_by_recording(rec.id)
        .await
        .expect("list chunks");
    assert_eq!(clist.len(), 1);

    // --- summaries ----------------------------------------------------------
    let summary = db
        .upsert_summary(
            rec.id,
            Some("Q3 offsite recap"),
            Some("The team aligned on shipping hybrid search."),
            serde_json::json!([{"who": "Dawson", "what": "ship search"}]),
            serde_json::json!(["search", "pgvector"]),
            serde_json::json!(["ship next sprint"]),
            Some("gemma3:27b"),
        )
        .await
        .expect("upsert summary");
    assert_eq!(summary.title.as_deref(), Some("Q3 offsite recap"));

    // Upsert again (idempotent on recording_id).
    let summary2 = db
        .upsert_summary(
            rec.id,
            Some("Q3 offsite recap v2"),
            Some("Updated."),
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([]),
            Some("gemma3:27b"),
        )
        .await
        .expect("re-upsert summary");
    assert_eq!(summary2.title.as_deref(), Some("Q3 offsite recap v2"));

    let got = db.get_summary(rec.id).await.expect("get summary");
    assert!(got.is_some());

    // --- hybrid search ------------------------------------------------------
    let filters = SearchFilters::default();
    let hits = db
        .hybrid_search("hybrid search sprint", &emb, &filters, 5)
        .await
        .expect("hybrid search");
    assert!(!hits.is_empty(), "hybrid search returns the chunk");
    assert_eq!(hits[0].recording_id, rec.id);
    assert!(hits[0].text.contains("hybrid search"));
    assert!(hits[0].score > 0.0);

    // Keyword and semantic helpers also return it.
    let kw = db
        .keyword_search("sprint", &filters, 5)
        .await
        .expect("keyword search");
    assert!(kw.iter().any(|h| h.recording_id == rec.id));

    let sem = db
        .semantic_search(&emb, &filters, 5)
        .await
        .expect("semantic search");
    assert!(sem.iter().any(|h| h.recording_id == rec.id));

    // Filtered: restrict to this recording, and to the matched speaker.
    let filtered = db
        .hybrid_search(
            "hybrid search",
            &emb,
            &SearchFilters {
                recording_id: Some(rec.id),
                speaker_id: Some(speaker.id),
                ..Default::default()
            },
            5,
        )
        .await
        .expect("filtered hybrid search");
    assert!(
        filtered.iter().any(|h| h.recording_id == rec.id),
        "filtered search still finds the chunk"
    );

    eprintln!("smoke_full_pipeline: all assertions passed");
}
