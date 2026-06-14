//! sqlx → [`scribe_core::Error`] conversion.
//!
//! Per the crate contract we surface persistence failures as
//! [`Error::Database`] carrying the rendered sqlx message. `RowNotFound` is the
//! one case worth distinguishing — callers that `fetch_one` a single entity
//! want a [`Error::NotFound`] (HTTP 404), so individual queries map that
//! explicitly; the generic helper here keeps everything else as `Database`.

use scribe_core::Error;

/// Map any [`sqlx::Error`] to [`Error::Database`].
pub fn db_err(e: sqlx::Error) -> Error {
    Error::Database(e.to_string())
}
