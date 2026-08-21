//! The processing schedule: *when* the heavy pipeline is allowed to run.
//!
//! The worker box is also a desktop. Transcription and diarization saturate the
//! GPU, so the operator wants the pipeline to drain while they are at work and
//! to stay out of the way when they are home using the machine. This module is
//! the shared, storage-agnostic definition of that policy; `scribe-db` persists
//! it, `scribe-api` edits it, and `scribe-pipeline` obeys it.
//!
//! Three ideas make up the policy:
//!
//! * **Weekly windows.** One window per weekday (Monday first), each with its
//!   own on/off switch. `end <= start` wraps past midnight, so an overnight
//!   window is expressible as a single row.
//! * **Overrides.** A time-boxed "run now" (drain the backlog on demand) or
//!   "pause now" (sit down to game inside the normal window). An override
//!   outranks the windows until it expires, then the windows resume.
//! * **A grace cap.** A stage already running when the window closes gets
//!   `grace_minutes` to finish before the worker gives up on it. Without a cap a
//!   long transcribe could run hours past the boundary.
//!
//! Everything is evaluated in the **server's local time**, which is the clock
//! that "when I'm home" is actually measured against. No timezone field: the
//! machine being protected is the machine deciding.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::JobKind;

/// Key this schedule is stored under in the `app_settings` table.
pub const SETTING_KEY: &str = "processing_schedule";

/// Minutes in a day; the upper bound for a window edge (`1440` = midnight).
pub const MINUTES_PER_DAY: u16 = 1440;

/// Longest a single override may run. An override is a nudge, not a second
/// schedule — capping it means a forgotten "pause now" cannot silently stop
/// processing forever.
pub const MAX_OVERRIDE_MINUTES: i64 = 24 * 60;

/// How far ahead [`ProcessingSchedule::decide`] looks for the next window.
const LOOKAHEAD_DAYS: i64 = 8;

/// One weekday's processing window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayWindow {
    /// Does this day have a window at all? `false` = never process on this day.
    pub enabled: bool,
    /// Window start, minutes after local midnight (`0..=1440`).
    pub start: u16,
    /// Window end, minutes after local midnight (`0..=1440`).
    ///
    /// `end <= start` wraps past midnight into the following day: `22:00 →
    /// 06:00` is `start: 1320, end: 360`. As a consequence `start == end` means
    /// a full 24 hours — a zero-length window is not expressible, and does not
    /// need to be, because that is what `enabled: false` is for.
    pub end: u16,
}

impl DayWindow {
    /// A day that never processes.
    pub const OFF: DayWindow = DayWindow { enabled: false, start: 540, end: 1020 };

    /// Length of the window in minutes (`1..=1440`), ignoring `enabled`.
    pub fn length_minutes(&self) -> u16 {
        if self.end > self.start {
            self.end - self.start
        } else {
            MINUTES_PER_DAY - self.start + self.end
        }
    }
}

/// Which way a time-boxed override bends the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    /// Process regardless of the windows (drain the backlog now).
    Run,
    /// Do not process, even inside a window (I am using the machine).
    Pause,
}

/// A time-boxed instruction that outranks the weekly windows until `until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleOverride {
    pub mode: OverrideMode,
    /// When the override lapses and the windows take over again (UTC — an
    /// absolute instant, so it survives the local clock moving under it).
    pub until: DateTime<Utc>,
}

/// The whole policy: weekly windows, an optional override, and the grace cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessingSchedule {
    /// Master switch. `false` (the default) means "always process" — the
    /// behaviour of every build before this feature existed.
    pub enabled: bool,
    /// Seven windows, **Monday first**. Shorter/longer lists are padded or
    /// truncated by [`ProcessingSchedule::normalize`] rather than rejected, so a
    /// hand-edited row in `app_settings` cannot brick the worker.
    pub days: Vec<DayWindow>,
    /// How long a stage that is already running may overrun a closing window
    /// before the worker abandons and requeues it.
    pub grace_minutes: u32,
    #[serde(rename = "override")]
    pub active_override: Option<ScheduleOverride>,
}

