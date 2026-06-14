//! Minimal Ollama client.
//!
//! Ollama runs on the processing node (design §9), default `:11434`. We use its
//! **native** non-streaming chat endpoint, `POST {base}/api/chat` with
//! `"stream": false`, rather than the OpenAI-compatible `/v1/chat/completions`.
//! The native shape is stable, returns a single JSON object (no SSE framing to
//! unwrap), and lets us hit `GET {base}/api/tags` for health with the same base
//! URL. Switching to the OpenAI surface later is a drop-in change.
//!
//! This client is plain HTTP (reqwest + rustls) and is therefore available
//! regardless of the `local-embed` Cargo feature.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use scribe_core::error::{Error, Result};

/// Default timeout for a chat completion. Local models on a busy single-GPU box
/// can take a while on long prompts, so we allow a generous window.
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
/// Health probes should be quick.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// One message in a chat conversation. `role` is `"system" | "user" |
/// "assistant"` per the OpenAI/Ollama convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// Optional generation parameters passed through to Ollama's `options` block.
/// Defaults leave everything to the server (i.e. nothing is sent), so callers
/// that just want the simple [`OllamaClient::chat`] never touch this.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Maps to Ollama's `num_predict` (max tokens to generate).
    #[serde(skip_serializing_if = "Option::is_none", rename = "num_predict")]
    pub max_tokens: Option<i32>,
    /// Maps to Ollama's `num_ctx` (context window size).
    #[serde(skip_serializing_if = "Option::is_none", rename = "num_ctx")]
    pub num_ctx: Option<u32>,
}

impl ChatOptions {
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.max_tokens.is_none()
            && self.num_ctx.is_none()
    }
}

/// A client for a single Ollama server.
#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    /// We always request a single, non-streamed response object.
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<&'a ChatOptions>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    #[allow(dead_code)]
    role: String,
    content: String,
}

impl OllamaClient {
    /// Create a client pointed at `base_url` (e.g. `http://127.0.0.1:11434`).
    /// A trailing slash is trimmed so path joins are predictable.
    pub fn new(base_url: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        // The per-request timeouts below bound each call; the shared client just
        // needs connection pooling. If the builder somehow fails we fall back to
        // a default client rather than panicking.
        let http = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { base_url, http }
    }

    /// Non-streaming chat completion. Returns the assistant message text.
    ///
    /// POSTs to `{base}/api/chat` with `"stream": false`.
    pub async fn chat(&self, model: &str, messages: &[ChatMessage]) -> Result<String> {
        self.chat_with(model, messages, &ChatOptions::default()).await
    }

    /// Chat completion with explicit generation parameters.
    pub async fn chat_with(
        &self,
        model: &str,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let body = ChatRequest {
            model,
            messages,
            stream: false,
            options: if options.is_empty() { None } else { Some(options) },
        };

        let resp = self
            .http
            .post(&url)
            .timeout(CHAT_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Http(format!("ollama chat request to {url} failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            // A 404 from /api/chat almost always means the model isn't pulled —
            // surface that as a model error rather than a generic HTTP error.
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(Error::Model(format!(
                    "ollama model `{model}` not available (404 from {url}): {text}"
                )));
            }
            return Err(Error::Http(format!(
                "ollama chat returned {status} from {url}: {text}"
            )));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Http(format!("decoding ollama chat response failed: {e}")))?;
        Ok(parsed.message.content)
    }

    /// Liveness check: `GET {base}/api/tags` succeeds with 2xx.
    pub async fn health(&self) -> Result<bool> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .http
            .get(&url)
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await
            .map_err(|e| Error::Http(format!("ollama health probe to {url} failed: {e}")))?;
        Ok(resp.status().is_success())
    }

    /// The base URL this client targets (trailing slash trimmed).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_trailing_slash() {
        let c = OllamaClient::new("http://127.0.0.1:11434/");
        assert_eq!(c.base_url(), "http://127.0.0.1:11434");
    }

    #[test]
    fn chat_request_serializes_with_stream_false() {
        let msgs = vec![ChatMessage::user("hi")];
        let req = ChatRequest {
            model: "gemma3:27b",
            messages: &msgs,
            stream: false,
            options: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["stream"], serde_json::json!(false));
        assert_eq!(v["model"], serde_json::json!("gemma3:27b"));
        assert_eq!(v["messages"][0]["role"], serde_json::json!("user"));
        assert_eq!(v["messages"][0]["content"], serde_json::json!("hi"));
        // Empty options must be omitted entirely.
        assert!(v.get("options").is_none());
    }

    #[test]
    fn chat_options_omit_none_fields() {
        let opts = ChatOptions {
            temperature: Some(0.2),
            ..Default::default()
        };
        let v = serde_json::to_value(&opts).unwrap();
        // f32 0.2 widens to a noisy f64 in JSON, so compare the value back as f32.
        assert_eq!(v["temperature"].as_f64().unwrap() as f32, 0.2f32);
        assert!(v.get("top_p").is_none());
        assert!(v.get("num_predict").is_none());
    }

    #[test]
    fn chat_response_deserializes() {
        let raw = serde_json::json!({
            "model": "gemma3:27b",
            "message": { "role": "assistant", "content": "hello there" },
            "done": true
        });
        let parsed: ChatResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.message.content, "hello there");
    }
}
