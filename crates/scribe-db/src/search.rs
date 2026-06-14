//! Hybrid search over retrieval chunks (design §9).
//!
//! Combines two rankings of the `chunks` table:
//!   * **keyword** — Postgres FTS over the generated `tsv` column
//!     (`tsv @@ plainto_tsquery`), ranked by `ts_rank_cd`.
//!   * **semantic** — pgvector KNN over the `halfvec(768)` embedding
//!     (`embedding <=> $query`), ranked by ascending cosine distance.
//!
//! The two ranked lists are fused with **Reciprocal Rank Fusion** (RRF, k≈60):
//! `score = Σ 1 / (k + rank)`. RRF needs only ranks, so the wildly different
//! score scales of FTS vs cosine distance never have to be normalized.
//!
//! Date / speaker / recording filters are applied inside both rankings. Because
//! we filter, the filtered-vector queries run in a transaction that sets
//! `hnsw.iterative_scan = relaxed_order` so the HNSW index keeps returning
//! candidates until `limit` survivors pass the filter (design §9).
//!
//! ### Filter parameter numbering
//!
//! Filters are bound *after* each query's fixed parameters. The fixed-bind count
//! differs per query (hybrid: 5, keyword/semantic: 2), so each caller passes its
//! own `base` (the count of fixed binds) to [`filter_clause`], which numbers the
//! optional placeholders `$base+1`, `$base+2`, … in a fixed order:
//! `from, to, speaker_id, recording_id`. [`bind_filters`] then binds the present
//! values in that same order.

use chrono::{DateTime, Utc};
use pgvector::HalfVector;
use scribe_core::types::SearchHit;
use scribe_core::Result;
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::Query;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::db_err;
use crate::Db;

/// RRF constant. 60 is the value from the original Cormack et al. paper and the
/// common default; it damps the contribution of low-ranked hits.
const RRF_K: i32 = 60;

/// Optional filters applied to every search (design §9: hybrid vector + SQL).
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Only chunks from recordings created at/after this time.
    pub from: Option<DateTime<Utc>>,
    /// Only chunks from recordings created at/before this time.
    pub to: Option<DateTime<Utc>>,
    /// Only chunks spoken by this enrolled speaker (via `recording_speakers`).
    pub speaker_id: Option<Uuid>,
    /// Restrict to a single recording (in-recording search).
    pub recording_id: Option<Uuid>,
}

impl SearchFilters {
    fn is_empty(&self) -> bool {
        self.from.is_none()
            && self.to.is_none()
            && self.speaker_id.is_none()
            && self.recording_id.is_none()
    }
}

