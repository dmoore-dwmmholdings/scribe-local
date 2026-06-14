//! Pure path helpers for the on-disk blob layout (design §10). No I/O — both
//! the API node (writing uploaded segments, serving audio) and the worker
//! (reading segments, writing the transcoded WAV) build paths through here so
//! the layout can never drift between them.
//!
//! ```text
//! {blobs}/
//!   {recording_id}/
//!     segments/000001.m4a 000002.m4a …   # raw uploaded chunks
//!     audio.wav                           # transcoded 16k mono (cache)
//! ```

use std::path::{Path, PathBuf};

use uuid::Uuid;

/// The storage-key prefix recorded in `recordings.storage_key`: just the
/// recording id as text. Everything else is derived from it.
pub fn storage_key(recording_id: Uuid) -> String {
    recording_id.to_string()
}

/// `{blobs}/{recording_id}`
pub fn recording_dir(blobs: &Path, recording_id: Uuid) -> PathBuf {
    blobs.join(recording_id.to_string())
}

/// `{blobs}/{recording_id}/segments`
pub fn segments_dir(blobs: &Path, recording_id: Uuid) -> PathBuf {
    recording_dir(blobs, recording_id).join("segments")
}

/// `{blobs}/{recording_id}/segments/000123.{ext}` for a 1-based sequence number.
///
/// `ext` is the container of the uploaded chunk (e.g. `"m4a"`); leading dots
/// are tolerated.
pub fn segment_path(blobs: &Path, recording_id: Uuid, seq: i32, ext: &str) -> PathBuf {
    let ext = ext.trim_start_matches('.');
    segments_dir(blobs, recording_id).join(format!("{seq:06}.{ext}"))
}

/// The per-segment storage key stored in `segments.storage_key`, relative to
/// the blob root so it stays portable if the root moves.
pub fn segment_key(recording_id: Uuid, seq: i32, ext: &str) -> String {
    let ext = ext.trim_start_matches('.');
    format!("{recording_id}/segments/{seq:06}.{ext}")
}

/// `{blobs}/{recording_id}/audio.wav` — the transcoded 16 kHz mono cache.
pub fn wav_path(blobs: &Path, recording_id: Uuid) -> PathBuf {
    recording_dir(blobs, recording_id).join("audio.wav")
}

/// Resolve a stored key (relative to the blob root) back to an absolute path.
pub fn resolve_key(blobs: &Path, key: &str) -> PathBuf {
    blobs.join(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_paths_are_zero_padded_and_nested() {
        let id = Uuid::nil();
        let blobs = Path::new("/var/lib/scribe/blobs");
        let p = segment_path(blobs, id, 123, ".m4a");
        assert!(p.ends_with("00000000-0000-0000-0000-000000000000/segments/000123.m4a"));
        assert_eq!(
            segment_key(id, 7, "m4a"),
            "00000000-0000-0000-0000-000000000000/segments/000007.m4a"
        );
        assert!(wav_path(blobs, id).ends_with("audio.wav"));
    }
}
