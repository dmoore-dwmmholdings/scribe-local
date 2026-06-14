//! ed25519 key generation and package signing — the operator side of the
//! trust model. Used by `scribe update keygen` and `scribe update sign`.

use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::manifest::Manifest;
use crate::package::{current_target, Package};
use crate::{Result, UpdateError};

/// An ed25519 signing keypair. The public half (hex) goes in the server config;
/// the private half stays with whoever builds releases.
pub struct Keypair {
    signing: SigningKey,
}

impl Keypair {
    /// Generate a fresh keypair from the OS CSPRNG.
    pub fn generate() -> Keypair {
        let mut rng = rand_core::OsRng;
        Keypair {
            signing: SigningKey::generate(&mut rng),
        }
    }

    /// Load a keypair from a 32-byte hex private key.
    pub fn from_private_hex(hex_str: &str) -> Result<Keypair> {
        let bytes = hex::decode(hex_str.trim())
            .map_err(|e| UpdateError::Other(format!("private key not hex: {e}")))?;
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| UpdateError::Other("private key must be 32 bytes".into()))?;
        Ok(Keypair {
            signing: SigningKey::from_bytes(&arr),
        })
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// Hex of the public key — paste into `[update].public_key`.
    pub fn public_hex(&self) -> String {
        hex::encode(self.verifying_key().to_bytes())
    }

    /// Hex of the private key — keep secret.
    pub fn private_hex(&self) -> String {
        hex::encode(self.signing.to_bytes())
    }
}

/// Generate and return a new keypair.
pub fn generate_keypair() -> Keypair {
    Keypair::generate()
}

/// Build a signed update package from a binary on disk.
///
/// Computes the binary's sha256, writes a manifest (stamped with `target` —
/// defaulting to this host's triple — and `created_at`), signs the manifest
/// bytes with `key`, and writes the `.tar.gz` to `out`. Returns the manifest.
pub fn sign_package(
    key: &Keypair,
    binary_path: &Path,
    version: &str,
    target: Option<&str>,
    notes: &str,
    created_at_rfc3339: &str,
    out: &Path,
) -> Result<Manifest> {
    let binary = std::fs::read(binary_path)?;
    let sha256 = hex::encode(Sha256::digest(&binary));

    let manifest = Manifest {
        name: "scribe".to_string(),
        version: version.to_string(),
        target: target.map(|t| t.to_string()).unwrap_or_else(current_target),
        sha256,
        created_at: created_at_rfc3339.to_string(),
        notes: notes.to_string(),
    };
    let manifest_bytes = manifest.to_signed_bytes();
    let signature = key.signing.sign(&manifest_bytes);
    let signature_hex = hex::encode(signature.to_bytes());

    Package::write(out, &manifest_bytes, &signature_hex, &binary)?;
    Ok(manifest)
}
