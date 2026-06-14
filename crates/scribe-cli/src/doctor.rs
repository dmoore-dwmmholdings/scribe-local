//! `scribe doctor` — preflight validation (design §4, §15 Phase 0).
//!
//! Runs a fixed battery of checks and prints an aligned `PASS`/`WARN`/`FAIL`
//! report. A `FAIL` is a hard stop (the process exits non-zero); a `WARN` is a
//! soft signal (e.g. no GPU, Ollama down) that doesn't block the stub build or a
//! storage-only node. The intent mirrors design §4: "validate config, DB
//! connectivity, model presence, GPU, Tailscale."

use std::path::Path;

use scribe_asr::models::{AsrModelPaths, DiarizationModelPaths};
use scribe_core::config::Config;
use scribe_db::Db;
use scribe_llm::OllamaClient;

/// A single check's outcome.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
    Info,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Info => "INFO",
        }
    }
}

/// Accumulates check results and tracks whether any hard failure occurred.
struct Report {
    rows: Vec<(Status, &'static str, String)>,
}

impl Report {
    fn new() -> Self {
        Self { rows: Vec::new() }
    }

    fn add(&mut self, status: Status, name: &'static str, detail: impl Into<String>) {
        self.rows.push((status, name, detail.into()));
    }

    fn any_fail(&self) -> bool {
        self.rows.iter().any(|(s, _, _)| *s == Status::Fail)
    }

    /// Print the aligned table and a summary line. The check name column is
    /// padded to the widest name so the detail column lines up.
    fn print(&self) {
        let width = self
            .rows
            .iter()
            .map(|(_, n, _)| n.len())
            .max()
            .unwrap_or(0);
        println!("scribe doctor — {} check(s)", self.rows.len());
        println!();
        for (status, name, detail) in &self.rows {
            println!("  [{}]  {:<width$}  {}", status.tag(), name, detail, width = width);
        }
        println!();
        let fails = self.rows.iter().filter(|(s, _, _)| *s == Status::Fail).count();
        let warns = self.rows.iter().filter(|(s, _, _)| *s == Status::Warn).count();
        if fails > 0 {
            println!("result: FAIL ({fails} failing, {warns} warning(s))");
        } else if warns > 0 {
            println!("result: OK with {warns} warning(s)");
        } else {
            println!("result: OK");
        }
    }
}

/// Run every check, print the report, and return `true` when there were no
/// hard failures (the caller maps `false` → non-zero exit).
pub async fn run(cfg: &Config, config_path: Option<&Path>) -> bool {
    let mut report = Report::new();

    check_config(&mut report, config_path);
    let db = check_database(&mut report, cfg).await;
    check_blobs(&mut report, cfg);
    check_ffmpeg(&mut report).await;
    check_models(&mut report, cfg);
    check_ollama(&mut report, cfg).await;
    check_best_effort(&mut report);

    // `db` is intentionally dropped here; the connection was only needed for the
    // checks above.
    drop(db);

    report.print();
    !report.any_fail()
}

/// (1) Config loaded. By the time `doctor` runs, `Config::load` already
/// succeeded (or the command would have errored out), so this is a PASS that
/// echoes the source for transparency.
fn check_config(report: &mut Report, config_path: Option<&Path>) {
    let detail = match config_path {
        Some(p) => format!("loaded from {}", p.display()),
        None => "loaded from defaults + SCRIBE_* env (no --config)".to_string(),
    };
    report.add(Status::Pass, "config", detail);
}

/// (2) DB connectable + migrations applied. Connect, then confirm the
/// `_sqlx_migrations` table exists with at least one applied row.
async fn check_database(report: &mut Report, cfg: &Config) -> Option<Db> {
    let db = match Db::connect(&cfg.database).await {
        Ok(db) => db,
        Err(e) => {
            report.add(Status::Fail, "database", format!("cannot connect: {e}"));
            return None;
        }
    };

    match applied_migrations(&db).await {
        Ok(n) if n > 0 => {
            report.add(
                Status::Pass,
                "database",
                format!("connected; {n} migration(s) applied"),
            );
        }
        Ok(_) => {
            report.add(
                Status::Fail,
                "database",
                "connected but no migrations applied — run `scribe migrate`",
            );
        }
        Err(_) => {
            // `_sqlx_migrations` missing → migrations never ran.
            report.add(
                Status::Fail,
                "database",
                "connected but migrations table absent — run `scribe migrate`",
            );
        }
    }
    Some(db)
}

async fn applied_migrations(db: &Db) -> anyhow::Result<i64> {
    use sqlx::Row;
    let row = sqlx::query("SELECT count(*) AS n FROM _sqlx_migrations")
        .fetch_one(db.pool())
        .await?;
    Ok(row.try_get::<i64, _>("n")?)
}

/// (3) Blob dir exists and is writable. Create it if missing → PASS. Probe
/// writability by creating and removing a temp file.
fn check_blobs(report: &mut Report, cfg: &Config) {
    let dir = &cfg.storage.blobs;
    if let Err(e) = std::fs::create_dir_all(dir) {
        report.add(
            Status::Fail,
            "storage",
            format!("cannot create blobs dir {}: {e}", dir.display()),
        );
        return;
    }
    let probe = dir.join(".scribe-doctor-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            report.add(
                Status::Pass,
                "storage",
                format!("blobs dir writable: {}", dir.display()),
            );
        }
        Err(e) => {
            report.add(
                Status::Fail,
                "storage",
                format!("blobs dir not writable {}: {e}", dir.display()),
            );
        }
    }
}

