//! Chat client for a local LLM server (summaries + RAG answers, design §9).
//!
//! Two providers are supported, selected by [`LlmProvider`]:
//!
//! * **Ollama** (default) — native non-streaming `POST {base}/api/chat` with
//!   `"stream": false`; health via `GET {base}/api/tags`.
//! * **OpenAI-compatible** — `POST {base}/v1/chat/completions`; health via
//!   `GET {base}/v1/models`. This covers **LM Studio**, llama.cpp server, vLLM,
//!   and Ollama's own `/v1` surface.
//!
//! Plain HTTP (reqwest + rustls), so it is available regardless of the
//! `local-embed` Cargo feature. The public type is [`ChatClient`]; the old name
//! `OllamaClient` remains as an alias.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use scribe_core::config::{LlmConfig, LlmProvider};
use scribe_core::error::{Error, Result};

/// Default timeout for a chat completion. Local models on a busy single-GPU box
/// can take a while on long prompts, so we allow a generous window.
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
/// Health probes should be quick.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// One message in a chat conversation. `role` is `"system" | "user" |
/// "assistant"` per the OpenAI/Ollama convention. Serializes identically for
/// both providers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".to_string(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".to_string(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".to_string(), content: content.into() }
    }
}

/// Optional generation parameters. Fields left `None` are not sent, so callers
/// that just want [`ChatClient::chat`] never touch this. Mapped per provider
/// (`max_tokens` → Ollama's `num_predict`; `num_ctx` is Ollama-only).
#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<i32>,
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

/// A client for one local LLM server (Ollama or OpenAI-compatible).
#[derive(Debug, Clone)]
pub struct ChatClient {
    base_url: String,
    provider: LlmProvider,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl ChatClient {
    /// Build from the `[llm]` config (provider + base URL + optional key).
    pub fn from_config(cfg: &LlmConfig) -> Self {
        Self::new(cfg.provider, &cfg.base_url, cfg.api_key.clone())
    }

    /// Build a client explicitly. The base URL is normalized: trailing slash
    /// trimmed, and for the OpenAI provider a missing `/v1` suffix is added (so
    /// both `http://host:1234` and `http://host:1234/v1` work).
    pub fn new(provider: LlmProvider, base_url: &str, api_key: Option<String>) -> Self {
        let mut base = base_url.trim_end_matches('/').to_string();
        if provider == LlmProvider::Openai && !base.ends_with("/v1") {
            base.push_str("/v1");
        }
        let http = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { base_url: base, provider, api_key, http }
    }

    /// Non-streaming chat completion. Returns the assistant message text.
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
        match self.provider {
            LlmProvider::Ollama => self.chat_ollama(model, messages, options).await,
            LlmProvider::Openai => self.chat_openai(model, messages, options).await,
        }
    }

    async fn chat_ollama(
        &self,
        model: &str,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<String> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            options: Option<serde_json::Value>,
        }
        #[derive(Deserialize)]
        struct Resp {
            message: Msg,
        }
        #[derive(Deserialize)]
        struct Msg {
            content: String,
        }

        let opts = if options.is_empty() {
            None
        } else {
            let mut m = serde_json::Map::new();
            if let Some(t) = options.temperature { m.insert("temperature".into(), t.into()); }
            if let Some(p) = options.top_p { m.insert("top_p".into(), p.into()); }
            if let Some(n) = options.max_tokens { m.insert("num_predict".into(), n.into()); }
            if let Some(c) = options.num_ctx { m.insert("num_ctx".into(), c.into()); }
            Some(serde_json::Value::Object(m))
        };

        let url = format!("{}/api/chat", self.base_url);
        let body = Req { model, messages, stream: false, options: opts };
        let text = self.post_chat(&url, &body, model).await?;
        let parsed: Resp = serde_json::from_str(&text)
            .map_err(|e| Error::Http(format!("decoding ollama chat response failed: {e}")))?;
        Ok(parsed.message.content)
    }

    async fn chat_openai(
        &self,
        model: &str,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<String> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            temperature: Option<f32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            top_p: Option<f32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            max_tokens: Option<i32>,
        }
        #[derive(Deserialize)]
        struct Resp {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: Msg,
        }
        #[derive(Deserialize)]
        struct Msg {
            content: String,
        }

        let url = format!("{}/chat/completions", self.base_url);
        let body = Req {
            model,
            messages,
            stream: false,
            temperature: options.temperature,
            top_p: options.top_p,
            max_tokens: options.max_tokens,
        };
        let text = self.post_chat(&url, &body, model).await?;
        let parsed: Resp = serde_json::from_str(&text).map_err(|e| {
            Error::Http(format!("decoding openai chat response failed: {e}"))
        })?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| Error::Model(format!("{} returned no choices", url)))
    }

    /// Shared POST + status handling for both providers. Returns the raw body.
    async fn post_chat<B: Serialize>(&self, url: &str, body: &B, model: &str) -> Result<String> {
        let mut req = self.http.post(url).timeout(CHAT_TIMEOUT).json(body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Http(format!("chat request to {url} failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            // A 404 (or LM Studio's "model not found") usually means the model
            // isn't loaded/pulled — surface as a model error, not a generic HTTP one.
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(Error::Model(format!(
                    "model `{model}` not available (404 from {url}): {text}"
                )));
            }
            return Err(Error::Http(format!("chat returned {status} from {url}: {text}")));
        }
        resp.text()
            .await
            .map_err(|e| Error::Http(format!("reading chat response from {url} failed: {e}")))
    }

    /// Liveness check against the provider's model-list endpoint.
    pub async fn health(&self) -> Result<bool> {
        let url = match self.provider {
            LlmProvider::Ollama => format!("{}/api/tags", self.base_url),
            LlmProvider::Openai => format!("{}/models", self.base_url),
        };
        let mut req = self.http.get(&url).timeout(HEALTH_TIMEOUT);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Http(format!("llm health probe to {url} failed: {e}")))?;
        Ok(resp.status().is_success())
    }

    /// The base URL this client targets (normalized).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Which provider this client speaks to.
    pub fn provider(&self) -> LlmProvider {
        self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_base_is_trimmed_not_suffixed() {
        let c = ChatClient::new(LlmProvider::Ollama, "http://127.0.0.1:11434/", None);
        assert_eq!(c.base_url(), "http://127.0.0.1:11434");
    }

    #[test]
    fn openai_base_gets_v1_when_missing() {
        // LM Studio default, given without /v1
        let c = ChatClient::new(LlmProvider::Openai, "http://127.0.0.1:1234", None);
        assert_eq!(c.base_url(), "http://127.0.0.1:1234/v1");
    }

    #[test]
    fn openai_base_keeps_existing_v1() {
        let c = ChatClient::new(LlmProvider::Openai, "http://127.0.0.1:1234/v1/", None);
        assert_eq!(c.base_url(), "http://127.0.0.1:1234/v1");
    }

    #[test]
    fn from_config_uses_provider() {
        let mut cfg = LlmConfig::default();
        cfg.provider = LlmProvider::Openai;
        cfg.base_url = "http://localhost:1234".into();
        let c = ChatClient::from_config(&cfg);
        assert_eq!(c.provider(), LlmProvider::Openai);
        assert_eq!(c.base_url(), "http://localhost:1234/v1");
    }
}
