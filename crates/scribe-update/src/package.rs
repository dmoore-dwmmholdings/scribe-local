//! Reading and writing the `.tar.gz` update package.

use std::io::{Read, Write};
use std::path::Path;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::manifest::Manifest;
use crate::{Result, UpdateError};

pub const MANIFEST_ENTRY: &str = "manifest.json";
pub const SIG_ENTRY: &str = "manifest.sig";
/// The binary entry name. We keep it stable (`scribe`) regardless of host so the
/// package is simple; the running binary's real path comes from config/current_exe.
pub const BINARY_ENTRY: &str = "scribe";

/// A parsed-but-unverified update package held in memory. Packages are small
/// (a single binary), so buffering is fine and avoids partial-extract states.
pub struct Package {
    /// Exact bytes of `manifest.json` (what the signature covers).
    pub manifest_bytes: Vec<u8>,
    /// Parsed manifest.
    pub manifest: Manifest,
    /// Hex ed25519 signature from `manifest.sig` (trimmed).
    pub signature_hex: String,
    /// The new binary's bytes.
    pub binary: Vec<u8>,
}

impl Package {
    /// Read and parse a package from a `.tar.gz` on disk. Does NOT verify the
    /// signature/checksum — call [`crate::verify_package`] for that.
    pub fn read(path: &Path) -> Result<Package> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(file)
    }

    /// Read a package from any reader yielding gzip(tar(...)).
    pub fn from_reader<R: Read>(reader: R) -> Result<Package> {
        let mut archive = tar::Archive::new(GzDecoder::new(reader));

        let mut manifest_bytes: Option<Vec<u8>> = None;
        let mut signature_hex: Option<String> = None;
        let mut binary: Option<Vec<u8>> = None;

        for entry in archive
            .entries()
            .map_err(|e| UpdateError::InvalidPackage(format!("not a tar archive: {e}")))?
        {
            let mut entry =
                entry.map_err(|e| UpdateError::InvalidPackage(format!("bad tar entry: {e}")))?;
            let path = entry
                .path()
                .map_err(|e| UpdateError::InvalidPackage(format!("bad entry path: {e}")))?
                .to_string_lossy()
                .to_string();
            // Match on the basename so a leading `./` or dir prefix is tolerated.
            let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);

            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            match name {
                MANIFEST_ENTRY => manifest_bytes = Some(buf),
                SIG_ENTRY => {
                    signature_hex = Some(String::from_utf8_lossy(&buf).trim().to_string());
                }
                BINARY_ENTRY => binary = Some(buf),
                // Also accept a windows-named binary.
                "scribe.exe" => binary = Some(buf),
                _ => { /* ignore extra files */ }
            }
        }

        let manifest_bytes = manifest_bytes
            .ok_or_else(|| UpdateError::InvalidPackage(format!("missing {MANIFEST_ENTRY}")))?;
        let signature_hex = signature_hex
            .ok_or_else(|| UpdateError::InvalidPackage(format!("missing {SIG_ENTRY}")))?;
        let binary =
            binary.ok_or_else(|| UpdateError::InvalidPackage(format!("missing {BINARY_ENTRY}")))?;
        let manifest = Manifest::from_bytes(&manifest_bytes)
            .map_err(|e| UpdateError::InvalidPackage(format!("bad manifest.json: {e}")))?;

        Ok(Package {
            manifest_bytes,
            manifest,
            signature_hex,
            binary,
        })
    }

    /// Write a package to `path` from its parts.
    pub fn write(
        path: &Path,
        manifest_bytes: &[u8],
        signature_hex: &str,
        binary: &[u8],
    ) -> Result<()> {
        let file = std::fs::File::create(path)?;
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = tar::Builder::new(enc);

        append(&mut tar, MANIFEST_ENTRY, manifest_bytes)?;
        append(&mut tar, SIG_ENTRY, signature_hex.as_bytes())?;
        append(&mut tar, BINARY_ENTRY, binary)?;

        let enc = tar
            .into_inner()
            .map_err(|e| UpdateError::Other(format!("finishing tar: {e}")))?;
        enc.finish()
            .map_err(|e| UpdateError::Other(format!("finishing gzip: {e}")))?
            .flush()?;
        Ok(())
    }
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, data: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    // The binary needs to be executable on Unix; harmless on Windows.
    header.set_mode(if name == BINARY_ENTRY { 0o755 } else { 0o644 });
    header.set_cksum();
    tar.append_data(&mut header, name, data)
        .map_err(|e| UpdateError::Other(format!("appending {name}: {e}")))?;
    Ok(())
}

/// The host target string, e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`.
/// Built from `std::env::consts` so it matches what `scribe update sign` stamps
/// by default on the same machine.
pub fn current_target() -> String {
    let arch = std::env::consts::ARCH; // x86_64, aarch64, …
    let os = std::env::consts::OS; // linux, macos, windows
    let triple = match os {
        "macos" => format!("{arch}-apple-darwin"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        "windows" => format!("{arch}-pc-windows-msvc"),
        other => format!("{arch}-{other}"),
    };
    triple
}
