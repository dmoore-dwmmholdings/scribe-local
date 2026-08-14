//! Stage 6 — summarize (design §9).
//!
//! Build a speaker-labelled transcript string (using enrolled names where the
//! diarized speaker was matched, else `Speaker N`) and prompt Ollama for STRICT
//! JSON metadata: `{title, summary, action_items, decisions, topics}`. Parsing
//! is defensive — we extract the first balanced `{...}` block and tolerate a
//! model that wraps it in prose; on total failure we still store the raw text as
//! the `summary` so the recording is never left without a row.

use std::collections::HashMap;

use scribe_core::config::Config;
use scribe_core::summary_template;
use scribe_core::types::Utterance;
use scribe_core::Result;
use scribe_db::Db;
use scribe_llm::{ChatMessage, OllamaClient};
use serde_json::{json, Value};
use uuid::Uuid;

const SYSTEM_PROMPT: &str = "You are a meeting-notes assistant. Respond with only JSON.";

/// Run the summarize stage for `recording_id` using the given template.
///
/// `template` is a template id (e.g. `"general"`, `"interview"`); it is resolved
/// case-insensitively, falling back to `general` for an empty/unknown id. The
/// template only changes the prompt framing — the required output JSON shape and
/// the persisted columns are unchanged.
pub async fn run(
    cfg: &Config,
    db: &Db,
    ollama: &OllamaClient,
    recording_id: Uuid,
    template: &str,
) -> Result<()> {
    let utterances = db.list_utterances_by_recording(recording_id).await?;
    let names = speaker_names(db, recording_id).await?;
    let transcript = render_transcript(&utterances, &names);

    let tmpl = summary_template::resolve(template);
    let model = &cfg.llm.summarize_model;
    let user = format!(
        "{instructions}\n\n\
         Return ONLY a JSON object with keys: \
         \"title\" (string), \"summary\" (string), \"action_items\" (array of strings), \
         \"decisions\" (array of strings), \"topics\" (array of strings). \
         If the transcript is empty or trivial, still return the object with empty values.\n\n\
         Transcript:\n{transcript}",
        instructions = tmpl.instructions
    );

    let messages = [ChatMessage::system(SYSTEM_PROMPT), ChatMessage::user(user)];

    // Call Ollama. A successful call with unparseable content degrades into a
    // raw-text summary. A *transport* failure (Ollama down / not installed)
    // degrades into an empty summary row rather than failing the stage, so the
    // pipeline still completes end-to-end on a box without Ollama (mirrors the
    // stub speech/embed engines). Genuinely retryable infra errors would be
    // better surfaced, but for a single-user self-hosted box a missing summary
    // shouldn't block `ready` — the recording is still searchable and readable.
    let parsed = match ollama.chat(model, &messages).await {
        Ok(raw) => parse_summary(&raw),
        Err(e) => {
            tracing::warn!(
                %recording_id, model = %model, error = %e,
                "summarize: Ollama unavailable; storing empty summary"
            );
            ParsedSummary {
                title: None,
                summary: None,
                action_items: json!([]),
                decisions: json!([]),
                topics: json!([]),
            }
        }
    };

    db.upsert_summary(
        recording_id,
        parsed.title.as_deref(),
        parsed.summary.as_deref(),
        parsed.action_items,
        parsed.topics,
        parsed.decisions,
        Some(model),
        Some(tmpl.id),
    )
    .await?;

    // Finalize the recording's name from the generated title. Overwrites the
    // continuously-regenerated provisional title but never a user-provided one
    // (the guard lives in `set_recording_title_auto`).
    if let Some(title) = parsed.title.as_deref() {
        db.set_recording_title_auto(recording_id, title).await?;
    }

    tracing::info!(%recording_id, model = %model, template = %tmpl.id, "summarize complete");
    Ok(())
}

/// Resolved per-speaker display names keyed by diarized `local_idx`.
async fn speaker_names(db: &Db, recording_id: Uuid) -> Result<HashMap<i32, String>> {
    let rows = db.list_recording_speakers(recording_id).await?;
    Ok(rows
        .into_iter()
        .map(|rs| {
            let name = rs
                .display_name
                .unwrap_or_else(|| format!("Speaker {}", rs.local_idx));
            (rs.local_idx, name)
        })
        .collect())
}

