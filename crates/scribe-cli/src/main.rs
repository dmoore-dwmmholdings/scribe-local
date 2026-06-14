//! `scribe` — the single binary for the Scribe meeting-transcription system.
//!
//! One crate, one binary; the subcommand selects the *role* (design §4). `serve`
//! runs the storage-node HTTP API, `worker` runs the processing pipeline, and the
//! remaining subcommands are operator utilities (migrate, ingest, reindex,
//! enroll, speaker, models, doctor).
//!
//! Shared plumbing — config loading, tracing init, the DB pool — lives here and
//! is set up once before dispatch. Every subcommand loads the same
//! [`Config`](scribe_core::config::Config) via `--config` + `SCRIBE_*` env.
//!
//! ## Build paths
//! With the default `onnx` feature this links the real ASR/diarization/embedding
//! stack. With `--no-default-features` the whole binary builds against
//! deterministic stubs, so the full CLI (including `ingest --inline`) runs
//! end-to-end without any ONNX runtime or downloaded models.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use scribe_core::config::Config;
use scribe_db::Db;
use uuid::Uuid;

mod doctor;
mod models;

// ---------------------------------------------------------------------------
// CLI surface (clap derive) — mirrors design §4.
// ---------------------------------------------------------------------------

/// Self-hosted meeting recording, transcription & semantic index.
#[derive(Debug, Parser)]
#[command(name = "scribe", version = scribe_core::VERSION, about, long_about = None)]
struct Cli {
    /// Path to a TOML config file (env `SCRIBE_*` overrides still apply).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Emit logs as structured JSON instead of human-readable text.
    #[arg(long, global = true)]
    json_logs: bool,

    /// Increase log verbosity (`-v` = debug, `-vv` = trace). `RUST_LOG` wins if set.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the storage node: Axum HTTP API + uploads, blob store, search & Q&A.
    Serve(ServeArgs),

    /// Run the processing node: claim jobs and run the pipeline.
    Worker(WorkerArgs),

    /// Apply database migrations (run on the storage node).
    Migrate(MigrateArgs),

    /// Manually ingest an audio file (backfill / testing / desktop capture).
    Ingest(IngestArgs),

    /// Recompute derived data (embeddings / summaries) over existing recordings.
    Reindex(ReindexArgs),

    /// Register a known speaker's voice for name labelling.
    Enroll(EnrollArgs),

    /// Manage speaker identities (list, rename, delete, merge).
    #[command(subcommand)]
    Speaker(SpeakerCmd),

    /// Manage local model assets (ONNX + Ollama).
    #[command(subcommand)]
    Models(ModelsCmd),

    /// Validate config, DB connectivity, model presence, ffmpeg, Ollama.
    Doctor,
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Override `api.bind` (e.g. `0.0.0.0:8443`).
    #[arg(long, value_name = "ADDR")]
    bind: Option<String>,
}

#[derive(Debug, Args)]
struct WorkerArgs {
    /// Which stages to handle: `all` or a CSV of
    /// `transcode,diarize,transcribe,merge,embed,summarize`.
    #[arg(long, value_name = "CSV")]
    stages: Option<String>,

    /// Parallel jobs (GPU box: usually 1 so the card isn't oversubscribed).
    #[arg(long, value_name = "N")]
    concurrency: Option<usize>,

    /// Directory holding ONNX model assets.
    #[arg(long, value_name = "PATH")]
    models_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    /// Migration target. Only `latest` is supported (the default).
    #[arg(long, default_value = "latest", value_name = "TARGET")]
    to: String,
}

#[derive(Debug, Args)]
struct IngestArgs {
    /// Path to the audio file to ingest.
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// Optional human title for the recording.
    #[arg(long)]
    title: Option<String>,

    /// Expected participant count (improves diarization clustering).
    #[arg(long, value_name = "N")]
    participants: Option<i32>,

    /// Run the whole pipeline synchronously now (no worker needed).
    #[arg(long)]
    inline: bool,
}

#[derive(Debug, Args)]
struct ReindexArgs {
    /// Re-embed transcript chunks (e.g. after an embedding-model change).
    #[arg(long)]
    embeddings: bool,

    /// Regenerate LLM summaries.
    #[arg(long)]
    summaries: bool,
}

#[derive(Debug, Args)]
struct EnrollArgs {
    /// Display name for the enrolled voice.
    #[arg(long)]
    name: String,

