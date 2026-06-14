//! Liveness/health (design §15 phase 5): `GET /health`.
//!
//! Unauthenticated — it is mounted outside the auth layer so a probe never needs
//! a device token. Reports the build version and whether a trivial `SELECT 1`
//! against Postgres succeeds.

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// `GET /health` → `{ status, version, db }`. Always 200; `db` reflects whether
/// the database round-trips.
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_ok = sqlx_ping(&state).await;
    Json(json!({
        "status": "ok",
        "version": scribe_core::VERSION,
        "db": db_ok,
    }))
}

/// Run `SELECT 1` against the pool; any error → `false` (never fails the probe).
async fn sqlx_ping(state: &AppState) -> bool {
    sqlx::query("SELECT 1")
        .execute(state.db.pool())
        .await
        .is_ok()
}
