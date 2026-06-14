//! Verify a package: ed25519 signature over the manifest, sha256 of the binary,
//! and the version/target policy from config.

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

use scribe_core::config::UpdateConfig;

use crate::package::{current_target, Package};
use crate::{Result, UpdateError};

/// Fully verify a package against the configured public key and policy.
/// Returns Ok(()) iff the package is authentic, intact, and installable.
pub fn verify_package(cfg: &UpdateConfig, pkg: &Package) -> Result<()> {
    // 1. Signature (authenticity). The manifest bytes must be signed by the
    //    operator's ed25519 key.
    let pubkey_hex = cfg
        .public_key
        .as_ref()
        .ok_or_else(|| UpdateError::Misconfigured("no [update].public_key set".into()))?;
    let verifying_key = parse_public_key(pubkey_hex)?;
    let signature = parse_signature(&pkg.signature_hex)?;
    verifying_key
        .verify_strict(&pkg.manifest_bytes, &signature)
        .map_err(|_| UpdateError::BadSignature)?;

    // 2. Checksum (integrity). The binary must hash to what the (now-trusted)
    //    manifest declares.
    let actual = hex::encode(Sha256::digest(&pkg.binary));
    let expected = pkg.manifest.sha256.trim().to_lowercase();
    if actual != expected {
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }

    // 3. name guard.
    if pkg.manifest.name != "scribe" {
        return Err(UpdateError::InvalidPackage(format!(
            "manifest name is `{}`, expected `scribe`",
            pkg.manifest.name
        )));
    }

    // 4. Target policy.
    let host = current_target();
    if pkg.manifest.target != host && !cfg.allow_target_mismatch && pkg.manifest.target != "any" {
        return Err(UpdateError::TargetMismatch {
            package: pkg.manifest.target.clone(),
            host,
        });
    }

    // 5. Downgrade policy (best-effort semver-ish compare on dotted numbers).
    if !cfg.allow_downgrade {
        let running = crate::running_version();
        if version_le(&pkg.manifest.version, running) {
            return Err(UpdateError::Downgrade {
                package: pkg.manifest.version.clone(),
                running: running.to_string(),
            });
        }
    }

    Ok(())
}

fn parse_public_key(hex_str: &str) -> Result<VerifyingKey> {
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| UpdateError::Misconfigured(format!("public_key is not hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| UpdateError::Misconfigured("public_key must be 32 bytes".into()))?;
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| UpdateError::Misconfigured(format!("invalid ed25519 public key: {e}")))
}

fn parse_signature(hex_str: &str) -> Result<Signature> {
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| UpdateError::InvalidPackage(format!("signature is not hex: {e}")))?;
    let arr: [u8; 64] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| UpdateError::InvalidPackage("signature must be 64 bytes".into()))?;
    Ok(Signature::from_bytes(&arr))
}

/// `a <= b` for dotted-numeric versions (e.g. "0.2.0" vs "0.10.1"). Non-numeric
/// segments compare as 0; missing segments are 0. Good enough for downgrade
/// protection without pulling a full semver dep.
fn version_le(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    pa <= pb
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    // Strip a leading 'v' and any pre-release/build suffix.
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.').map(|s| s.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}
