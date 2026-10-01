//! MCP endpoint — lets an MCP client such as Claude Code read and work on the
//! meetings this server holds.
//!
//! ```text
//! claude mcp add --transport http scribe https://<server>.ts.net/mcp
//! ```
//!
//! This is the Streamable HTTP transport, in its simplest conforming shape:
//! every JSON-RPC request arrives as a `POST` and is answered with a single
//! `application/json` body. There is no server-initiated stream, so `GET`
//! returns 405, and no session state, so no `Mcp-Session-Id` is issued. The
//! tools need neither: each call is one request and one answer.
//!
//! Hand-written rather than built on an SDK because the surface is four
//! methods (`initialize`, `ping`, `tools/list`, `tools/call`), and because each
//! tool calls the HTTP handler for the same operation. Validation, clamping and
//! error mapping are therefore the API's own, not a second copy that drifts.
//!
//! Auth is the API's: the route sits behind [`crate::auth::require_auth`], so a
//! client on the owner's tailnet login needs nothing, and anything else sends
//! `Authorization: Bearer <device key>` (`claude mcp add ... --header`).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::handlers::{recordings, search, speakers};
use crate::state::AppState;

/// Protocol versions this endpoint speaks, newest first. Only the tools surface
/// is used, and it has not changed shape across these.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

const INSTRUCTIONS: &str = "Scribe holds recorded meetings: diarized transcripts \
with speaker names, summaries, and a search index. Find a meeting with \
list_recordings or search, then read it with get_transcript or get_summary. \
If a transcript has the wrong number of speakers, set_participants with the \
right count re-runs speaker detection without re-transcribing; poll \
get_transcript until its status is ready again.";

// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// `GET` / `DELETE /mcp`: no server-initiated stream and no sessions to end.
pub async fn not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

/// `POST /mcp`: one JSON-RPC message, or a batch of them.
pub async fn post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // A browser page can POST here too. Auth already stops a page from
    // presenting a credential, but the transport spec asks for the check, and
    // MCP clients do not send Origin at all.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !origin_allowed(origin, &state.cfg.api.public_base_url) {
            tracing::warn!(%origin, "refusing MCP request from a foreign origin");
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return Json(error_response(Value::Null, PARSE_ERROR, &e.to_string())).into_response()
        }
    };

    let replies: Vec<Value> = match msg {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for m in batch {
                out.extend(handle_message(&state, m).await);
            }
            out
        }
        single => handle_message(&state, single).await.into_iter().collect(),
    };

    match replies.len() {
        // Only notifications or responses: nothing to answer.
        0 => StatusCode::ACCEPTED.into_response(),
        1 => Json(replies.into_iter().next().unwrap_or(Value::Null)).into_response(),
        _ => Json(Value::Array(replies)).into_response(),
    }
}

fn origin_allowed(origin: &str, public_base_url: &str) -> bool {
    let base = public_base_url.trim().trim_end_matches('/');
    !base.is_empty() && origin.trim_end_matches('/').eq_ignore_ascii_case(base)
}

/// Answer one message. `None` for a notification or a client response.
async fn handle_message(state: &AppState, msg: Value) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A response to a server request — this server makes none — or junk.
        return msg
            .get("id")
            .filter(|_| msg.get("result").is_none() && msg.get("error").is_none())
            .map(|id| error_response(id.clone(), INVALID_REQUEST, "missing method"));
    };
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(state, &params).await,
        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    // Echo the client's version when it is one of ours; otherwise offer the
    // newest, and the client decides whether it can speak it.
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "scribe", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

// --------------------------------------------------------------------------
// Tools
// --------------------------------------------------------------------------

