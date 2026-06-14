//! Backend self-update endpoints (design: §5 self-update).
//!
//! `POST /admin/update` accepts a signed package, installs it, and restarts the
//! process into the new binary. These routes sit behind the *update token*
//! (not device tokens) and are only mounted when `[update].enabled`.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use futures::StreamExt;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use scribe_core::Error;
use scribe_update::{apply_package, current_target, rollback, ApplyOptions, Package};

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /admin/info` — current version/target and whether a rollback is possible.
pub async fn info(State(state): State<AppState>) -> impl IntoResponse {
    let u = &state.cfg.update;
    let binary = u
        .binary_path
        .clone()
        .or_else(|| std::env::current_exe().ok());
    let has_backup = binary
        .as_ref()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            p.with_file_name(format!("{name}.old")).exists()
        })
        .unwrap_or(false);

    Json(json!({
        "version": scribe_update::running_version(),
        "target": current_target(),
        "update_enabled": u.enabled,
        "restart_mode": format!("{:?}", u.restart),
        "has_backup": has_backup,
    }))
}

/// `POST /admin/update` — body is the raw `.tar.gz` package. Streams it to the
/// staging dir, verifies + installs it, then schedules a restart and returns.
pub async fn update(
    State(state): State<AppState>,
    body: axum::body::Body,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cfg = state.cfg.clone();

    // 1. Stream the upload to a staging file (never buffer a whole binary in RAM).
    let staging = cfg.update.staging_dir.clone();
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| ApiError(Error::Storage(format!("creating staging dir: {e}"))))?;
    let pkg_path = staging.join(format!("upload-{}.tar.gz", uuid::Uuid::new_v4()));

    {
        let mut file = tokio::fs::File::create(&pkg_path)
            .await
            .map_err(|e| ApiError(Error::Storage(format!("creating staging file: {e}"))))?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError(Error::BadRequest(format!("upload error: {e}"))))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError(Error::Storage(format!("writing package: {e}"))))?;
        }
        file.flush().await.ok();
    }

    // 2. Verify + install on a blocking thread (fs + crypto + subprocess).
    let cfg_for_apply = cfg.clone();
    let pkg_path_for_apply = pkg_path.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let pkg = Package::read(&pkg_path_for_apply)?;
        let opts = ApplyOptions {
            db_url: Some(cfg_for_apply.database.url.clone()),
            run_sanity_check: true,
            run_migrations: true,
        };
        apply_package(&cfg_for_apply.update, &pkg, &opts)
    })
    .await
    .map_err(|e| ApiError(Error::Internal(format!("update task panicked: {e}"))))?
    .map_err(|e| ApiError(e.into()))?;

    // Best-effort: drop the uploaded archive now that the binary is installed.
    let _ = tokio::fs::remove_file(&pkg_path).await;

    tracing::warn!(
        from = %outcome.from_version,
        to = %outcome.to_version,
        "update installed; scheduling restart"
    );

    // 3. Schedule the restart AFTER this response flushes to the client.
    let delay = cfg.update.restart_delay_ms;
    let cfg_for_restart = cfg.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        if let Err(e) = scribe_update::restart(&cfg_for_restart.update) {
            tracing::error!("restart failed: {e}");
        }
    });

    Ok(Json(json!({
        "from_version": outcome.from_version,
        "to_version": outcome.to_version,
        "target": outcome.target,
        "restarting": true,
        "restart_in_ms": delay,
    })))
}

/// `POST /admin/update/rollback` — restore the `.old` backup and restart.
pub async fn rollback_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cfg = state.cfg.clone();
    let cfg_for_rb = cfg.clone();
    let restored = tokio::task::spawn_blocking(move || rollback(&cfg_for_rb.update))
        .await
        .map_err(|e| ApiError(Error::Internal(format!("rollback task panicked: {e}"))))?
        .map_err(|e| ApiError(e.into()))?;

    let delay = cfg.update.restart_delay_ms;
    let cfg_for_restart = cfg.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        if let Err(e) = scribe_update::restart(&cfg_for_restart.update) {
            tracing::error!("restart after rollback failed: {e}");
        }
    });

    Ok(Json(json!({
        "restored_version": restored,
        "restarting": true,
        "restart_in_ms": delay,
    })))
}
