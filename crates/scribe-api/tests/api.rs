//! Integration tests for the storage-node HTTP API.
//!
//! These drive the real Axum router via `tower::ServiceExt::oneshot` (no socket
//! is bound) against a live Postgres. They are **skipped** when `DATABASE_URL`
//! is unset so `cargo test` stays green on machines without a database.
//!
//! Run against a DISPOSABLE test database (these tests DROP SCHEMA — never point
//! them at the live dev `scribe` DB; `assert_disposable_test_db` enforces this):
//! ```text
//! createdb -p 5433 -U scribe scribe_test   # one-time
//! DATABASE_URL=postgres://scribe:scribe@localhost:5433/scribe_test \
//!   cargo test -p scribe-api --no-default-features --test api -- --nocapture
//! ```
//!
//! Built `--no-default-features`, so the embedder is scribe-llm's deterministic
//! hash stub (768-dim, matching the `chunks.embedding halfvec(768)` column) —
//! identical text yields identical vectors, which is enough to exercise the
//! search/RAG plumbing without an ONNX runtime or a running Ollama.

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt; // for `oneshot`
use uuid::Uuid;

use scribe_api::{router, AppState};
use scribe_core::config::Config;
use scribe_db::chunks::NewChunk;
use scribe_db::transcript::NewUtterance;
use scribe_db::Db;
use scribe_llm::build_embedder;

/// Read `DATABASE_URL`, or `None` to skip.
fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty())
}

/// Reset the schema to a clean slate, then re-run migrations. Destructive — only
/// ever pointed at the disposable dev database.
async fn reset_and_migrate(url: &str) -> Db {
    // Never DROP SCHEMA on a non-disposable database (e.g. the live dev `scribe`).
    scribe_db::assert_disposable_test_db(url);

    let db = Db::connect(&scribe_core::config::DatabaseConfig {
        url: url.to_string(),
        max_connections: 5,
    })
    .await
    .expect("connect");

    // `raw_sql` runs over the simple-query protocol so multiple statements in
    // one string are allowed (a prepared statement rejects that).
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(db.pool())
        .await
        .expect("reset schema");
    db.run_migrations().await.expect("migrate");
    db
}

/// Build an [`AppState`] for the test against `url`, with blobs under `blob_root`.
async fn test_state(url: &str, blob_root: &Path) -> AppState {
    let mut cfg = Config::default();
    cfg.database.url = url.to_string();
    cfg.database.max_connections = 5;
    cfg.storage.blobs = blob_root.to_path_buf();
    // Auth off (dev default) so requests need no token.
    cfg.auth.require_device_token = false;
    // Point Ollama at an unused port so /ask exercises the no-LLM fallback fast.
    cfg.llm.base_url = "http://127.0.0.1:1".to_string();
    AppState::build(cfg).await.expect("build state")
}