impl Default for ProcessingSchedule {
    fn default() -> Self {
        // Pre-filled with a plausible work week so flipping `enabled` on in the
        // app is one tap rather than fourteen time edits. Inert until then.
        let weekday = DayWindow { enabled: true, start: 9 * 60, end: 17 * 60 };
        Self {
            enabled: false,
            days: vec![
                weekday,
                weekday,
                weekday,
                weekday,
                weekday,
                DayWindow::OFF,
                DayWindow::OFF,
            ],
            grace_minutes: 10,
            active_override: None,
        }
    }
}

/// Why the pipeline is or is not allowed to run right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleReason {
    /// No schedule configured — everything runs, as it always did.
    Disabled,
    /// Inside one of the weekly windows.
    InWindow,
    /// Outside every window.
    OutsideWindow,
    /// A "run now" override is in force.
    OverrideRun,
    /// A "pause now" override is in force.
    OverridePause,
}

/// The verdict for one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleDecision {
    /// May scheduled work start (or continue) right now?
    pub allowed: bool,
    pub reason: ScheduleReason,
    /// Seconds until this verdict could next change — the far edge of the
    /// current window, the near edge of the next one, or an override's expiry.
    /// `None` when nothing is coming (schedule off, or no window in the next
    /// [`LOOKAHEAD_DAYS`] days).
    pub next_change_secs: Option<i64>,
}

impl ProcessingSchedule {
    /// Clamp the stored value into something evaluable: exactly seven days, all
    /// edges within `0..=1440`, a sane grace cap, and no expired override.
    ///
    /// Called on both read and write. On write it rejects nothing, because the
    /// API validates first; on read it is the guard that keeps a malformed or
    /// out-of-date JSONB row from taking the worker down.
    pub fn normalize(&mut self) {
        self.days.truncate(7);
        while self.days.len() < 7 {
            self.days.push(DayWindow::OFF);
        }
        for d in &mut self.days {
            d.start = d.start.min(MINUTES_PER_DAY);
            d.end = d.end.min(MINUTES_PER_DAY);
        }
        self.grace_minutes = self.grace_minutes.min(24 * 60);
        if let Some(o) = self.active_override {
            if o.until <= Utc::now() {
                self.active_override = None;
            }
        }
    }

    /// Evaluate against the current local clock.
    pub fn decide_now(&self) -> ScheduleDecision {
        self.decide(chrono::Local::now().naive_local(), Utc::now())
    }

    /// Evaluate at an explicit instant. `now_local` is wall-clock time on the
    /// worker box (what the windows are written in); `now_utc` is the same
    /// instant as an absolute, used only to age out the override.
    ///
    /// Split in two so the whole policy is testable without touching the host
    /// clock or its timezone.
    pub fn decide(&self, now_local: NaiveDateTime, now_utc: DateTime<Utc>) -> ScheduleDecision {
        // 1. A live override outranks everything until it lapses.
        if let Some(o) = self.active_override {
            if o.until > now_utc {
                let (allowed, reason) = match o.mode {
                    OverrideMode::Run => (true, ScheduleReason::OverrideRun),
                    OverrideMode::Pause => (false, ScheduleReason::OverridePause),
                };
                return ScheduleDecision {
                    allowed,
                    reason,
                    next_change_secs: Some((o.until - now_utc).num_seconds().max(0)),
                };
            }
        }

        // 2. No schedule → the pre-feature behaviour: run whenever there is work.
        if !self.enabled {
            return ScheduleDecision {
                allowed: true,
                reason: ScheduleReason::Disabled,
                next_change_secs: None,
            };
        }

        // 3. Otherwise, where does `now` sit relative to the merged windows?
        let windows = self.merged_windows(now_local.date());
        if let Some(w) = windows.iter().find(|w| w.0 <= now_local && now_local < w.1) {
            return ScheduleDecision {
                allowed: true,
                reason: ScheduleReason::InWindow,
                next_change_secs: Some((w.1 - now_local).num_seconds().max(0)),
            };
        }
        let next = windows.iter().find(|w| w.0 > now_local);
        ScheduleDecision {
            allowed: false,
            reason: ScheduleReason::OutsideWindow,
            next_change_secs: next.map(|w| (w.0 - now_local).num_seconds().max(0)),
        }
    }