/// Render `Speaker: text` lines for the LLM prompt.
fn render_transcript(utterances: &[Utterance], names: &HashMap<i32, String>) -> String {
    let mut s = String::new();
    for u in utterances {
        let label = match u.local_idx {
            Some(idx) => names
                .get(&idx)
                .cloned()
                .unwrap_or_else(|| format!("Speaker {idx}")),
            None => "Speaker".to_string(),
        };
        s.push_str(&label);
        s.push_str(": ");
        s.push_str(u.text.trim());
        s.push('\n');
    }
    s
}

/// The parsed (or degraded) summary fields.
struct ParsedSummary {
    title: Option<String>,
    summary: Option<String>,
    action_items: Value,
    decisions: Value,
    topics: Value,
}

/// Parse the model output defensively. Extract the first balanced `{...}` block
/// and read the expected keys; on any failure store the whole raw response as
/// the `summary` and leave the arrays empty.
fn parse_summary(raw: &str) -> ParsedSummary {
    if let Some(block) = first_json_object(raw) {
        if let Ok(v) = serde_json::from_str::<Value>(&block) {
            return ParsedSummary {
                title: string_field(&v, "title"),
                summary: string_field(&v, "summary"),
                action_items: string_array(&v, "action_items"),
                decisions: string_array(&v, "decisions"),
                topics: string_array(&v, "topics"),
            };
        }
    }
    // Could not parse JSON — preserve the raw text so nothing is lost.
    ParsedSummary {
        title: None,
        summary: Some(raw.trim().to_string()),
        action_items: json!([]),
        decisions: json!([]),
        topics: json!([]),
    }
}

/// Read a string field, treating empty/null as absent.
fn string_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Normalize a field into a JSON array of strings. Accepts an array (kept as-is
/// if all strings, else stringified) or a single string (wrapped). Anything
/// else becomes `[]`.
fn string_array(v: &Value, key: &str) -> Value {
    match v.get(key) {
        Some(Value::Array(items)) => {
            let out: Vec<Value> = items
                .iter()
                .filter_map(|i| match i {
                    Value::String(s) if !s.trim().is_empty() => Some(json!(s.trim())),
                    Value::String(_) => None,
                    other => Some(json!(other.to_string())),
                })
                .collect();
            Value::Array(out)
        }
        Some(Value::String(s)) if !s.trim().is_empty() => json!([s.trim()]),
        _ => json!([]),
    }
}

/// Return the first balanced `{...}` substring of `s`, respecting braces inside
/// JSON strings (and their escapes). `None` if there is no balanced object.
fn first_json_object(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_clean_json() {
        let raw = r#"{"title":"Standup","summary":"We synced.","action_items":["ship it"],"decisions":[],"topics":["status"]}"#;
        let p = parse_summary(raw);
        assert_eq!(p.title.as_deref(), Some("Standup"));
        assert_eq!(p.summary.as_deref(), Some("We synced."));
        assert_eq!(p.action_items, json!(["ship it"]));
        assert_eq!(p.topics, json!(["status"]));
        assert_eq!(p.decisions, json!([]));
    }

    #[test]
    fn extracts_json_wrapped_in_prose() {
        let raw = "Sure! Here is the JSON:\n{\"title\": \"X\", \"summary\": \"Y\"}\nHope that helps.";
        let p = parse_summary(raw);
        assert_eq!(p.title.as_deref(), Some("X"));
        assert_eq!(p.summary.as_deref(), Some("Y"));
        assert_eq!(p.action_items, json!([]));
    }

    #[test]
    fn braces_inside_strings_dont_break_extraction() {
        let raw = r#"{"summary":"use the {placeholder} here","topics":["a"]}"#;
        let p = parse_summary(raw);
        assert_eq!(p.summary.as_deref(), Some("use the {placeholder} here"));
        assert_eq!(p.topics, json!(["a"]));
    }

    #[test]
    fn unparseable_falls_back_to_raw_summary() {
        let raw = "I could not produce JSON, sorry.";
        let p = parse_summary(raw);
        assert!(p.title.is_none());
        assert_eq!(p.summary.as_deref(), Some("I could not produce JSON, sorry."));
        assert_eq!(p.action_items, json!([]));
        assert_eq!(p.decisions, json!([]));
        assert_eq!(p.topics, json!([]));
    }

    #[test]
    fn string_field_for_action_items_is_wrapped() {
        let raw = r#"{"action_items":"follow up with finance"}"#;
        let p = parse_summary(raw);
        assert_eq!(p.action_items, json!(["follow up with finance"]));
    }
}