const RECORDING_ID: &str = "Recording id (a UUID), from list_recordings or search.";
const RFC3339: &str = "RFC 3339 timestamp, e.g. 2026-09-01T00:00:00Z";

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_recordings",
            "description": "List recorded meetings, newest first: id, date, title, length, processing status, stated participant count and tags.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "How many to return (default 20)." },
                    "offset": { "type": "integer", "minimum": 0, "description": "Skip this many, for paging." },
                    "tag": { "type": "string", "description": "Only recordings carrying this tag." }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_transcript",
            "description": "Read one meeting: its details, speakers, and the diarized transcript as timestamped lines with speaker names. While the meeting is still processing, shows how far each pipeline stage has got instead.",
            "inputSchema": {
                "type": "object",
                "properties": { "recording_id": { "type": "string", "description": RECORDING_ID } },
                "required": ["recording_id"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_summary",
            "description": "Read a meeting's generated summaries: title, summary, decisions, action items and topics. A meeting has one summary per template it was summarised with.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recording_id": { "type": "string", "description": RECORDING_ID },
                    "template": { "type": "string", "description": "Only this summary template (e.g. general). Default: all." }
                },
                "required": ["recording_id"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "search",
            "description": "Search every transcript by meaning and keyword. Returns matching passages with the meeting, timestamp and speaker.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Default 10." },
                    "from": { "type": "string", "description": format!("Only meetings on or after this. {RFC3339}") },
                    "to": { "type": "string", "description": format!("Only meetings before this. {RFC3339}") },
                    "recording_id": { "type": "string", "description": "Only this meeting." },
                    "speaker_id": { "type": "string", "description": "Only this enrolled speaker (id from list_speakers)." }
                },
                "required": ["query"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "ask",
            "description": "Answer a question from the meetings using the server's own language model, with citations. Use search and get_transcript instead when you want to read the material yourself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "recording_id": { "type": "string", "description": "Only this meeting." },
                    "from": { "type": "string", "description": RFC3339 },
                    "to": { "type": "string", "description": RFC3339 }
                },
                "required": ["question"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_speakers",
            "description": "The enrolled speaker library: each known voice's id, name, and how many meetings it appears in.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "set_participants",
            "description": "State how many people speak in a meeting, then (by default) re-run speaker detection with that count. Only speaker detection and the stages after it re-run; the words are kept. The meeting is processing until it finishes — poll get_transcript.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recording_id": { "type": "string", "description": RECORDING_ID },
                    "participants": { "type": ["integer", "null"], "minimum": 1, "maximum": 64, "description": "Number of people. null clears it, so speaker detection discovers the count again." },
                    "rediarize": { "type": "boolean", "description": "Re-run speaker detection now (default true)." }
                },
                "required": ["recording_id", "participants"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        },
        {
            "name": "rediarize",
            "description": "Re-run speaker detection on a meeting with its current settings, keeping the transcript's words. The meeting is processing until it finishes — poll get_transcript.",
            "inputSchema": {
                "type": "object",
                "properties": { "recording_id": { "type": "string", "description": RECORDING_ID } },
                "required": ["recording_id"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        }
    ])
}

/// What a tool hands back: text for the model, and whether it is an error the
/// model should see and act on (as opposed to a protocol failure).
type ToolOutput = Result<String, String>;

async fn call_tool(state: &AppState, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let output = match name {
        "list_recordings" => list_recordings_tool(state, args).await,
        "get_transcript" => get_transcript_tool(state, args).await,
        "get_summary" => get_summary_tool(state, args).await,
        "search" => search_tool(state, args).await,
        "ask" => ask_tool(state, args).await,
        "list_speakers" => list_speakers_tool(state).await,
        "set_participants" => set_participants_tool(state, args).await,
        "rediarize" => rediarize_tool(state, args).await,
        other => return Err((INVALID_PARAMS, format!("unknown tool: {other}"))),
    };

    let (text, is_error) = match output {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn parse_time(field: &str, value: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    value
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            DateTime::parse_from_rfc3339(s.trim())
                .map(|t| t.with_timezone(&Utc))
                .map_err(|e| format!("`{field}` is not an RFC 3339 timestamp: {e}"))
        })
        .transpose()
}

/// The API's own error text: "recording ... not found", "must be between 1
/// and 64", and so on.
fn api_err(e: crate::error::ApiError) -> String {
    e.0.to_string()
}