    /// Concrete windows around `anchor`, as `(start, end)` local instants,
    /// sorted and with touching/overlapping windows merged.
    ///
    /// Merging is what makes chained days behave: a Monday `22:00 → 06:00`
    /// window that abuts Tuesday's `06:00 → 09:00` is one nine-hour window, so
    /// the worker does not stop at 06:00 and the UI does not claim a boundary
    /// where nothing changes.
    fn merged_windows(&self, anchor: NaiveDate) -> Vec<(NaiveDateTime, NaiveDateTime)> {
        let mut out: Vec<(NaiveDateTime, NaiveDateTime)> = Vec::new();
        // Start a day early: yesterday's overnight window can still be running.
        for offset in -1..LOOKAHEAD_DAYS {
            let Some(date) = anchor.checked_add_signed(Duration::days(offset)) else {
                continue;
            };
            let idx = date.weekday().num_days_from_monday() as usize;
            let Some(day) = self.days.get(idx).copied() else {
                continue;
            };
            if !day.enabled {
                continue;
            }
            let midnight = date.and_time(NaiveTime::MIN);
            let start = midnight + Duration::minutes(day.start as i64);
            let end = start + Duration::minutes(day.length_minutes() as i64);
            out.push((start, end));
        }

        out.sort_by_key(|w| w.0);
        let mut merged: Vec<(NaiveDateTime, NaiveDateTime)> = Vec::with_capacity(out.len());
        for w in out {
            match merged.last_mut() {
                Some(last) if w.0 <= last.1 => last.1 = last.1.max(w.1),
                _ => merged.push(w),
            }
        }
        merged
    }

    /// The job kinds a worker may claim given `decision`, filtered from the
    /// kinds it is configured to handle.
    ///
    /// When processing is paused this keeps only the kinds that
    /// [`JobKind::bypasses_schedule`] exempts, so live transcription of an
    /// active recording never goes dark — the operator started that recording
    /// deliberately, and it is seconds of audio at a time, not a full pipeline.
    pub fn claimable_kinds(configured: &[JobKind], decision: &ScheduleDecision) -> Vec<JobKind> {
        if decision.allowed {
            return configured.to_vec();
        }
        configured
            .iter()
            .copied()
            .filter(JobKind::bypasses_schedule)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// A local instant on a known weekday. 2026-08-17 is a Monday.
    fn local(day: u32, hour: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, day)
            .unwrap()
            .and_hms_opt(hour, min, 0)
            .unwrap()
    }

    fn workweek() -> ProcessingSchedule {
        let mut s = ProcessingSchedule { enabled: true, ..Default::default() };
        s.normalize();
        s
    }

    #[test]
    fn disabled_schedule_always_allows() {
        let s = ProcessingSchedule::default();
        let d = s.decide(local(17, 22, 0), Utc::now());
        assert!(d.allowed);
        assert_eq!(d.reason, ScheduleReason::Disabled);
        assert_eq!(d.next_change_secs, None);
    }

    #[test]
    fn inside_a_weekday_window() {
        let d = workweek().decide(local(17, 10, 0), Utc::now());
        assert!(d.allowed);
        assert_eq!(d.reason, ScheduleReason::InWindow);
        // 10:00 → 17:00 is seven hours.
        assert_eq!(d.next_change_secs, Some(7 * 3600));
    }

    #[test]
    fn outside_the_window_reports_the_next_opening() {
        let d = workweek().decide(local(17, 20, 0), Utc::now());
        assert!(!d.allowed);
        assert_eq!(d.reason, ScheduleReason::OutsideWindow);
        // 20:00 Monday → 09:00 Tuesday is thirteen hours.
        assert_eq!(d.next_change_secs, Some(13 * 3600));
    }

