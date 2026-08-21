//! The worker's view of the processing schedule.
//!
//! [`ScheduleGate`] wraps the stored [`ProcessingSchedule`] in a short-lived
//! cache and answers one question: may this worker start heavy work right now?
//!
//! The cache exists because the claim loop asks on every iteration, and while a
//! backlog drains that is a tight loop — one `SELECT` per claim would be pure
//! overhead. It is deliberately short (and explicitly invalidated on a wakeup
//! NOTIFY) so a "pause now" tapped on the phone takes effect in seconds, not on
//! the next poll boundary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use scribe_core::schedule::{ProcessingSchedule, ScheduleDecision};
use scribe_db::Db;
use tokio::sync::Mutex;

/// How long a schedule read is reused before going back to Postgres.
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Cached schedule + the last verdict we logged, so transitions are announced
/// once rather than every poll.
#[derive(Default)]
struct GateState {
    cached: Option<(Instant, ProcessingSchedule)>,
    last_logged_allowed: Option<bool>,
}

pub struct ScheduleGate {
    db: Db,
    state: Mutex<GateState>,
}

impl ScheduleGate {
    pub fn new(db: Db) -> Arc<Self> {
        Arc::new(Self { db, state: Mutex::new(GateState::default()) })
    }

    /// Drop the cache so the next read goes to the database. Called when a
    /// NOTIFY wakes the worker, since the reason may be a schedule edit.
    pub async fn invalidate(&self) {
        self.state.lock().await.cached = None;
    }

    /// The current schedule, from cache when fresh.
    ///
    /// A database error here is reported and then ignored in favour of the last
    /// known schedule (or the default, which allows everything). Refusing to
    /// process because the settings table was briefly unreachable would be the
    /// wrong trade: the schedule is a courtesy to the desktop user, not a safety
    /// interlock.
    pub async fn schedule(&self) -> ProcessingSchedule {
        let mut state = self.state.lock().await;
        if let Some((at, sched)) = &state.cached {
            if at.elapsed() < CACHE_TTL {
                return sched.clone();
            }
        }
        match self.db.processing_schedule().await {
            Ok(sched) => {
                state.cached = Some((Instant::now(), sched.clone()));
                sched
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not read the processing schedule");
                state
                    .cached
                    .as_ref()
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default()
            }
        }
    }

    /// Evaluate the schedule against the local clock, logging window openings
    /// and closings as they happen.
    pub async fn decide(&self) -> ScheduleDecision {
        let sched = self.schedule().await;
        let decision = sched.decide_now();

        let mut state = self.state.lock().await;
        if state.last_logged_allowed != Some(decision.allowed) {
            state.last_logged_allowed = Some(decision.allowed);
            let resumes_in_mins = decision.next_change_secs.map(|s| s / 60);
            if decision.allowed {
                tracing::info!(
                    reason = ?decision.reason,
                    closes_in_mins = ?resumes_in_mins,
                    "processing window open"
                );
            } else {
                tracing::info!(
                    reason = ?decision.reason,
                    opens_in_mins = ?resumes_in_mins,
                    "processing paused by the schedule; live transcription still runs"
                );
            }
        }
        decision
    }
}