#[derive(Deserialize)]
struct ListArgs {
    limit: Option<i64>,
    offset: Option<i64>,
    tag: Option<String>,
}

async fn list_recordings_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: ListArgs = parse_args(args)?;
    let limit = a.limit.unwrap_or(20);
    let offset = a.offset.unwrap_or(0);
    let Json(list) = recordings::list_recordings(
        State(state.clone()),
        Query(recordings::ListQuery {
            limit: Some(limit),
            offset: Some(offset),
            tag: a.tag,
        }),
    )
    .await
    .map_err(api_err)?;

    if list.recordings.is_empty() {
        return Ok(if offset > 0 {
            "No more recordings."
        } else {
            "No recordings."
        }
        .to_string());
    }
    let mut out = String::new();
    for r in &list.recordings {
        out.push_str(&format!(
            "- {} | {} | {} | {} | {}",
            r.id,
            r.created_at.format("%Y-%m-%d %H:%M UTC"),
            r.title.as_deref().unwrap_or("(untitled)"),
            r.duration_ms.map(fmt_ms).unwrap_or_else(|| "?".into()),
            r.status.as_str(),
        ));
        if let Some(n) = r.participants_expected {
            out.push_str(&format!(" | {n} participants stated"));
        }
        if !r.tags.is_empty() {
            out.push_str(&format!(" | tags: {}", r.tags.join(", ")));
        }
        out.push('\n');
    }
    if list.recordings.len() as i64 >= limit {
        out.push_str(&format!("(more may follow: offset {})\n", offset + limit));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct RecordingArgs {
    recording_id: Uuid,
}

async fn get_transcript_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: RecordingArgs = parse_args(args)?;
    let Json(d) = recordings::get_recording(State(state.clone()), Path(a.recording_id))
        .await
        .map_err(api_err)?;
    let r = &d.recording;

    let mut out = format!(
        "# {}\n\nid: {}\nrecorded: {}\nlength: {}\nstatus: {}\n",
        r.title.as_deref().unwrap_or("(untitled)"),
        r.id,
        r.created_at.format("%Y-%m-%d %H:%M UTC"),
        r.duration_ms.map(fmt_ms).unwrap_or_else(|| "?".into()),
        r.status.as_str(),
    );
    match r.participants_expected {
        Some(n) => out.push_str(&format!("participants stated: {n}\n")),
        None => out.push_str("participants stated: none (speaker detection chose the count)\n"),
    }
    if !r.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", r.tags.join(", ")));
    }

    if let Some(p) = &d.progress {
        out.push_str(&format!(
            "\n## Processing: {}/{} stages done\n",
            p.completed, p.total
        ));
        for s in &p.stages {
            let kind = serde_json::to_value(s.kind).ok();
            let kind = kind.as_ref().and_then(Value::as_str).unwrap_or("?");
            out.push_str(&format!("- {kind}: {}", s.state));
            if let Some(e) = &s.error {
                out.push_str(&format!(" — {e}"));
            }
            out.push('\n');
        }
    }

    out.push_str(&format!("\n## Speakers ({})\n", d.speakers.len()));
    for s in &d.speakers {
        let name = s
            .display_name
            .clone()
            .unwrap_or_else(|| format!("Speaker {}", s.local_idx));
        let words: usize = d
            .utterances
            .iter()
            .filter(|u| u.local_idx == Some(s.local_idx))
            .map(|u| u.text.split_whitespace().count())
            .sum();
        out.push_str(&format!(
            "- {name} (local index {}{}) — {words} words\n",
            s.local_idx,
            if s.speaker_id.is_some() {
                ", enrolled"
            } else {
                ""
            },
        ));
    }

    out.push_str("\n## Transcript\n");
    if d.utterances.is_empty() {
        out.push_str("(no transcript yet)\n");
    }
    for u in &d.utterances {
        let who = u
            .speaker_name
            .clone()
            .or_else(|| u.local_idx.map(|i| format!("Speaker {i}")))
            .unwrap_or_else(|| "Unknown".into());
        out.push_str(&format!(
            "[{}] {who}: {}\n",
            fmt_ms(u.start_ms),
            u.text.trim()
        ));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct SummaryArgs {
    recording_id: Uuid,
    template: Option<String>,
}

async fn get_summary_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: SummaryArgs = parse_args(args)?;
    let Json(d) = recordings::get_recording(State(state.clone()), Path(a.recording_id))
        .await
        .map_err(api_err)?;
    let want = a
        .template
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let summaries: Vec<_> = d
        .summaries
        .iter()
        .filter(|s| want.is_none_or(|w| s.template.as_deref() == Some(w)))
        .collect();
    if summaries.is_empty() {
        return Ok(match want {
            Some(w) => format!("No `{w}` summary for this meeting."),
            None => format!("No summary yet (status: {}).", d.recording.status.as_str()),
        });
    }

    let mut out = String::new();
    for s in summaries {
        out.push_str(&format!(
            "# {} ({} template)\n\n",
            s.title.as_deref().unwrap_or("(untitled)"),
            s.template.as_deref().unwrap_or("general"),
        ));
        if let Some(text) = &s.summary {
            out.push_str(text.trim());
            out.push_str("\n\n");
        }
        for (heading, items) in [
            ("Decisions", &s.decisions),
            ("Action items", &s.action_items),
            ("Topics", &s.topics),
        ] {
            let lines = json_list(items);
            if !lines.is_empty() {
                out.push_str(&format!("## {heading}\n"));
                for line in lines {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// Render a summary's JSON list (of strings, or of objects) one item per line.
fn json_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => s.clone(),
                Value::Object(o) => o
                    .iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| match v {
                        Value::String(s) => format!("{k}: {s}"),
                        other => format!("{k}: {other}"),
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
                other => other.to_string(),
            })
            .collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    limit: Option<i64>,
    from: Option<String>,
    to: Option<String>,
    recording_id: Option<Uuid>,
    speaker_id: Option<Uuid>,
}

async fn search_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: SearchArgs = parse_args(args)?;
    let Json(res) = search::search(
        State(state.clone()),
        Query(search::SearchQuery {
            q: a.query,
            from: parse_time("from", a.from)?,
            to: parse_time("to", a.to)?,
            speaker: a.speaker_id,
            recording: a.recording_id,
            limit: Some(a.limit.unwrap_or(10)),
        }),
    )
    .await
    .map_err(api_err)?;

    if res.hits.is_empty() {
        return Ok("No matches.".to_string());
    }
    let mut out = String::new();
    for h in &res.hits {
        out.push_str(&format!(
            "- {} ({}) [{}] {}: {}\n",
            h.recording_title.as_deref().unwrap_or("(untitled)"),
            h.recording_id,
            h.start_ms.map(fmt_ms).unwrap_or_else(|| "?".into()),
            h.speaker.as_deref().unwrap_or("Unknown"),
            h.text.trim(),
        ));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct AskArgs {
    question: String,
    recording_id: Option<Uuid>,
    from: Option<String>,
    to: Option<String>,
}

async fn ask_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: AskArgs = parse_args(args)?;
    let Json(answer) = search::ask(
        State(state.clone()),
        Json(search::AskBody {
            question: a.question,
            history: Vec::new(),
            filters: Some(search::AskFilters {
                from: parse_time("from", a.from)?,
                to: parse_time("to", a.to)?,
                speaker: None,
                recording: a.recording_id,
            }),
            top_k: None,
        }),
    )
    .await
    .map_err(api_err)?;

    let mut out = answer.answer.trim().to_string();
    if !answer.citations.is_empty() {
        out.push_str("\n\nSources:\n");
        for c in &answer.citations {
            out.push_str(&format!(
                "- {} ({}) [{}] {}: {}\n",
                c.recording_title.as_deref().unwrap_or("(untitled)"),
                c.recording_id,
                c.start_ms.map(fmt_ms).unwrap_or_else(|| "?".into()),
                c.speaker.as_deref().unwrap_or("Unknown"),
                c.snippet.trim(),
            ));
        }
    }
    Ok(out)
}

async fn list_speakers_tool(state: &AppState) -> ToolOutput {
    let Json(v) = speakers::list_speakers(State(state.clone()))
        .await
        .map_err(api_err)?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct ParticipantsArgs {
    recording_id: Uuid,
    participants: Option<i32>,
    rediarize: Option<bool>,
}

async fn set_participants_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: ParticipantsArgs = parse_args(args)?;
    // The handlers' JSON bodies are for HTTP clients; the tools say it in words.
    let _ = recordings::set_participants(
        State(state.clone()),
        Path(a.recording_id),
        Json(recordings::SetParticipantsBody {
            participants_expected: a.participants,
        }),
    )
    .await
    .map_err(api_err)?;

    let stated = match a.participants {
        Some(n) => format!("Participant count set to {n}."),
        None => "Participant count cleared.".to_string(),
    };
    if !a.rediarize.unwrap_or(true) {
        return Ok(format!("{stated} Speaker detection was not re-run."));
    }
    let _ = recordings::rediarize_recording(State(state.clone()), Path(a.recording_id))
        .await
        .map_err(|e| {
            format!(
                "{stated} But re-running speaker detection failed: {}",
                api_err(e)
            )
        })?;
    Ok(format!(
        "{stated} Speaker detection is re-running; poll get_transcript until status is ready."
    ))
}

async fn rediarize_tool(state: &AppState, args: Value) -> ToolOutput {
    let a: RecordingArgs = parse_args(args)?;
    let _ = recordings::rediarize_recording(State(state.clone()), Path(a.recording_id))
        .await
        .map_err(api_err)?;
    Ok("Speaker detection is re-running; poll get_transcript until status is ready.".to_string())
}

/// `83_000` → `1:23`; an hour or more gets an hour field.
fn fmt_ms(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(fmt_ms(0), "0:00");
        assert_eq!(fmt_ms(83_000), "1:23");
        assert_eq!(fmt_ms(3_723_000), "1:02:03");
        assert_eq!(fmt_ms(-5), "0:00");
    }

    #[test]
    fn echoes_a_supported_protocol_version() {
        let r = initialize_result(&json!({ "protocolVersion": "2025-06-18" }));
        assert_eq!(r["protocolVersion"], "2025-06-18");
        assert_eq!(r["serverInfo"]["name"], "scribe");
        assert!(r["capabilities"]["tools"].is_object());
    }

    #[test]
    fn offers_the_newest_for_an_unknown_version() {
        let r = initialize_result(&json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn every_tool_has_a_schema_and_a_unique_name() {
        let tools = tool_definitions();
        let tools = tools.as_array().unwrap();
        let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
    }

    #[test]
    fn origin_must_match_the_public_url() {
        let base = "https://scribe.tail1234.ts.net";
        assert!(origin_allowed("https://scribe.tail1234.ts.net", base));
        assert!(origin_allowed(
            "https://scribe.tail1234.ts.net/",
            &format!("{base}/")
        ));
        assert!(!origin_allowed("https://evil.example", base));
        assert!(!origin_allowed("https://scribe.tail1234.ts.net", ""));
    }

    #[test]
    fn rejects_bad_timestamps_and_skips_blank_ones() {
        assert!(parse_time("from", Some("yesterday".into())).is_err());
        assert_eq!(parse_time("from", Some("  ".into())).unwrap(), None);
        assert!(parse_time("from", Some("2026-09-01T00:00:00Z".into()))
            .unwrap()
            .is_some());
    }

    #[test]
    fn renders_summary_lists_of_strings_and_objects() {
        assert_eq!(json_list(&json!(["a", "b"])), vec!["a", "b"]);
        assert_eq!(
            json_list(&json!([{ "owner": "Karen", "task": "send the note", "due": null }])),
            vec!["owner: Karen; task: send the note"]
        );
        assert!(json_list(&Value::Null).is_empty());
    }
}