    #[test]
    fn weekend_waits_for_monday() {
        // 2026-08-22 is a Saturday; the next window is Monday 09:00.
        let d = workweek().decide(local(22, 12, 0), Utc::now());
        assert!(!d.allowed);
        assert_eq!(d.next_change_secs, Some((2 * 24 - 12 + 9) * 3600));
    }

    #[test]
    fn overnight_window_wraps_past_midnight() {
        let mut s = ProcessingSchedule {
            enabled: true,
            days: vec![DayWindow { enabled: true, start: 22 * 60, end: 6 * 60 }; 7],
            grace_minutes: 10,
            active_override: None,
        };
        s.normalize();
        // 02:00 Tuesday is inside Monday's 22:00 → 06:00 window.
        let d = s.decide(local(18, 2, 0), Utc::now());
        assert!(d.allowed);
        assert_eq!(d.next_change_secs, Some(4 * 3600));
        // 12:00 Tuesday is between windows.
        assert!(!s.decide(local(18, 12, 0), Utc::now()).allowed);
    }

    #[test]
    fn abutting_windows_merge_into_one() {
        let mut s = ProcessingSchedule {
            enabled: true,
            // Every day 22:00 → 06:00 plus 06:00 → 09:00 is not expressible as
            // one row, but consecutive days chaining is: 12:00 → 12:00 is 24h.
            days: vec![DayWindow { enabled: true, start: 12 * 60, end: 12 * 60 }; 7],
            grace_minutes: 10,
            active_override: None,
        };
        s.normalize();
        let d = s.decide(local(17, 13, 0), Utc::now());
        assert!(d.allowed);
        // Continuous cover: the boundary is the end of the lookahead, far away.
        assert!(d.next_change_secs.unwrap() > 5 * 24 * 3600);
    }

    #[test]
    fn run_override_beats_a_closed_window() {
        let now = Utc::now();
        let mut s = workweek();
        s.active_override =
            Some(ScheduleOverride { mode: OverrideMode::Run, until: now + Duration::minutes(30) });
        let d = s.decide(local(17, 23, 0), now);
        assert!(d.allowed);
        assert_eq!(d.reason, ScheduleReason::OverrideRun);
        assert_eq!(d.next_change_secs, Some(30 * 60));
    }

    #[test]
    fn pause_override_beats_an_open_window() {
        let now = Utc::now();
        let mut s = workweek();
        s.active_override =
            Some(ScheduleOverride { mode: OverrideMode::Pause, until: now + Duration::hours(1) });
        let d = s.decide(local(17, 10, 0), now);
        assert!(!d.allowed);
        assert_eq!(d.reason, ScheduleReason::OverridePause);
    }

    #[test]
    fn expired_override_falls_back_to_the_windows() {
        let now = Utc::now();
        let mut s = workweek();
        s.active_override =
            Some(ScheduleOverride { mode: OverrideMode::Pause, until: now - Duration::minutes(1) });
        assert!(s.decide(local(17, 10, 0), now).allowed);
        // ...and normalize drops the dead override entirely.
        s.normalize();
        assert!(s.active_override.is_none());
    }

    #[test]
    fn normalize_pads_a_short_day_list() {
        let mut s = ProcessingSchedule { days: vec![DayWindow::OFF], ..Default::default() };
        s.normalize();
        assert_eq!(s.days.len(), 7);
    }

    #[test]
    fn live_transcription_survives_a_pause() {
        let paused = ScheduleDecision {
            allowed: false,
            reason: ScheduleReason::OutsideWindow,
            next_change_secs: Some(60),
        };
        let kinds = ProcessingSchedule::claimable_kinds(&JobKind::ALL, &paused);
        assert_eq!(kinds, vec![JobKind::TranscribeSegment]);
    }
}