/// Issue a request through the router and return (status, json body).
async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn full_api_flow() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL unset — skipping integration test");
        return;
    };

    let db = reset_and_migrate(&url).await;
    let tmp = tempfile::tempdir().expect("tempdir");
    let blob_root = tmp.path();
    let state = test_state(&url, blob_root).await;
    let app = router(state.clone());

    // --- health -----------------------------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "health status");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["db"], true, "health db should be true: {body}");
    assert!(body["version"].is_string());

    // --- create recording -------------------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/recordings")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "title": "Q3 planning", "participants_expected": 3 }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create status: {body}");
    assert_eq!(body["status"], "uploading");
    let rec_id: Uuid = serde_json::from_value(body["id"].clone()).expect("id");
    let template = body["upload"]["segment_url_template"]
        .as_str()
        .expect("template");
    assert_eq!(template, format!("/recordings/{rec_id}/segments/{{seq}}"));

    // --- PUT two segments -------------------------------------------------
    for seq in 1..=2i32 {
        let payload = format!("audio-bytes-for-seq-{seq}").into_bytes();
        let (status, body) = call(
            &app,
            Request::builder()
                .method("PUT")
                .uri(format!("/recordings/{rec_id}/segments/{seq}?ext=m4a"))
                .header("content-type", "audio/mp4")
                .header("x-segment-start-ms", (seq as i64 - 1) * 30_000)
                .header("x-segment-duration-ms", 30_000)
                .body(Body::from(payload.clone()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "segment {seq} status: {body}");
        assert_eq!(body["seq"], seq);
        assert_eq!(body["bytes"], payload.len());

        // File landed under the blob root at the expected path.
        let path = scribe_core::storage::segment_path(blob_root, rec_id, seq, "m4a");
        assert!(path.exists(), "segment file {seq} should exist at {path:?}");
        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk, payload, "segment {seq} bytes round-trip");
    }

    // Segment rows exist.
    let segments = db.list_segments_by_recording(rec_id).await.unwrap();
    assert_eq!(segments.len(), 2, "two segment rows");
    assert_eq!(segments[0].seq, 1);
    assert!(segments[0].sha256.is_some(), "sha256 was stored");

    // GET a stored segment back (range support path).
    let (status, _) = call(
        &app,
        Request::builder()
            .uri(format!("/recordings/{rec_id}/segments/1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "GET segment");

    // --- complete ---------------------------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{rec_id}/complete"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "duration_ms": 60_000, "marks": [1500, 42_000] }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete status: {body}");
    assert_eq!(body["status"], "processing");

    // Recording flipped to processing, with duration and bookmark marks stored.
    let rec = db.get_recording(rec_id).await.unwrap();
    assert_eq!(rec.status.as_str(), "processing");
    assert_eq!(rec.duration_ms, Some(60_000));
    assert_eq!(rec.marks, vec![1500, 42_000], "marks persisted on complete");

    // A transcode job was enqueued.
    let job_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE recording_id = $1 AND kind = 'transcode'")
            .bind(rec_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(job_count, 1, "one transcode job enqueued");

    // Completing again is a 409 (not in uploading state any more).
    let (status, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{rec_id}/complete"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "double complete is 409");

    // --- list + detail ----------------------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .uri("/recordings?limit=10")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["recordings"].as_array().unwrap().len(), 1);

    let (status, body) = call(
        &app,
        Request::builder()
            .uri(format!("/recordings/{rec_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "detail status: {body}");
    assert_eq!(body["recording"]["id"], json!(rec_id));
    // Marks supplied at completion serialize back in the detail response.
    assert_eq!(body["recording"]["marks"], json!([1500, 42_000]));
    assert!(body["utterances"].is_array());
    assert!(body["speakers"].is_array());

    // --- seed transcript + chunk + summary for search/ask -----------------
    // Use the same stub embedder the API uses so the chunk vector matches a
    // query for the same text.
    let embedder = build_embedder(&Config::default().llm).unwrap();
    let chunk_text = "We agreed to move the launch date to October and finalize the pricing.";
    let embedding = embedder.embed_one(chunk_text).await.unwrap();

    db.insert_utterances(
        rec_id,
        &[NewUtterance {
            local_idx: Some(0),
            start_ms: 0,
            end_ms: 5_000,
            text: chunk_text.to_string(),
            words: vec![],
        }],
    )
    .await
    .unwrap();

    db.insert_chunks(
        rec_id,
        &[NewChunk {
            start_ms: Some(0),
            end_ms: Some(5_000),
            local_idx: Some(0),
            text: chunk_text.to_string(),
            embedding,
        }],
    )
    .await
    .unwrap();

    db.upsert_summary(
        rec_id,
        Some("Q3 planning"),
        Some("The team set an October launch and finalized pricing."),
        json!(["finalize pricing"]),
        json!(["launch", "pricing"]),
        json!(["move launch to October"]),
        Some("test-model"),
        Some("general"),
    )
    .await
    .unwrap();

    // Detail now carries summaries[] (one per template) + transcript.
    let (status, body) = call(
        &app,
        Request::builder()
            .uri(format!("/recordings/{rec_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summaries = body["summaries"].as_array().expect("summaries array");
    assert_eq!(summaries.len(), 1, "one summary view so far: {body}");
    assert_eq!(summaries[0]["title"], "Q3 planning");
    assert_eq!(summaries[0]["template"], "general", "detail carries template");
    assert_eq!(body["utterances"].as_array().unwrap().len(), 1);

    // Multidimensional summaries (Feature C): a second template ADDS a view
    // rather than replacing the first. Upsert directly (the summarize stage
    // runs in the worker, not in this router-only test).
    db.upsert_summary(
        rec_id,
        Some("Q3 planning — interview view"),
        Some("Interview-framed recap."),
        json!([]),
        json!(["interview"]),
        json!([]),
        Some("test-model"),
        Some("interview"),
    )
    .await
    .unwrap();

    let (status, body) = call(
        &app,
        Request::builder()
            .uri(format!("/recordings/{rec_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let summaries = body["summaries"].as_array().expect("summaries array");
    assert_eq!(summaries.len(), 2, "both template views retained: {body}");
    let templates: Vec<&str> = summaries
        .iter()
        .map(|s| s["template"].as_str().unwrap())
        .collect();
    assert!(templates.contains(&"general"), "general view kept: {templates:?}");
    assert!(templates.contains(&"interview"), "interview view added: {templates:?}");

    // Re-summarizing the SAME template overwrites just that view (still 2 rows).
    db.upsert_summary(
        rec_id,
        Some("Q3 planning"),
        Some("Refreshed general recap."),
        json!([]),
        json!([]),
        json!([]),
        Some("test-model"),
        Some("general"),
    )
    .await
    .unwrap();
    let general_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM summaries WHERE recording_id = $1 AND template = 'general'",
    )
    .bind(rec_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(general_count, 1, "same-template re-summary upserts, not duplicates");

    // --- summary templates registry --------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .uri("/summary-templates")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "templates status: {body}");
    let templates = body["templates"].as_array().expect("templates array");
    let ids: Vec<&str> = templates.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec!["general", "standup", "interview", "one_on_one", "lecture", "sales"]
    );
    assert_eq!(templates[2]["label"], "Interview");

    // --- re-summarize with a chosen template (202, enqueues a job) --------
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{rec_id}/summarize"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "template": "interview" }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "summarize status: {body}");
    assert_eq!(body["id"], json!(rec_id));
    assert_eq!(body["template"], "interview");
    assert_eq!(body["status"], "queued");

    // The summarize job was enqueued with the template in its payload.
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE recording_id = $1 AND kind = 'summarize' \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(rec_id)
    .fetch_one(db.pool())
    .await
    .expect("a summarize job was enqueued");
    assert_eq!(payload["template"], "interview");

    // Unknown template id → 400.
    let (status, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{rec_id}/summarize"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "template": "nonsense" }).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown template rejected");

    // --- search -----------------------------------------------------------
    let (status, body) = call(
        &app,
        Request::builder()
            .uri("/search?q=launch%20date%20pricing&limit=5")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "search status: {body}");
    let hits = body["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty(), "search should return at least one hit: {body}");
    assert_eq!(hits[0]["recording_id"], json!(rec_id));
    assert!(hits[0]["text"].as_str().unwrap().contains("launch"));

    // --- ask --------------------------------------------------------------
    // Ollama isn't running (we pointed it at a dead port), so the answer is a
    // placeholder, but citations must still come back from retrieval.
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri("/ask")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "question": "When is the launch?", "top_k": 5 }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "ask status: {body}");
    let citations = body["citations"].as_array().expect("citations array");
    assert!(!citations.is_empty(), "ask should return citations: {body}");
    assert_eq!(citations[0]["recording_id"], json!(rec_id));
    assert!(body["answer"].is_string(), "ask returns an answer string");

    // --- audio 404 before transcode --------------------------------------
    let (status, _) = call(
        &app,
        Request::builder()
            .uri(format!("/recordings/{rec_id}/audio"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "audio 404 before transcode");

    // --- reprocess (Feature D) -------------------------------------------
    // Re-runs the whole pipeline: derived data (utterances/chunks/summaries) is
    // wiped, status flips to processing, and a fresh transcode job is enqueued.
    let (status, body) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{rec_id}/reprocess"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "reprocess status: {body}");
    assert_eq!(body["id"], json!(rec_id));
    assert_eq!(body["status"], "processing");

    // Derived data was cleared.
    assert!(
        db.list_utterances_by_recording(rec_id).await.unwrap().is_empty(),
        "utterances cleared on reprocess"
    );
    assert!(
        db.list_summaries_by_recording(rec_id).await.unwrap().is_empty(),
        "summaries cleared on reprocess"
    );
    let chunk_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chunks WHERE recording_id = $1")
            .bind(rec_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(chunk_count, 0, "chunks cleared on reprocess");

    // A single fresh transcode job is queued (old jobs were deleted first).
    let transcode_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE recording_id = $1 AND kind = 'transcode'",
    )
    .bind(rec_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(transcode_jobs, 1, "one fresh transcode job after reprocess");

    // Status flipped back to processing.
    let rec = db.get_recording(rec_id).await.unwrap();
    assert_eq!(rec.status.as_str(), "processing");

    // Reprocessing a nonexistent recording is a 404.
    let (status, _) = call(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/recordings/{}/reprocess", Uuid::new_v4()))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "reprocess unknown recording is 404");
}
