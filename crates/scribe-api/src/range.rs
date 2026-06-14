//! HTTP range serving for audio scrub/seek (design §10: "Serve with HTTP range
//! requests").
//!
//! We implement the single-range subset of RFC 7233 directly so we keep full
//! control over path resolution (404 when the transcoded WAV doesn't exist yet)
//! and can stream straight off disk without buffering the whole file:
//!
//! * always advertise `Accept-Ranges: bytes`;
//! * no/blank/un-parseable `Range` → 200 with the whole body;
//! * a satisfiable `bytes=start-end` → 206 with `Content-Range` and just that
//!   slice (streamed);
//! * an unsatisfiable range → 416 with `Content-Range: bytes */len`.
//!
//! The body is a `tokio::io::AsyncRead` wrapped as an HTTP stream via
//! `tokio_util::io::ReaderStream`, so a large WAV is never read into memory.

use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;

use scribe_core::Error;

use crate::error::ApiError;

/// Serve `path` with single-range support, honouring the request's `Range`
/// header. `content_type` is the MIME type to advertise (e.g. `audio/wav`).
///
/// Returns [`Error::NotFound`] (→ 404) when the file is absent, so callers can
/// just `?` it.
pub async fn serve_file_range(
    path: &Path,
    content_type: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError(Error::NotFound(format!(
                "audio not found: {}",
                path.display()
            ))));
        }
        Err(e) => return Err(ApiError(Error::Io(e))),
    };
    let total = file.metadata().await.map_err(Error::Io)?.len();

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(ct) = HeaderValue::from_str(content_type) {
        resp_headers.insert(header::CONTENT_TYPE, ct);
    }

    // No Range header → whole body, 200.
    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let Some(range_header) = range_header else {
        return Ok(full_body(file, total, resp_headers));
    };

    match parse_range(range_header, total) {
        ParsedRange::Full => Ok(full_body(file, total, resp_headers)),
        ParsedRange::Unsatisfiable => {
            resp_headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}")).unwrap(),
            );
            Ok((StatusCode::RANGE_NOT_SATISFIABLE, resp_headers).into_response())
        }
        ParsedRange::Bytes { start, end } => {
            // `end` is inclusive per RFC 7233.
            let len = end - start + 1;
            file.seek(SeekFrom::Start(start)).await.map_err(Error::Io)?;
            let limited = file.take(len);
            let stream = ReaderStream::new(limited);
            let body = Body::from_stream(stream);

            resp_headers.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
            );
            resp_headers.insert(
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).unwrap(),
            );
            Ok((StatusCode::PARTIAL_CONTENT, resp_headers, body).into_response())
        }
    }
}

/// Build a 200 response streaming the whole file.
fn full_body(file: tokio::fs::File, total: u64, mut headers: HeaderMap) -> Response {
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&total.to_string()).unwrap(),
    );
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    (StatusCode::OK, headers, body).into_response()
}

/// The outcome of parsing a `Range` header against a known total length.
enum ParsedRange {
    /// Serve the whole body (no usable range; we degrade to 200).
    Full,
    /// A satisfiable single byte range, inclusive bounds.
    Bytes { start: u64, end: u64 },
    /// A syntactically valid range that can't be satisfied → 416.
    Unsatisfiable,
}

/// Parse a `Range: bytes=…` header. Supports a single range in the forms
/// `start-end`, `start-` and `-suffix`. Multi-range and non-`bytes` units
/// degrade to [`ParsedRange::Full`] (we serve the whole body rather than error).
fn parse_range(value: &str, total: u64) -> ParsedRange {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return ParsedRange::Full;
    };
    // Only handle the first range; ignore additional comma-separated ranges.
    let first = spec.split(',').next().unwrap_or("").trim();
    let Some((start_s, end_s)) = first.split_once('-') else {
        return ParsedRange::Full;
    };

    if total == 0 {
        return ParsedRange::Unsatisfiable;
    }
    let last = total - 1;

    let (start, end) = if start_s.is_empty() {
        // Suffix range: `-N` = the last N bytes.
        let Ok(suffix) = end_s.parse::<u64>() else {
            return ParsedRange::Full;
        };
        if suffix == 0 {
            return ParsedRange::Unsatisfiable;
        }
        let len = suffix.min(total);
        (total - len, last)
    } else {
        let Ok(start) = start_s.parse::<u64>() else {
            return ParsedRange::Full;
        };
        if start > last {
            return ParsedRange::Unsatisfiable;
        }
        let end = if end_s.is_empty() {
            last
        } else {
            match end_s.parse::<u64>() {
                Ok(e) => e.min(last),
                Err(_) => return ParsedRange::Full,
            }
        };
        if end < start {
            return ParsedRange::Unsatisfiable;
        }
        (start, end)
    };

    ParsedRange::Bytes { start, end }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: &str, total: u64) -> Option<(u64, u64)> {
        match parse_range(value, total) {
            ParsedRange::Bytes { start, end } => Some((start, end)),
            _ => None,
        }
    }

    #[test]
    fn parses_closed_range() {
        assert_eq!(bytes("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(bytes("bytes=100-199", 1000), Some((100, 199)));
    }

    #[test]
    fn parses_open_ended_range() {
        assert_eq!(bytes("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn parses_suffix_range() {
        assert_eq!(bytes("bytes=-100", 1000), Some((900, 999)));
        // Suffix larger than file clamps to the whole file.
        assert_eq!(bytes("bytes=-5000", 1000), Some((0, 999)));
    }

    #[test]
    fn clamps_end_past_eof() {
        assert_eq!(bytes("bytes=900-100000", 1000), Some((900, 999)));
    }

    #[test]
    fn unsatisfiable_start_past_eof() {
        assert!(matches!(
            parse_range("bytes=2000-3000", 1000),
            ParsedRange::Unsatisfiable
        ));
    }

    #[test]
    fn non_bytes_unit_is_full() {
        assert!(matches!(parse_range("items=0-10", 1000), ParsedRange::Full));
        assert!(matches!(parse_range("garbage", 1000), ParsedRange::Full));
    }
}