/// (4) `ffmpeg` on PATH (`ffmpeg -version`).
async fn check_ffmpeg(report: &mut Report) {
    match tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let first = String::from_utf8_lossy(&out.stdout);
            let line = first.lines().next().unwrap_or("ffmpeg").trim().to_string();
            report.add(Status::Pass, "ffmpeg", line);
        }
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            report.add(Status::Fail, "ffmpeg", format!("`ffmpeg -version` exited {code}"));
        }
        Err(e) => {
            report.add(
                Status::Fail,
                "ffmpeg",
                format!("not found on PATH: {e}"),
            );
        }
    }
}

/// (5) Models dir presence — WARN if missing (the stub engine still works).
fn check_models(report: &mut Report, cfg: &Config) {
    let dir = &cfg.worker.models_dir;
    let asr = AsrModelPaths::discover(dir).is_some();
    let diar = DiarizationModelPaths::discover(dir).is_some();
    if asr && (diar || !cfg.asr.diarization) {
        report.add(
            Status::Pass,
            "models",
            format!("ONNX assets present under {}", dir.display()),
        );
    } else if !dir.exists() {
        report.add(
            Status::Warn,
            "models",
            format!(
                "models_dir {} missing — stub engine will be used (run `scribe models pull`)",
                dir.display()
            ),
        );
    } else {
        report.add(
            Status::Warn,
            "models",
            format!(
                "incomplete ONNX assets under {} (asr: {}, diar: {}) — stub engine will be used",
                dir.display(),
                if asr { "ok" } else { "missing" },
                if diar { "ok" } else { "missing" },
            ),
        );
    }
}

/// (6) Ollama reachable — WARN if not (a storage-only node has no Ollama).
async fn check_ollama(report: &mut Report, cfg: &Config) {
    let client = OllamaClient::new(&cfg.llm.ollama_url);
    match client.health().await {
        Ok(true) => report.add(
            Status::Pass,
            "ollama",
            format!("reachable at {}", cfg.llm.ollama_url),
        ),
        Ok(false) => report.add(
            Status::Warn,
            "ollama",
            format!("responded with non-2xx at {}", cfg.llm.ollama_url),
        ),
        Err(_) => report.add(
            Status::Warn,
            "ollama",
            format!(
                "not reachable at {} — needed only on the processing node",
                cfg.llm.ollama_url
            ),
        ),
    }
}

/// (7) GPU / Tailscale — best-effort INFO/WARN, never required (design §4).
fn check_best_effort(report: &mut Report) {
    // GPU: a present `nvidia-smi` is a good hint; absence is fine (CPU path).
    match std::process::Command::new("nvidia-smi").arg("-L").output() {
        Ok(out) if out.status.success() => {
            let line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("GPU detected")
                .trim()
                .to_string();
            report.add(Status::Pass, "gpu", line);
        }
        _ => report.add(
            Status::Info,
            "gpu",
            "no NVIDIA GPU detected (CPU path is supported, just slower)",
        ),
    }

    // Tailscale: a present `tailscale` binary reporting a backend state is a
    // good signal. We don't parse status — just whether the CLI runs.
    match std::process::Command::new("tailscale").arg("version").output() {
        Ok(out) if out.status.success() => {
            let line = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("tailscale present")
                .trim()
                .to_string();
            report.add(Status::Info, "tailscale", format!("CLI present ({line})"));
        }
        _ => report.add(
            Status::Info,
            "tailscale",
            "CLI not found (only needed for from-anywhere access)",
        ),
    }
}