    /// Audio sample of the speaker's voice.
    #[arg(long, value_name = "FILE")]
    audio: PathBuf,
}

#[derive(Debug, Subcommand)]
enum SpeakerCmd {
    /// List enrolled speakers.
    List,
    /// Rename an enrolled speaker.
    Rename {
        /// Speaker id (UUID).
        id: Uuid,
        /// New display name.
        new_name: String,
    },
    /// Delete an enrolled speaker (its diarized rows revert to anonymous).
    Delete {
        /// Speaker id (UUID).
        id: Uuid,
    },
    /// Merge one speaker into another, reassigning all references, then delete
    /// the source.
    Merge {
        /// Source speaker id (will be deleted).
        from_id: Uuid,
        /// Destination speaker id (kept).
        into_id: Uuid,
    },
}

#[derive(Debug, Subcommand)]
enum ModelsCmd {
    /// Report expected ONNX assets, their presence, and configured Ollama models.
    List,
    /// Verify model presence and print exactly what to fetch and where.
    Pull,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Tracing first, so config-load errors below are logged consistently. The
    // verbosity flag sets a default filter; `RUST_LOG` (read inside init_tracing)
    // still wins when present.
    apply_verbosity(cli.verbose);
    scribe_core::init_tracing(cli.json_logs);

