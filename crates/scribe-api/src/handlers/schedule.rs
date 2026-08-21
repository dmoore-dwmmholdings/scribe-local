//! Processing-schedule endpoints — the mobile app's control over *when* the
//! worker box does heavy work (see [`scribe_core::schedule`]).
//!
//! ```text
//! GET  /processing-schedule            current policy + live status + backlog
//! PUT  /processing-schedule            replace the weekly windows
//! POST /processing-schedule/override   time-boxed run-now / pause-now / clear
//! ```
//!
//! These sit behind ordinary device auth, not the update token: this is a user
//! setting, not an operator action on the binary.
//!
//! Writes finish with a `NOTIFY` so a paused worker re-reads the schedule at
//! once instead of waiting out its poll interval — otherwise "pause now" would
//! feel broken for the several seconds that matter most.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use scribe_core::schedule::{
    DayWindow, OverrideMode, ProcessingSchedule, ScheduleDecision, ScheduleOverride,
    MAX_OVERRIDE_MINUTES, MINUTES_PER_DAY,
};
use scribe_core::Error;
use scribe_db::queue::QueueBacklog;

use crate::error::ApiError;
use crate::state::AppState;

/// Why the worker sent this NOTIFY. Nothing branches on the payload; it is
/// there so `pg_notify` traffic is legible in logs.
const NOTIFY_PAYLOAD: &str = "schedule";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Everything the schedule screen needs in one round trip: the policy, what it
/// evaluates to right now, and how much work is waiting on it.
#[derive(Debug, Serialize)]
pub struct ScheduleResponse {
    schedule: ProcessingSchedule,
    status: ScheduleStatus,
    backlog: Backlog,
}

/// The live verdict, with the next boundary as an absolute instant so the
/// client does not have to guess when its own countdown started.
#[derive(Debug, Serialize)]
pub struct ScheduleStatus {
    #[serde(flatten)]
    decision: ScheduleDecision,
    next_change_at: Option<DateTime<Utc>>,
    /// Server-local time, so the app can show the windows against the clock
    /// they are actually written in rather than the phone's.
    server_time: String,
}

#[derive(Debug, Serialize)]
pub struct Backlog {
    queued: i64,
    running: i64,
    failed: i64,
    recordings_waiting: i64,
}

impl From<QueueBacklog> for Backlog {
    fn from(b: QueueBacklog) -> Self {
        Self {
            queued: b.queued,
            running: b.running,
            failed: b.failed,
            recordings_waiting: b.recordings_waiting,
        }
    }
}

/// `PUT` body. The override is not settable here — it has its own endpoint, so
/// saving the weekly windows can never silently cancel a pause.
#[derive(Debug, Deserialize)]
pub struct SetScheduleRequest {
    enabled: bool,
    days: Vec<DayWindow>,
    #[serde(default = "default_grace")]
    grace_minutes: u32,
}

fn default_grace() -> u32 {
    10
}

/// `POST /processing-schedule/override` body.
#[derive(Debug, Deserialize)]
pub struct OverrideRequest {
    mode: OverrideAction,
    /// How long it lasts. Ignored for `clear`.
    #[serde(default)]
    minutes: Option<i64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideAction {
    Run,
    Pause,
    Clear,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /processing-schedule`
pub async fn get_schedule(State(state): State<AppState>) -> Result<Json<ScheduleResponse>, ApiError> {
    let schedule = state.db.processing_schedule().await?;
    let backlog = state.db.backlog().await?;
    Ok(Json(build_response(schedule, backlog)))
}

/// `PUT /processing-schedule` — replace the weekly windows.
pub async fn set_schedule(
    State(state): State<AppState>,
    Json(req): Json<SetScheduleRequest>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    validate_days(&req.days)?;
    if req.grace_minutes > 24 * 60 {
        return Err(ApiError(Error::BadRequest(
            "grace_minutes must be 1440 or less".into(),
        )));
    }

    // Carry the live override across the write: the user editing Tuesday's
    // window is not asking to cancel the pause they set five minutes ago.
    let existing = state.db.processing_schedule().await?;
    let schedule = ProcessingSchedule {
        enabled: req.enabled,
        days: req.days,
        grace_minutes: req.grace_minutes,
        active_override: existing.active_override,
    };

    let saved = state.db.set_processing_schedule(&schedule).await?;
    wake_workers(&state).await;
    let backlog = state.db.backlog().await?;
    Ok(Json(build_response(saved, backlog)))
}

/// `POST /processing-schedule/override` — run now, pause now, or clear.
pub async fn set_override(
    State(state): State<AppState>,
    Json(req): Json<OverrideRequest>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let mut schedule = state.db.processing_schedule().await?;

    schedule.active_override = match req.mode {
        OverrideAction::Clear => None,
        mode => {
            let minutes = req.minutes.unwrap_or(60);
            if minutes < 1 || minutes > MAX_OVERRIDE_MINUTES {
                return Err(ApiError(Error::BadRequest(format!(
                    "minutes must be between 1 and {MAX_OVERRIDE_MINUTES}"
                ))));
            }
            Some(ScheduleOverride {
                mode: match mode {
                    OverrideAction::Run => OverrideMode::Run,
                    OverrideAction::Pause => OverrideMode::Pause,
                    OverrideAction::Clear => unreachable!("handled above"),
                },
                until: Utc::now() + Duration::minutes(minutes),
            })
        }
    };

    let saved = state.db.set_processing_schedule(&schedule).await?;
    wake_workers(&state).await;
    let backlog = state.db.backlog().await?;
    Ok(Json(build_response(saved, backlog)))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_response(schedule: ProcessingSchedule, backlog: QueueBacklog) -> ScheduleResponse {
    let now = Utc::now();
    let decision = schedule.decide_now();
    let next_change_at = decision
        .next_change_secs
        .map(|secs| now + Duration::seconds(secs));
    ScheduleResponse {
        schedule,
        status: ScheduleStatus {
            decision,
            next_change_at,
            server_time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        },
        backlog: backlog.into(),
    }
}

/// Reject a day list the schedule could not evaluate as written. Rejecting
/// rather than clamping keeps the app honest: a window it could not save is one
/// the user should see fail, not one that quietly becomes something else.
fn validate_days(days: &[DayWindow]) -> Result<(), ApiError> {
    if days.len() != 7 {
        return Err(ApiError(Error::BadRequest(format!(
            "days must have exactly 7 entries (Monday first), got {}",
            days.len()
        ))));
    }
    for (i, d) in days.iter().enumerate() {
        if d.start > MINUTES_PER_DAY || d.end > MINUTES_PER_DAY {
            return Err(ApiError(Error::BadRequest(format!(
                "day {i}: start and end must be between 0 and {MINUTES_PER_DAY} minutes"
            ))));
        }
    }
    Ok(())
}

/// Best-effort nudge so an idle worker re-reads the schedule immediately. A
/// failure here costs latency, not correctness — the worker re-reads on its own
/// poll anyway — so it is logged rather than returned.
async fn wake_workers(state: &AppState) {
    if let Err(e) = state.db.notify_workers(NOTIFY_PAYLOAD).await {
        tracing::warn!(error = %e, "could not notify workers of the schedule change");
    }
}
