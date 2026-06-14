//! The package manifest — the signed description of an update.

use serde::{Deserialize, Serialize};

/// Metadata describing an update package. The exact serialized bytes of this
/// struct (as written into `manifest.json`) are what the ed25519 signature
/// covers, and `sha256` binds the accompanying binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Always `"scribe"` for now; guards against cross-product packages.
    pub name: String,
    /// Semantic version of the new binary (e.g. `"0.2.0"`).
    pub version: String,
    /// Host triple the binary is built for (e.g. `"aarch64-apple-darwin"`).
    pub target: String,
    /// Lowercase hex sha256 of the binary bytes.
    pub sha256: String,
    /// RFC3339 build timestamp.
    pub created_at: String,
    /// Optional human release notes.
    #[serde(default)]
    pub notes: String,
}

impl Manifest {
    /// Serialize to the canonical bytes that get signed and shipped as
    /// `manifest.json`. Pretty JSON with a trailing newline — stable across
    /// round-trips so the signature verifies byte-for-byte.
    pub fn to_signed_bytes(&self) -> Vec<u8> {
        let mut v = serde_json::to_vec_pretty(self).expect("manifest serializes");
        v.push(b'\n');
        v
    }

    /// Parse from the bytes read out of `manifest.json`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
