//! Install a verified package: stage → sanity-check → migrate → atomic swap,
//! keeping a `.old` backup for rollback. None of this restarts the process;
//! the caller invokes [`crate::restart`] once it has answered the client.

use std::path::{Path, PathBuf};
use std::process::Command;

use scribe_core::config::UpdateConfig;

use crate::package::Package;
use crate::verify::verify_package;
use crate::{Result, UpdateError};

/// Knobs for [`apply_package`]. Defaults run the full real flow; tests disable
/// the sanity/migrate sub-steps that would execute the (fake) binary.
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// DB URL passed to the new binary's `migrate` (via `SCRIBE_DATABASE__URL`).
    pub db_url: Option<String>,
    /// Run `staged --version` before committing the swap.
    pub run_sanity_check: bool,
    /// Run `staged migrate` before committing the swap.
    pub run_migrations: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            db_url: None,
            run_sanity_check: true,
            run_migrations: true,
        }
    }
}

/// What an install changed.
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    pub from_version: String,
    pub to_version: String,
    pub target: String,
    pub binary_path: PathBuf,
    pub backup_path: PathBuf,
}

/// Verify, then install `pkg` over the running binary. The currently-executing
/// process keeps running its in-memory image; the new bytes only take effect on
/// restart. Returns once the swap is durably in place.
pub fn apply_package(cfg: &UpdateConfig, pkg: &Package, opts: &ApplyOptions) -> Result<UpdateOutcome> {
    // Verify here regardless of caller: apply is the trust boundary. (The
    // `enabled` flag gates the *HTTP* endpoint, not local CLI installs, so it is
    // checked in the API middleware rather than here.)
    verify_package(cfg, pkg)?;

    let binary_path = resolve_binary_path(cfg)?;
    let staged = sibling(&binary_path, "new");
    let backup = sibling(&binary_path, "old");

    // 1. Stage the new binary next to the target (same filesystem → atomic rename).
    if let Some(parent) = staged.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&staged, &pkg.binary)?;
    set_executable(&staged)?;

    // From here, clean up `staged` on any early return.
    let result = (|| {
        // 2. Sanity: the staged binary must at least run and report a version.
        if opts.run_sanity_check {
            let out = Command::new(&staged)
                .arg("--version")
                .output()
                .map_err(|e| UpdateError::SanityCheck(format!("spawning staged binary: {e}")))?;
            if !out.status.success() {
                return Err(UpdateError::SanityCheck(format!(
                    "`--version` exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }

        // 3. Migrate with the NEW binary (its migrations may be newer than ours).
        if opts.run_migrations {
            if let Some(url) = opts.db_url.as_deref().filter(|u| !u.is_empty()) {
                let out = Command::new(&staged)
                    .arg("migrate")
                    .env("SCRIBE_DATABASE__URL", url)
                    .output()
                    .map_err(|e| UpdateError::Migration(format!("spawning migrate: {e}")))?;
                if !out.status.success() {
                    return Err(UpdateError::Migration(
                        String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    ));
                }
            }
        }

        // 4. Atomic swap: back up the current binary, then move staged into place.
        if binary_path.exists() {
            // overwrite any prior backup
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(&binary_path, &backup).map_err(|e| {
                UpdateError::Io(std::io::Error::new(
                    e.kind(),
                    format!("backing up {}: {e}", binary_path.display()),
                ))
            })?;
        }
        if let Err(e) = std::fs::rename(&staged, &binary_path) {
            // Roll the backup back so we never leave the binary missing.
            if backup.exists() {
                let _ = std::fs::rename(&backup, &binary_path);
            }
            return Err(UpdateError::Io(std::io::Error::new(
                e.kind(),
                format!("installing new binary at {}: {e}", binary_path.display()),
            )));
        }
        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    Ok(UpdateOutcome {
        from_version: crate::running_version().to_string(),
        to_version: pkg.manifest.version.clone(),
        target: pkg.manifest.target.clone(),
        binary_path,
        backup_path: backup,
    })
}

/// Restore the `.old` backup over the current binary. Returns the restored
/// binary's reported version (best-effort).
pub fn rollback(cfg: &UpdateConfig) -> Result<String> {
    let binary_path = resolve_binary_path(cfg)?;
    let backup = sibling(&binary_path, "old");
    if !backup.exists() {
        return Err(UpdateError::NoBackup);
    }
    // Swap current ↔ backup so a rollback is itself reversible.
    let pending = sibling(&binary_path, "rollback-tmp");
    let _ = std::fs::remove_file(&pending);
    if binary_path.exists() {
        std::fs::rename(&binary_path, &pending)?;
    }
    std::fs::rename(&backup, &binary_path)?;
    if pending.exists() {
        // The just-replaced (newer) binary becomes the new backup.
        let _ = std::fs::rename(&pending, &backup);
    }

    let version = Command::new(&binary_path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(version)
}

/// Resolve which on-disk binary to replace: config override, else current exe.
pub(crate) fn resolve_binary_path(cfg: &UpdateConfig) -> Result<PathBuf> {
    match &cfg.binary_path {
        Some(p) => Ok(p.clone()),
        None => std::env::current_exe()
            .map_err(|e| UpdateError::Other(format!("cannot resolve current_exe(): {e}"))),
    }
}

/// `path` with its extension replaced by `ext` (kept on the same filesystem).
fn sibling(path: &Path, ext: &str) -> PathBuf {
    let mut p = path.to_path_buf();
    // Preserve `.exe` then append, e.g. `scribe.exe` → `scribe.exe.new`.
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "scribe".to_string());
    p.set_file_name(format!("{name}.{ext}"));
    p
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}
