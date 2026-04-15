//! OpenAI-compatible Chat Completions with `tools` (function calling).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

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

fn maybe_decode_chunked_body(input: &str) -> Option<String> {
    fn take_line<'a>(s: &'a str, pos: &mut usize) -> Option<&'a str> {
        if *pos >= s.len() {
            return None;
        }
        let rest = &s[*pos..];
        if let Some(i) = rest.find('\n') {
            let mut line = &rest[..i];
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped;
            }
            *pos += i + 1;
            Some(line)
        } else {
            let mut line = rest;
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped;
            }
            *pos = s.len();
            Some(line)
        }
    }

    let mut pos = 0usize;
    let first = take_line(input, &mut pos)?;
    let first = first.trim();
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let mut out = String::new();
    let mut line = first.to_string();
    loop {
        let size = usize::from_str_radix(line.trim(), 16).ok()?;
        if size == 0 {
            return Some(out);
        }
        let rest = &input[pos..];
        if rest.len() < size {
            return None;
        }
        out.push_str(&rest[..size]);
        pos += size;
        if input[pos..].starts_with("\r\n") {
            pos += 2;
        } else if input[pos..].starts_with('\n') {
            pos += 1;
        } else {
            return None;
        }
        line = take_line(input, &mut pos)?.trim().to_string();
    }
}

pub struct Client {
    host: String,
    port: i32,
    base_path: String,
    model: String,
}

#[derive(Debug, Clone)]
struct HttpBase {
    host: String,
    port: i32,
    base_path: String,
}

fn parse_http_base(base: &str) -> Result<HttpBase> {
    let rest = base
        .strip_prefix("http://")
        .with_context(|| format!("base URL must start with http://, got: {base}"))?;

    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host_port.is_empty() {
        anyhow::bail!("base URL missing host: {base}");
    }

    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() => {
            let port = p
                .parse::<i32>()
                .with_context(|| format!("invalid port in base URL: {base}"))?;
            (h.to_string(), port)
        }
        _ => (host_port.to_string(), 80),
    };

    if !(1..=65535).contains(&port) {
        anyhow::bail!("port out of range in base URL: {base}");
    }

    Ok(HttpBase {
        host,
        port,
        base_path: path.trim_end_matches('/').to_string(),
    })
}

fn join_http_path(base_path: &str, suffix: &str) -> String {
    let base = if base_path.is_empty() { "/" } else { base_path };
    if base == "/" {
        suffix.to_string()
    } else {
        format!("{base}{suffix}")
    }
}

unsafe extern "C" {
    fn sc_http_post_json(
        host: *const c_char,
        port: c_int,
        path: *const c_char,
        json_body: *const c_char,
        bearer_token: *const c_char,
        timeout_secs: c_int,
        status_code: *mut c_int,
        response_body: *mut *mut c_char,
        error_msg: *mut *mut c_char,
    ) -> c_int;
    fn sc_http_free(ptr: *mut c_void);
}

impl Client {
    pub fn new(base: String, model: String) -> Result<Self> {
        let parsed = parse_http_base(&base)?;
        Ok(Self {
            host: parsed.host,
            port: parsed.port,
            base_path: parsed.base_path,
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
        let body = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: if tools.is_empty() {
                None
            } else {
                Some(tools.to_vec())
            },
        };
        let json_body = serde_json::to_string(&body).context("serialize chat request")?;

        let host = self.host.clone();
        let port = self.port;
        let path = join_http_path(&self.base_path, "/chat/completions");
        let api_key_owned = api_key.map(ToOwned::to_owned);

        let (status, text) = tokio::task::spawn_blocking(move || -> Result<(i32, String)> {
            let host_c = CString::new(host).context("host contains NUL")?;
            let path_c = CString::new(path).context("path contains NUL")?;
            let body_c = CString::new(json_body).context("request body contains NUL")?;

            let key_opt = api_key_owned
                .as_deref()
                .filter(|k| !k.is_empty())
                .map(|k| CString::new(k).context("api key contains NUL"))
                .transpose()?;

            let key_ptr = key_opt
                .as_ref()
                .map_or(std::ptr::null(), |s| s.as_ptr());

            let mut status_code: c_int = 0;
            let mut response_ptr: *mut c_char = std::ptr::null_mut();
            let mut error_ptr: *mut c_char = std::ptr::null_mut();

            let rc = unsafe {
                sc_http_post_json(
                    host_c.as_ptr(),
                    port,
                    path_c.as_ptr(),
                    body_c.as_ptr(),
                    key_ptr,
                    120,
                    &mut status_code,
                    &mut response_ptr,
                    &mut error_ptr,
                )
            };

            let response_text = if !response_ptr.is_null() {
                let s = unsafe { CStr::from_ptr(response_ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { sc_http_free(response_ptr.cast::<c_void>()) };
                s
            } else {
                String::new()
            };

            let error_text = if !error_ptr.is_null() {
                let s = unsafe { CStr::from_ptr(error_ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { sc_http_free(error_ptr.cast::<c_void>()) };
                Some(s)
            } else {
                None
            };

            if rc != 0 {
                anyhow::bail!(
                    "chat request failed: {}",
                    error_text.unwrap_or_else(|| "unknown C HTTP error".into())
                );
            }

            Ok((status_code, response_text))
        })
        .await
        .context("join C HTTP task")??;

        if !(200..300).contains(&status) {
            anyhow::bail!("HTTP {} — {}", status, text.trim());
        }

        let text = maybe_decode_chunked_body(&text).unwrap_or(text);

        let parsed: ChatResponse = serde_json::from_str(&text)
            .with_context(|| format!("parse JSON: {}", text.chars().take(200).collect::<String>()))?;

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
