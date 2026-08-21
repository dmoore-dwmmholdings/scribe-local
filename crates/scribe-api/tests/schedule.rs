//! Integration tests for the processing-schedule endpoints.
//!
//! Same shape as `api.rs`: the real Axum router driven through
//! `tower::ServiceExt::oneshot` against a live Postgres, skipped when
//! `DATABASE_URL` is unset.
//!
//! Run against a DISPOSABLE test database (this DROPs SCHEMA — never point it at
//! the live dev `scribe` DB; `assert_disposable_test_db` enforces this):
//! ```text
//! DATABASE_URL=postgres://scribe:scribe@localhost:5433/scribe_test \
//!   cargo test -p scribe-api --no-default-features --test schedule -- --nocapture
//! ```
//!
//! One test function, not several: every DB-backed test here resets the schema,
//! so parallel test functions in one binary would race each other.

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt; // for `oneshot`

use scribe_api::{router, AppState};
use scribe_core::config::Config;
use scribe_db::Db;

fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty())
}

async fn reset_and_migrate(url: &str) -> Db {
    scribe_db::assert_disposable_test_db(url);
    let db = Db::connect(&scribe_core::config::DatabaseConfig {
        url: url.to_string(),
        max_connections: 5,
    })
    .await
    .expect("connect");
    sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .execute(db.pool())
        .await
        .expect("reset schema");
    db.run_migrations().await.expect("migrate");
    db
}

async fn test_state(url: &str, blob_root: &Path) -> AppState {
    let mut cfg = Config::default();
    cfg.database.url = url.to_string();
    cfg.database.max_connections = 5;
    cfg.storage.blobs = blob_root.to_path_buf();
    cfg.auth.require_device_token = false;
    cfg.llm.base_url = "http://127.0.0.1:1".to_string();
    AppState::build(cfg).await.expect("build state")
}

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn get_schedule(app: &axum::Router) -> (StatusCode, Value) {
    call(
        app,
        Request::builder()
            .uri("/processing-schedule")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn put_schedule(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    call(
        app,
        Request::builder()
            .method("PUT")
            .uri("/processing-schedule")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn post_override(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    call(
        app,
        Request::builder()
            .method("POST")
            .uri("/processing-schedule/override")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

/// Seven days, all enabled, covering the whole day — so the "in a window"
/// assertions hold whatever time the test happens to run at.
fn always_on_days() -> Value {
    json!(vec![json!({ "enabled": true, "start": 0, "end": 1440 }); 7])
}

/// Seven days, none enabled — never in a window, whatever the clock says.
fn never_on_days() -> Value {
    json!(vec![json!({ "enabled": false, "start": 540, "end": 1020 }); 7])
}

#[tokio::test]
async fn processing_schedule_endpoints() {
    let Some(url) = database_url() else {
        eprintln!("DATABASE_URL unset — skipping integration test");
        return;
    };

    let _db = reset_and_migrate(&url).await;
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = test_state(&url, tmp.path()).await;
    let app = router(state);

    // --- default: no row, no schedule, everything runs --------------------
    let (status, body) = get_schedule(&app).await;
    assert_eq!(status, StatusCode::OK, "get status: {body}");
    assert_eq!(body["schedule"]["enabled"], false);
    assert_eq!(
        body["schedule"]["days"].as_array().map(|d| d.len()),
        Some(7),
        "always seven days: {body}"
    );
    assert_eq!(body["status"]["allowed"], true);
    assert_eq!(body["status"]["reason"], "disabled");
    assert_eq!(body["backlog"]["queued"], 0);

    // --- a schedule that is always open -----------------------------------
    let (status, body) = put_schedule(
        &app,
        json!({ "enabled": true, "days": always_on_days(), "grace_minutes": 5 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put status: {body}");
    assert_eq!(body["schedule"]["enabled"], true);
    assert_eq!(body["schedule"]["grace_minutes"], 5);
    assert_eq!(body["status"]["allowed"], true);
    assert_eq!(body["status"]["reason"], "in_window");

    // It persisted, rather than merely being echoed back.
    let (_, body) = get_schedule(&app).await;
    assert_eq!(body["schedule"]["grace_minutes"], 5, "persisted: {body}");

    // --- a schedule that is never open ------------------------------------
    let (status, body) = put_schedule(
        &app,
        json!({ "enabled": true, "days": never_on_days(), "grace_minutes": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put status: {body}");
    assert_eq!(body["status"]["allowed"], false);
    assert_eq!(body["status"]["reason"], "outside_window");
    assert_eq!(
        body["status"]["next_change_secs"],
        Value::Null,
        "no window is ever coming: {body}"
    );

    // --- "process now" outranks the closed windows ------------------------
    let (status, body) = post_override(&app, json!({ "mode": "run", "minutes": 60 })).await;
    assert_eq!(status, StatusCode::OK, "override status: {body}");
    assert_eq!(body["status"]["allowed"], true);
    assert_eq!(body["status"]["reason"], "override_run");
    assert!(body["status"]["next_change_at"].is_string());
    assert_eq!(body["schedule"]["override"]["mode"], "run");

    // --- editing the windows must not cancel a live override --------------
    let (status, body) = put_schedule(
        &app,
        json!({ "enabled": true, "days": never_on_days(), "grace_minutes": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put status: {body}");
    assert_eq!(body["schedule"]["grace_minutes"], 30);
    assert_eq!(
        body["schedule"]["override"]["mode"], "run",
        "the override should survive a window edit: {body}"
    );
    assert_eq!(body["status"]["allowed"], true);

    // --- "pause now" outranks an open window ------------------------------
    put_schedule(
        &app,
        json!({ "enabled": true, "days": always_on_days(), "grace_minutes": 10 }),
    )
    .await;
    let (status, body) = post_override(&app, json!({ "mode": "pause", "minutes": 30 })).await;
    assert_eq!(status, StatusCode::OK, "override status: {body}");
    assert_eq!(body["status"]["allowed"], false);
    assert_eq!(body["status"]["reason"], "override_pause");

    // --- clearing hands control back to the windows -----------------------
    let (status, body) = post_override(&app, json!({ "mode": "clear" })).await;
    assert_eq!(status, StatusCode::OK, "clear status: {body}");
    assert_eq!(body["schedule"]["override"], Value::Null);
    assert_eq!(body["status"]["allowed"], true);
    assert_eq!(body["status"]["reason"], "in_window");

    // --- validation -------------------------------------------------------
    let (status, _) = put_schedule(
        &app,
        json!({ "enabled": true, "days": [], "grace_minutes": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an empty day list is rejected");

    let (status, _) = put_schedule(
        &app,
        json!({
            "enabled": true,
            "days": vec![json!({ "enabled": true, "start": 0, "end": 9999 }); 7],
            "grace_minutes": 10,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an out-of-range edge is rejected");

    let (status, _) = post_override(&app, json!({ "mode": "run", "minutes": 100_000 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an over-long override is rejected");

    // A rejected write leaves the stored schedule untouched.
    let (_, body) = get_schedule(&app).await;
    assert_eq!(body["schedule"]["grace_minutes"], 10);
    assert_eq!(body["schedule"]["override"], Value::Null);
}
