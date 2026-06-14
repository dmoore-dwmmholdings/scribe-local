//! Restart the process into the freshly-installed binary.

use scribe_core::config::{RestartMode, UpdateConfig};

use crate::apply::resolve_binary_path;
use crate::Result;

/// Restart according to [`UpdateConfig::restart`].
///
/// * [`RestartMode::SelfExec`] — replace this process image with the new binary
///   (`execv`) using the same argv the process was launched with. On Unix this
///   does not return on success. On non-Unix it spawns a detached child and
///   exits (best-effort, since you can't `execv` a running image there).
/// * [`RestartMode::Supervisor`] — exit cleanly and let systemd/launchd relaunch.
///
/// In supervisor mode this never returns (it exits the process).
pub fn restart(cfg: &UpdateConfig) -> Result<()> {
    match cfg.restart {
        RestartMode::Supervisor => {
            tracing::info!("update: exiting for service-manager restart");
            std::process::exit(0);
        }
        RestartMode::SelfExec => self_exec(cfg),
    }
}

#[cfg(unix)]
fn self_exec(cfg: &UpdateConfig) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let bin = resolve_binary_path(cfg)?;
    // Re-run with the same arguments (everything after argv[0]).
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    tracing::info!(binary = %bin.display(), "update: re-exec into new binary");

    // `exec` only returns on failure.
    let err = std::process::Command::new(&bin).args(&args).exec();
    Err(crate::UpdateError::Other(format!(
        "execv {} failed: {err}",
        bin.display()
    )))
}

#[cfg(not(unix))]
fn self_exec(cfg: &UpdateConfig) -> Result<()> {
    let bin = resolve_binary_path(cfg)?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    tracing::info!(binary = %bin.display(), "update: spawn new binary and exit");
    std::process::Command::new(&bin).args(&args).spawn()?;
    std::process::exit(0);
}