    match run(&cli).await {
        Ok(code) => code,
        Err(e) => {
            // One clean stderr line + non-zero exit. `code()` gives a stable tag.
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Translate `-v`/`-vv` into a `RUST_LOG` default when the user hasn't set one.
fn apply_verbosity(verbose: u8) {
    if std::env::var_os("RUST_LOG").is_some() {
        return;
    }
    let level = match verbose {
        0 => return,
        1 => "debug",
        _ => "trace",
    };
    // sqlx/hyper stay quieter even at high verbosity to keep output readable.
    std::env::set_var("RUST_LOG", format!("{level},sqlx=info,hyper=info"));
}

/// Load config, then dispatch. Returns the process exit code (most commands
/// return `SUCCESS`; `doctor` may return `FAILURE` without being an error).
async fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    let cfg = Config::load(cli.config.as_deref())?;

    match &cli.command {
        Command::Serve(args) => {
            let mut cfg = cfg;
            if let Some(bind) = &args.bind {
                cfg.api.bind = bind.clone();
            }
            scribe_api::serve(cfg).await?;
            Ok(ExitCode::SUCCESS)
        }

        Command::Worker(args) => {
            let mut cfg = cfg;
            if let Some(stages) = &args.stages {
                cfg.worker.stages = stages
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(n) = args.concurrency {
                cfg.worker.concurrency = n;
            }
            if let Some(dir) = &args.models_dir {
                cfg.worker.models_dir = dir.clone();
            }
            scribe_pipeline::run_worker(cfg).await?;
            Ok(ExitCode::SUCCESS)
        }

        Command::Migrate(args) => {
            if !args.to.eq_ignore_ascii_case("latest") {
                anyhow::bail!("unsupported migrate target `{}` (only `latest`)", args.to);
            }
            let db = connect(&cfg).await?;
            db.run_migrations().await?;
            let applied = applied_migration_count(&db).await.unwrap_or(0);
            println!("migrations applied (target: latest) — {applied} migration(s) recorded");
            Ok(ExitCode::SUCCESS)
        }

        Command::Ingest(args) => {
            let db = connect(&cfg).await?;
            let id = scribe_pipeline::ingest_file(
                &cfg,
                &db,
                &args.file,
                args.title.clone(),
                args.participants,
                args.inline,
            )
            .await?;
            // The recording id is the one piece of machine-readable output here.
            println!("{id}");
            if args.inline {
                eprintln!("ingested and processed recording {id} (status: ready)");
            } else {
                eprintln!("ingested recording {id} (queued for processing)");
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Reindex(args) => {
            // Neither flag → do both (design §4).
            let (embeddings, summaries) = if !args.embeddings && !args.summaries {
                (true, true)
            } else {
                (args.embeddings, args.summaries)
            };
            let db = connect(&cfg).await?;
            scribe_pipeline::reindex(&cfg, &db, embeddings, summaries).await?;
            eprintln!(
                "reindex enqueued (embeddings: {embeddings}, summaries: {summaries}) — \
                 a running worker will pick the jobs up"
            );
            Ok(ExitCode::SUCCESS)
        }

        Command::Enroll(args) => {
            let db = connect(&cfg).await?;
            let id = scribe_pipeline::enroll(&cfg, &db, &args.name, &args.audio).await?;
            println!("{id}");
            eprintln!("enrolled speaker `{}` as {id}", args.name);
            Ok(ExitCode::SUCCESS)
        }

        Command::Speaker(cmd) => {
            let db = connect(&cfg).await?;
            run_speaker(&db, cmd).await?;
            Ok(ExitCode::SUCCESS)
        }

        Command::Models(cmd) => {
            match cmd {
                ModelsCmd::List => models::list(&cfg),
                ModelsCmd::Pull => models::pull(&cfg).await,
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Doctor => {
            let ok = doctor::run(&cfg, cli.config.as_deref()).await;
            Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Connect to Postgres, wrapping the failure in actionable context.
async fn connect(cfg: &Config) -> anyhow::Result<Db> {
    Db::connect(&cfg.database).await.map_err(|e| {
        anyhow::anyhow!(
            "could not connect to the database ({}): {e}",
            redact_url(&cfg.database.url)
        )
    })
}

/// Count rows in `_sqlx_migrations` (the table sqlx records applied versions in).
/// Best-effort — used only for a friendly post-migrate message.
async fn applied_migration_count(db: &Db) -> anyhow::Result<i64> {
    use sqlx::Row;
    let row = sqlx::query("SELECT count(*) AS n FROM _sqlx_migrations")
        .fetch_one(db.pool())
        .await?;
    Ok(row.try_get::<i64, _>("n")?)
}

/// Hide any password embedded in a connection URL before logging it.
fn redact_url(url: &str) -> String {
    // postgres://user:pass@host/db → postgres://user:***@host/db
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            let creds = &rest[..at];
            let tail = &rest[at..];
            if let Some(colon) = creds.find(':') {
                return format!("{scheme}{}:***{tail}", &creds[..colon]);
            }
        }
    }
    url.to_string()
}

// ---------------------------------------------------------------------------
// `speaker` subcommand
// ---------------------------------------------------------------------------

async fn run_speaker(db: &Db, cmd: &SpeakerCmd) -> anyhow::Result<()> {
    match cmd {
        SpeakerCmd::List => {
            let speakers = db.list_speakers().await?;
            if speakers.is_empty() {
                println!("no enrolled speakers");
                return Ok(());
            }
            println!("{:<38}  {:<6}  {}", "ID", "EMBED", "NAME");
            for s in &speakers {
                let embed = if s.embedding.is_some() { "yes" } else { "no" };
                println!("{:<38}  {:<6}  {}", s.id.to_string(), embed, s.display_name);
            }
            Ok(())
        }
        SpeakerCmd::Rename { id, new_name } => {
            db.rename_speaker(*id, new_name).await?;
            println!("renamed speaker {id} → `{new_name}`");
            Ok(())
        }
        SpeakerCmd::Delete { id } => {
            db.delete_speaker(*id).await?;
            println!("deleted speaker {id}");
            Ok(())
        }
        SpeakerCmd::Merge { from_id, into_id } => {
            merge_speakers(db, *from_id, *into_id).await
        }
    }
}

/// Merge `from_id` into `into_id`: validate both exist, reassign every
/// `recording_speakers.speaker_id` reference, then delete the source. Done in one
/// transaction via the raw pool so a half-merge can't leave dangling references.
async fn merge_speakers(db: &Db, from_id: Uuid, into_id: Uuid) -> anyhow::Result<()> {
    if from_id == into_id {
        anyhow::bail!("cannot merge a speaker into itself ({from_id})");
    }
    // Surfaces NotFound for either side before we touch anything.
    db.get_speaker(from_id).await?;
    db.get_speaker(into_id).await?;

    let mut tx = db.pool().begin().await?;
    let reassigned = sqlx::query(
        "UPDATE recording_speakers SET speaker_id = $2 WHERE speaker_id = $1",
    )
    .bind(from_id)
    .bind(into_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM speakers WHERE id = $1")
        .bind(from_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    println!(
        "merged speaker {from_id} into {into_id} ({reassigned} recording reference(s) reassigned)"
    );
    Ok(())
}
