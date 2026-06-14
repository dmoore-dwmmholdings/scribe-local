//! Hybrid search + RAG ask (design §9).
//!
//! * `GET /search` embeds the query, runs [`Db::hybrid_search`] (keyword + vector
//!   fused with RRF), and returns the hits.
//! * `POST /ask` embeds the question, retrieves the top-k chunks, stuffs them
//!   into a RAG prompt (each with its recording title + timestamp), asks Ollama,
//!   and returns the answer plus citations mapped from the retrieved chunks.
//!   Robust to Ollama being down: the citations are still returned and the
//!   `answer` carries a placeholder so the mobile app degrades gracefully.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use scribe_core::types::{Answer, Citation, SearchHit};
use scribe_db::search::SearchFilters;
use scribe_llm::ChatMessage;

use crate::error::ApiResult;
use crate::state::AppState;

/// Default chunks to retrieve for a RAG answer when `top_k` is omitted.
const DEFAULT_TOP_K: i64 = 8;
/// Hard cap so a caller can't ask for an unbounded retrieval.
const MAX_TOP_K: i64 = 50;
/// Default search result count.
const DEFAULT_SEARCH_LIMIT: i64 = 20;

// --------------------------------------------------------------------------
// GET /search
// --------------------------------------------------------------------------

/// Query for `GET /search?q=&from=&to=&speaker=&recording=&limit=`.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub speaker: Option<Uuid>,
    pub recording: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
}

/// `GET /search` — embed the query and run hybrid search.
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<SearchResponse>> {
    let limit = q.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).clamp(1, 200);
    let filters = SearchFilters {
        from: q.from,
        to: q.to,
        speaker_id: q.speaker,
        recording_id: q.recording,
    };

    let embedding = state.embedder.embed_one(&q.q).await?;
    let hits = state
        .db
        .hybrid_search(&q.q, &embedding, &filters, limit)
        .await?;
    Ok(Json(SearchResponse { hits }))
}

// --------------------------------------------------------------------------
// POST /ask
// --------------------------------------------------------------------------

/// Filters embedded in the `POST /ask` body.
#[derive(Debug, Default, Deserialize)]
pub struct AskFilters {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub speaker: Option<Uuid>,
    pub recording: Option<Uuid>,
}

/// Body for `POST /ask`.
#[derive(Debug, Deserialize)]
pub struct AskBody {
    pub question: String,
    #[serde(default)]
    pub filters: Option<AskFilters>,
    pub top_k: Option<i64>,
}

/// `POST /ask` — retrieval-augmented answer with citations.
pub async fn ask(
    State(state): State<AppState>,
    Json(body): Json<AskBody>,
) -> ApiResult<Json<Answer>> {
    let top_k = body.top_k.unwrap_or(DEFAULT_TOP_K).clamp(1, MAX_TOP_K);
    let filters = match body.filters {
        Some(f) => SearchFilters {
            from: f.from,
            to: f.to,
            speaker_id: f.speaker,
            recording_id: f.recording,
        },
        None => SearchFilters::default(),
    };

    // Retrieve.
    let embedding = state.embedder.embed_one(&body.question).await?;
    let hits = state
        .db
        .hybrid_search(&body.question, &embedding, &filters, top_k)
        .await?;

    let citations: Vec<Citation> = hits.iter().map(hit_to_citation).collect();

    // No context → say so without bothering the LLM.
    if hits.is_empty() {
        return Ok(Json(Answer {
            answer: "I couldn't find anything relevant in your recordings to answer that."
                .to_string(),
            citations,
        }));
    }

    // Build the RAG prompt and ask the model. If Ollama is unreachable we still
    // return the citations with a placeholder answer (design: robust to no LLM).
    let messages = build_prompt(&body.question, &hits);
    let answer = match state
        .ollama
        .chat(&state.cfg.llm.summarize_model, &messages)
        .await
    {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            tracing::warn!(error = %e, "ollama chat failed; returning citations with placeholder answer");
            format!(
                "(Could not generate an answer — the language model is unavailable: {e}. \
                 See the cited excerpts below.)"
            )
        }
    };

    Ok(Json(Answer { answer, citations }))
}

/// Map a retrieved [`SearchHit`] to a response [`Citation`].
fn hit_to_citation(hit: &SearchHit) -> Citation {
    Citation {
        recording_id: hit.recording_id,
        recording_title: hit.recording_title.clone(),
        start_ms: hit.start_ms,
        end_ms: hit.end_ms,
        snippet: hit.text.clone(),
    }
}

/// Construct the RAG chat prompt: a system instruction to answer only from the
/// provided context and cite sources, then the numbered context excerpts (each
/// tagged with recording title + timestamp) and the user's question.
fn build_prompt(question: &str, hits: &[SearchHit]) -> Vec<ChatMessage> {
    let system = "You are a meeting assistant. Answer the user's question using \
                  ONLY the numbered excerpts from their meeting transcripts below. \
                  Cite the excerpts you use inline like [1], [2]. If the excerpts \
                  do not contain the answer, say you don't have enough information. \
                  Be concise.";

    let mut context = String::new();
    for (i, hit) in hits.iter().enumerate() {
        let n = i + 1;
        let title = hit.recording_title.as_deref().unwrap_or("Untitled recording");
        let ts = format_timestamp(hit.start_ms);
        context.push_str(&format!("[{n}] ({title}{ts})\n{}\n\n", hit.text.trim()));
    }

    let user = format!(
        "Context excerpts:\n\n{context}\nQuestion: {question}\n\n\
         Answer using the excerpts above and cite them inline."
    );

    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Render a millisecond offset as `, mm:ss` for the prompt, or empty if absent.
fn format_timestamp(start_ms: Option<i64>) -> String {
    match start_ms {
        Some(ms) if ms >= 0 => {
            let total_secs = ms / 1000;
            let minutes = total_secs / 60;
            let seconds = total_secs % 60;
            format!(", {minutes:02}:{seconds:02}")
        }
        _ => String::new(),
    }
}
