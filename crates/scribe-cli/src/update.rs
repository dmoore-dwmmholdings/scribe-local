//! The `scribe update` subcommands — the operator side of self-update
//! (keygen / sign / verify / apply / rollback / info). The frontend uses the
//! `POST /admin/update` API; these are for building and installing packages
//! from the command line. See `scribe-update`.

use std::path::Path;

use scribe_core::config::Config;
use scribe_update::{
    apply_package, current_target, generate_keypair, rollback, running_version, sign_package,
    verify_package, ApplyOptions, Keypair, Package,
};

use crate::UpdateCmd;

pub async fn run(cfg: &Config, cmd: &UpdateCmd) -> anyhow::Result<()> {
    match cmd {
        UpdateCmd::Keygen { out } => keygen(out.as_deref()),
        UpdateCmd::Sign {
            key,
            binary,
            version,
            target,
            notes,
            out,
        } => sign(key, binary, version, target.as_deref(), notes, out),
        UpdateCmd::Verify { package } => verify(cfg, package),
        UpdateCmd::Apply { package } => apply(cfg, package),
        UpdateCmd::Rollback => do_rollback(cfg),
        UpdateCmd::Info => info(cfg),
    }
}

fn keygen(out: Option<&Path>) -> anyhow::Result<()> {
    let kp = generate_keypair();
    let public = kp.public_hex();

    match out {
        Some(path) => {
            std::fs::write(path, format!("{}\n", kp.private_hex()))?;
            restrict_permissions(path);
            let pubpath = path.with_extension("pub");
            std::fs::write(&pubpath, format!("{public}\n"))?;
            eprintln!("private key → {} (KEEP SECRET, chmod 600)", path.display());
            eprintln!("public key  → {}", pubpath.display());
        }
        None => {
            eprintln!("private key (KEEP SECRET): {}", kp.private_hex());
        }
    }

    // The public half is what goes in the server config.
    println!();
    println!("# Add to your server config to enable signed self-update:");
    println!("[update]");
    println!("enabled = true");
    println!("public_key = \"{public}\"");
    println!("token = \"<choose-a-strong-random-token>\"");
    Ok(())
}

fn sign(
    key: &str,
    binary: &Path,
    version: &str,
    target: Option<&str>,
    notes: &str,
    out: &Path,
) -> anyhow::Result<()> {
    let key_hex = read_key(key)?;
    let kp = Keypair::from_private_hex(&key_hex).map_err(anyhow::Error::msg)?;
    // Real wall-clock timestamp is fine here (operator CLI, not a sandbox).
    let created = chrono::Utc::now().to_rfc3339();

    let manifest = sign_package(&kp, binary, version, target, notes, &created, out)
        .map_err(anyhow::Error::msg)?;

    eprintln!(
        "signed package → {}\n  version: {}\n  target:  {}\n  sha256:  {}",
        out.display(),
        manifest.version,
        manifest.target,
        manifest.sha256,
    );
    Ok(())
}

fn verify(cfg: &Config, package: &Path) -> anyhow::Result<()> {
    let pkg = Package::read(package).map_err(anyhow::Error::msg)?;
    print_manifest(&pkg);
    match verify_package(&cfg.update, &pkg) {
        Ok(()) => {
            println!("\nOK — signature and checksum valid; installable on this host.");
            Ok(())
        }
        Err(e) => anyhow::bail!("verification FAILED: {e}"),
    }
}

fn apply(cfg: &Config, package: &Path) -> anyhow::Result<()> {
    let pkg = Package::read(package).map_err(anyhow::Error::msg)?;
    let opts = ApplyOptions {
        db_url: Some(cfg.database.url.clone()),
        run_sanity_check: true,
        run_migrations: true,
    };
    let outcome = apply_package(&cfg.update, &pkg, &opts).map_err(anyhow::Error::msg)?;
    println!(
        "installed {} → {} ({})",
        outcome.from_version, outcome.to_version, outcome.target
    );
    println!("backup: {}", outcome.backup_path.display());
    eprintln!(
        "restart `scribe serve` to run the new binary \
         (the /admin/update API restarts automatically)."
    );
    Ok(())
}

fn do_rollback(cfg: &Config) -> anyhow::Result<()> {
    let version = rollback(&cfg.update).map_err(anyhow::Error::msg)?;
    println!("rolled back to {version}");
    eprintln!("restart `scribe serve` to run the restored binary.");
    Ok(())
}

fn info(cfg: &Config) -> anyhow::Result<()> {
    let binary = cfg
        .update
        .binary_path
        .clone()
        .or_else(|| std::env::current_exe().ok());
    let has_backup = binary
        .as_ref()
        .map(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            p.with_file_name(format!("{name}.old")).exists()
        })
        .unwrap_or(false);

    println!("version:     {}", running_version());
    println!("target:      {}", current_target());
    println!(
        "binary:      {}",
        binary.map(|p| p.display().to_string()).unwrap_or_default()
    );
    println!("update:      {}", if cfg.update.enabled { "enabled" } else { "disabled" });
    println!("rollback:    {}", if has_backup { "available (.old present)" } else { "none" });
    Ok(())
}

/// A `--key` value is either a path to a key file or the hex itself.
fn read_key(value: &str) -> anyhow::Result<String> {
    let p = Path::new(value);
    if p.exists() {
        Ok(std::fs::read_to_string(p)?.trim().to_string())
    } else {
        Ok(value.trim().to_string())
    }
}

fn print_manifest(pkg: &Package) {
    let m = &pkg.manifest;
    println!("package manifest:");
    println!("  name:       {}", m.name);
    println!("  version:    {}", m.version);
    println!("  target:     {}", m.target);
    println!("  sha256:     {}", m.sha256);
    println!("  created_at: {}", m.created_at);
    if !m.notes.is_empty() {
        println!("  notes:      {}", m.notes);
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
