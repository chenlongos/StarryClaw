//! OpenAI-compatible Chat Completions with `tools` (function calling).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    http: reqwest::Client,
    base: String,
    model: String,
}

impl Client {
    pub fn new(base: String, model: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("build HTTP client")?;
        Ok(Self { http, base, model })
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
        let url = format!(
            "{}/chat/completions",
            self.base.trim_end_matches('/')
        );
        let body = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
        };

        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(key) = api_key.filter(|k| !k.is_empty()) {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let res = req.send().await.context("chat request")?;

        let status = res.status();
        let text = res.text().await.context("read body")?;
        if !status.is_success() {
            anyhow::bail!("HTTP {} — {}", status, text.trim());
        }

        let parsed: ChatResponse =
            serde_json::from_str(&text).with_context(|| format!("parse JSON: {}", text.chars().take(200).collect::<String>()))?;

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
        Ok((msg, text_out))
    }
}

pub fn openai_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files in a directory (runs ls -la). Use for fuzzy requests: 看看、有啥、列一下、当前目录、show what's here、list files、browse、打开看看、里有什么.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path; use . for current directory"
                        }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "mkdir",
                "description": "Create one directory (mkdir -p), name only, no path. Fuzzy: 建个xx、新建文件夹、帮我创建、make a folder、mkdir foo、弄个目录叫.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Single directory name in the current working directory (no slashes or ..)"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "change_dir",
                "description": "Change current working directory (cd). Use for: 进入xx目录、切换到、cd foo、chdir、去某个文件夹.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Target directory path (relative or absolute, .. allowed)"
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read text file contents (UTF-8 / lossy). Use for: 查看文件、读一下、cat、show content、打开某某文件.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path"
                        },
                        "max_bytes": {
                            "type": "integer",
                            "description": "Optional max bytes to read (default 262144, cap 2097152)"
                        }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "run_shell",
                "description": "Run a single allowlisted program with arguments (no shell, no pipes). For 今天几号/日期/时间 call date (e.g. date +%Y-%m-%d). For kernel/OS: uname -a. Also: pwd, whoami, hostname, uptime, cal, df, env, which, wc, head, tail, id, stat, file, readlink, etc. If not in this tool's allowlist (e.g. free, grep, curl), tell the user to run it in their terminal. Prefer run_shell over only suggesting markdown when the command is allowlisted.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "One program name plus args, whitespace-separated (e.g. \"date +%Y-%m-%d\", \"uname -a\"). No ; | & $ ` or newlines."
                        }
                    },
                    "required": ["command"]
                }
            }
        }),
    ]
}
