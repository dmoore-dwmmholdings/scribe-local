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

/// One earlier turn of the conversation, as the client remembers it.
#[derive(Debug, Deserialize)]
pub struct AskTurn {
    /// `user` or `assistant`. Anything else is treated as `user`.
    pub role: String,
    pub content: String,
}

/// Body for `POST /ask`.
#[derive(Debug, Deserialize)]
pub struct AskBody {
    pub question: String,
    /// Earlier turns, oldest first, excluding the current question. Ask is a
    /// conversation, so a follow-up like "why?" only makes sense with these.
    #[serde(default)]
    pub history: Vec<AskTurn>,
    #[serde(default)]
    pub filters: Option<AskFilters>,
    pub top_k: Option<i64>,
}

/// How many earlier turns to carry into the prompt. Enough for a real
/// back-and-forth, bounded so a long thread cannot crowd out the excerpts.
const MAX_HISTORY_TURNS: usize = 12;

/// Longest earlier turn we replay verbatim; older answers get truncated rather
/// than dropped, so the thread stays coherent without eating the context.
const MAX_HISTORY_CHARS: usize = 1_200;

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

    // Retrieve. A follow-up ("what about the second one?") carries almost no
    // searchable content on its own, so the retrieval query also carries the
    // recent user turns. The conversation the model sees is unaffected.
    let retrieval_query = retrieval_query(&body.question, &body.history);
    let embedding = state.embedder.embed_one(&retrieval_query).await?;
    let hits = state
        .db
        .hybrid_search(&retrieval_query, &embedding, &filters, top_k)
        .await?;

    let citations: Vec<Citation> = hits.iter().map(hit_to_citation).collect();

    // Note that we do NOT short-circuit on an empty retrieval. "Thanks", "what
    // did I just ask?", or a question about something never recorded all match
    // nothing, and a canned "I couldn't find anything relevant" is a bad answer
    // to every one of them. The model gets the conversation either way and is
    // told plainly that the search came back empty.
    let messages = build_prompt(&body.question, &body.history, &hits);
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

/// Build the retrieval query.
///
/// Search sees the current question plus the last couple of user turns, because
/// a follow-up is often a pronoun away from meaningless ("and the other one?").
/// Assistant turns are left out: they repeat the excerpts, which would drag the
/// search back toward what was already found.
fn retrieval_query(question: &str, history: &[AskTurn]) -> String {
    const RECENT_USER_TURNS: usize = 2;
    let recent: Vec<&str> = history
        .iter()
        .filter(|t| !t.role.eq_ignore_ascii_case("assistant"))
        .map(|t| t.content.trim())
        .filter(|c| !c.is_empty())
        .rev()
        .take(RECENT_USER_TURNS)
        .collect();
    if recent.is_empty() {
        return question.to_string();
    }
    let mut q = String::new();
    for turn in recent.iter().rev() {
        q.push_str(turn);
        q.push(' ');
    }
    q.push_str(question);
    q
}

/// Construct the chat prompt: a system instruction, the earlier turns, then the
/// retrieved excerpts and the current question.
///
/// The instruction deliberately does NOT say "answer only from the excerpts".
/// That framing turns the assistant into a quote extractor: it refuses to add
/// two numbers that appear in a transcript, and it falls back to a canned "not
/// enough information" whenever retrieval misses. The excerpts are the source of
/// truth about what was *said*; reasoning over them is the assistant's job.
fn build_prompt(question: &str, history: &[AskTurn], hits: &[SearchHit]) -> Vec<ChatMessage> {
    let system = "You are the user's assistant for their own meeting recordings. You are having \
        a conversation, not filling in a form.\n\n\
        Excerpts retrieved from the user's transcripts are provided with each question. Treat \
        them as the record of what was actually said, and prefer them over your own assumptions \
        about the user's work.\n\n\
        How to answer:\n\
        - Answer the question that was asked, in a natural conversational voice. Follow-ups \
          refer to the conversation so far.\n\
        - Reason over the excerpts: compare, count, calculate, draw conclusions, and offer your \
          read of what they mean. You do not need permission to do arithmetic or analysis on \
          numbers that appear in a transcript.\n\
        - Cite an excerpt as [1], [2] when you rely on something specific from it. Do not cite \
          for chit-chat, arithmetic, or your own reasoning. Never invent a citation number that \
          was not provided.\n\
        - If the excerpts do not cover the question, say so in your own words, say what you DID \
          find that is close, and suggest what the user could ask instead. Never answer with a \
          bare stock sentence.\n\
        - If the question is not about the recordings at all (a greeting, a question about this \
          conversation, a general request), just answer it normally. Do not mention the search.\n\
        - Do not fabricate quotes, speakers, decisions, numbers, or dates. If you are unsure \
          whether something was said, say that plainly.";

    let mut messages = vec![ChatMessage::system(system)];

    // Replay the thread so follow-ups and pronouns resolve.
    let start = history.len().saturating_sub(MAX_HISTORY_TURNS);
    for turn in &history[start..] {
        let content = truncate_chars(turn.content.trim(), MAX_HISTORY_CHARS);
        if content.is_empty() {
            continue;
        }
        if turn.role.eq_ignore_ascii_case("assistant") {
            messages.push(ChatMessage::assistant(content));
        } else {
            messages.push(ChatMessage::user(content));
        }
    }

    let user = if hits.is_empty() {
        format!(
            "[Search over the recordings returned no matching excerpts for this message.]\n\n\
             {question}"
        )
    } else {
        let mut context = String::new();
        for (i, hit) in hits.iter().enumerate() {
            let n = i + 1;
            let title = hit.recording_title.as_deref().unwrap_or("Untitled recording");
            let ts = format_timestamp(hit.start_ms);
            context.push_str(&format!("[{n}] ({title}{ts})\n{}\n\n", hit.text.trim()));
        }
        format!("Excerpts from my recordings:\n\n{context}\n{question}")
    };
    messages.push(ChatMessage::user(user));
    messages
}

/// Truncate on a character boundary, adding an ellipsis when it cuts.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
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