impl Db {
    /// Hybrid keyword + semantic search, fused with RRF. Returns up to `limit`
    /// [`SearchHit`]s ordered by descending fused score.
    pub async fn hybrid_search(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        filters: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        let embedding = HalfVector::from_f32_slice(query_embedding);
        // Candidate pool per arm: a few × limit gives RRF room to re-rank.
        let candidate_n = limit.max(1) * 4;

        // Fixed binds: $1 query_text, $2 embedding, $3 candidate_n, $4 rrf_k,
        // $5 limit. Filters numbered from $6.
        let filter_sql = filter_clause(filters, 5);

        let sql = format!(
            "WITH kw AS ( \
               SELECT c.id, \
                      row_number() OVER ( \
                        ORDER BY ts_rank_cd(c.tsv, plainto_tsquery('english', $1)) DESC, c.id \
                      ) AS rnk \
               FROM chunks c \
               JOIN recordings r ON r.id = c.recording_id \
               WHERE c.tsv @@ plainto_tsquery('english', $1){filter_sql} \
               ORDER BY rnk LIMIT $3 \
             ), \
             vec AS ( \
               SELECT c.id, \
                      row_number() OVER (ORDER BY c.embedding <=> $2, c.id) AS rnk \
               FROM chunks c \
               JOIN recordings r ON r.id = c.recording_id \
               WHERE c.embedding IS NOT NULL{filter_sql} \
               ORDER BY c.embedding <=> $2 LIMIT $3 \
             ), \
             fused AS ( \
               SELECT id, sum(score) AS score FROM ( \
                 SELECT id, 1.0 / ($4 + rnk) AS score FROM kw \
                 UNION ALL \
                 SELECT id, 1.0 / ($4 + rnk) AS score FROM vec \
               ) s GROUP BY id \
             ) \
             SELECT c.recording_id, r.title AS recording_title, c.start_ms, c.end_ms, c.text, \
                    f.score::float8 AS score \
             FROM fused f \
             JOIN chunks c ON c.id = f.id \
             JOIN recordings r ON r.id = c.recording_id \
             ORDER BY f.score DESC, c.id \
             LIMIT $5"
        );

        let mut tx = self.pool().begin().await.map_err(db_err)?;
        // Filtered vector search: keep scanning the HNSW graph past filtered-out
        // rows until enough survivors are found (design §9).
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        let mut q = sqlx::query(&sql)
            .bind(query_text)
            .bind(&embedding)
            .bind(candidate_n)
            .bind(RRF_K)
            .bind(limit);
        q = bind_filters(q, filters);

        let rows = q.fetch_all(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        rows.iter().map(hit_from_row).collect()
    }

    /// Keyword-only search (Postgres FTS), ranked by `ts_rank_cd`.
    pub async fn keyword_search(
        &self,
        query_text: &str,
        filters: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        // Fixed binds: $1 query_text, $2 limit. Filters from $3.
        let filter_sql = filter_clause(filters, 2);
        let sql = format!(
            "SELECT c.recording_id, r.title AS recording_title, c.start_ms, c.end_ms, c.text, \
                    ts_rank_cd(c.tsv, plainto_tsquery('english', $1))::float8 AS score \
             FROM chunks c \
             JOIN recordings r ON r.id = c.recording_id \
             WHERE c.tsv @@ plainto_tsquery('english', $1){filter_sql} \
             ORDER BY score DESC, c.id \
             LIMIT $2"
        );
        let mut q = sqlx::query(&sql).bind(query_text).bind(limit);
        q = bind_filters(q, filters);
        let rows = q.fetch_all(self.pool()).await.map_err(db_err)?;
        rows.iter().map(hit_from_row).collect()
    }

    /// Semantic-only search (pgvector KNN). `score` is cosine similarity
    /// (`1 - distance`), higher is better.
    pub async fn semantic_search(
        &self,
        query_embedding: &[f32],
        filters: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        let embedding = HalfVector::from_f32_slice(query_embedding);
        // Fixed binds: $1 embedding, $2 limit. Filters from $3.
        let filter_sql = filter_clause(filters, 2);
        let sql = format!(
            "SELECT c.recording_id, r.title AS recording_title, c.start_ms, c.end_ms, c.text, \
                    (1 - (c.embedding <=> $1))::float8 AS score \
             FROM chunks c \
             JOIN recordings r ON r.id = c.recording_id \
             WHERE c.embedding IS NOT NULL{filter_sql} \
             ORDER BY c.embedding <=> $1 \
             LIMIT $2"
        );
        let mut tx = self.pool().begin().await.map_err(db_err)?;
        sqlx::query("SET LOCAL hnsw.iterative_scan = relaxed_order")
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let mut q = sqlx::query(&sql).bind(&embedding).bind(limit);
        q = bind_filters(q, filters);
        let rows = q.fetch_all(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        rows.iter().map(hit_from_row).collect()
    }
}

/// Build the trailing `AND …` WHERE clause for `filters`, numbering placeholders
/// starting at `$base+1` in the order `from, to, speaker_id, recording_id`.
/// `base` is the caller's fixed-bind count.
///
/// The speaker filter uses an EXISTS subquery against `recording_speakers` so
/// the chunk row stays unique even when a recording has several diarized
/// speakers (a JOIN would multiply rows).
fn filter_clause(filters: &SearchFilters, base: usize) -> String {
    if filters.is_empty() {
        return String::new();
    }
    let mut clause = String::new();
    let mut n = base;
    if filters.from.is_some() {
        n += 1;
        clause.push_str(&format!(" AND r.created_at >= ${n}"));
    }
    if filters.to.is_some() {
        n += 1;
        clause.push_str(&format!(" AND r.created_at <= ${n}"));
    }
    if filters.speaker_id.is_some() {
        n += 1;
        clause.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM recording_speakers rs \
                          WHERE rs.recording_id = c.recording_id AND rs.speaker_id = ${n})"
        ));
    }
    if filters.recording_id.is_some() {
        n += 1;
        clause.push_str(&format!(" AND c.recording_id = ${n}"));
    }
    clause
}

/// Bind the present filter values, in the same order [`filter_clause`] numbered
/// them: `from, to, speaker_id, recording_id`.
fn bind_filters<'q>(
    mut q: Query<'q, Postgres, PgArguments>,
    filters: &'q SearchFilters,
) -> Query<'q, Postgres, PgArguments> {
    if let Some(from) = filters.from {
        q = q.bind(from);
    }
    if let Some(to) = filters.to {
        q = q.bind(to);
    }
    if let Some(speaker_id) = filters.speaker_id {
        q = q.bind(speaker_id);
    }
    if let Some(recording_id) = filters.recording_id {
        q = q.bind(recording_id);
    }
    q
}

fn hit_from_row(row: &PgRow) -> Result<SearchHit> {
    let score: f64 = row.try_get("score").map_err(db_err)?;
    Ok(SearchHit {
        recording_id: row.try_get("recording_id").map_err(db_err)?,
        recording_title: row.try_get("recording_title").map_err(db_err)?,
        start_ms: row.try_get("start_ms").map_err(db_err)?,
        end_ms: row.try_get("end_ms").map_err(db_err)?,
        text: row.try_get("text").map_err(db_err)?,
        score: score as f32,
    })
}
