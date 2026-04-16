//! OpenAI-compatible Chat Completions with `tools` (function calling).
//! HTTP：`ureq`（blocking + 连接池），在 `tokio::task::spawn_blocking` 里跑，避免在 StarryOS / musl 上踩 `reqwest`/`rustls`（ring）的坑。
//! 当前依赖未启用 ureq 的 TLS；`STARRYCLAW_BASE_URL` 请用 `http://…`（典型 Ollama 局域网）。HTTPS 需在 Cargo 中为 ureq 打开 `native-tls`（或 `tls`）特性。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
    pub error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
pub struct ApiError {
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: AssistantMessage,
}

#[derive(Debug, Deserialize)]
pub struct AssistantMessage {
    #[allow(dead_code)]
    pub role: String,
    pub content: Option<Value>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

fn content_as_string(c: &Option<Value>) -> Option<String> {
    let v = c.as_ref()?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|x| x.as_str()) {
                    out.push_str(t);
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => Some(v.to_string()),
    }
}

pub struct Client {
    agent: ureq::Agent,
    base: String,
    model: String,
}

impl Client {
    pub fn new(base: String, model: String) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(120))
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(120))
            .timeout_write(Duration::from_secs(120))
            .max_idle_connections_per_host(20)
            .no_delay(true)
            .build();
        Ok(Self {
            agent,
            base,
            model,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// `api_key`: `None` or empty → no `Authorization` header (e.g. local Ollama).
    pub async fn chat(
        &self,
        api_key: Option<&str>,
        messages: Vec<ChatMessage>,
        tools: &[Value],
    ) -> Result<(AssistantMessage, Option<String>)> {
        let url = format!("{}/chat/completions", self.base.trim_end_matches('/'));
        let body = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
        };

        let agent = self.agent.clone();
        let bearer = api_key
            .filter(|k| !k.is_empty())
            .map(|k| k.to_string());

        let (msg, text_out) = tokio::task::spawn_blocking(move || {
            let mut req = agent.post(&url);
            if let Some(ref key) = bearer {
                req = req.set("Authorization", &format!("Bearer {key}"));
            }
            let resp = req
                .send_json(&body)
                .map_err(|e| anyhow::anyhow!("chat request: {e}"))?;
            let status = resp.status();
            let body_str = resp
                .into_string()
                .map_err(|e| anyhow::anyhow!("read body: {e}"))?;
            if !(200..300).contains(&status) {
                anyhow::bail!("HTTP {status} — {}", body_str.trim());
            }
            let parsed: ChatResponse = serde_json::from_str(&body_str).with_context(|| {
                format!(
                    "parse JSON: {}",
                    body_str.chars().take(200).collect::<String>()
                )
            })?;
            if let Some(e) = parsed.error {
                anyhow::bail!("API error: {}", e.message);
            }
            let choice = parsed
                .choices
                .into_iter()
                .next()
                .context("empty choices")?;
            let msg = choice.message;
            let text_out = content_as_string(&msg.content);
            Ok::<_, anyhow::Error>((msg, text_out))
        })
        .await
        .context("join HTTP worker")??;

        Ok((msg, text_out))
    }
}