//! Minimal tool layer modeled after zeroclaw-api `Tool` / `ToolResult`.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

pub trait Tool {
    fn name(&self) -> &str;
    fn execute(&self, args: &str) -> ToolResult;
}

impl ToolResult {
    pub fn to_tool_message_content(&self) -> String {
        let mut s = String::new();
        if !self.output.is_empty() {
            s.push_str(&self.output);
            if !self.output.ends_with('\n') {
                s.push('\n');
            }
        }
        if self.success {
            s.push_str("[ok]");
        } else if let Some(e) = &self.error {
            s.push_str(&format!("[error] {e}"));
        } else {
            s.push_str("[error]");
        }
        s
    }
}

fn unsafe_path_token(path: &str) -> bool {
    path.contains(';')
        || path.contains('|')
        || path.contains('`')
        || path.contains('\0')
        || path.len() > 4096
}

/// Default cap for read_file (bytes).
pub const DEFAULT_READ_MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn change_dir_path(path: &str) -> ToolResult {
    let path = path.trim();
    if path.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("missing path".into()),
        };
    }
    if unsafe_path_token(path) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("unsafe path".into()),
        };
    }
    match std::env::set_current_dir(path) {
        Ok(()) => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".into());
            ToolResult {
                success: true,
                output: format!("cwd: {cwd}\n"),
                error: None,
            }
        }
        Err(e) => ToolResult {
            success: false,
            output: String::new(),
            error: Some(e.to_string()),
        },
    }
}

pub(crate) fn read_file_path(path: &str, max_bytes: u64) -> ToolResult {
    let path = path.trim();
    if path.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("missing path".into()),
        };
    }
    if unsafe_path_token(path) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("unsafe path".into()),
        };
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            };
        }
    };
    if !meta.is_file() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("not a regular file".into()),
        };
    }
    let len = meta.len();
    if len > max_bytes {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!(
                "file too large ({} bytes, max {})",
                len, max_bytes
            )),
        };
    }
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            };
        }
    };
    let mut buf = Vec::new();
    if let Err(e) = f.take(max_bytes + 1).read_to_end(&mut buf) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(e.to_string()),
        };
    }
    if buf.len() as u64 > max_bytes {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!("read exceeds max_bytes ({max_bytes})")),
        };
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    ToolResult {
        success: true,
        output: text,
        error: None,
    }
}

/// 仅允许只读/低风险单程序调用（无 sh -c，无管道）。用于 date、uname 等，由模型或离线意图触发。
const RUN_SHELL_ALLOW: &[&str] = &[
    "arch",
    "basename",
    "cal",
    "date",
    "df",
    "dirname",
    "echo",
    "env",
    "false",
    "file",
    "getconf",
    "groups",
    "head",
    "hostname",
    "id",
    "nproc",
    "pwd",
    "readlink",
    "seq",
    "stat",
    "tail",
    "true",
    "tty",
    "uname",
    "uptime",
    "wc",
    "which",
    "whoami",
];

const RUN_SHELL_OUTPUT_CAP: usize = 64 * 1024;

fn truncate_tool_output(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = s;
    out.truncate(end);
    out.push_str("\n… [output truncated]\n");
    out
}

pub(crate) fn is_allowlisted_shell_program(prog: &str) -> bool {
    let prog = prog.strip_prefix("./").unwrap_or(prog);
    RUN_SHELL_ALLOW.iter().any(|&a| a == prog)
}

fn shell_args_safe(parts: &[&str]) -> Option<&'static str> {
    for a in parts {
        if a.contains(';')
            || a.contains('|')
            || a.contains('&')
            || a.contains('`')
            || a.contains('$')
            || a.contains('(')
            || a.contains(')')
            || a.contains('\n')
            || a.contains('\r')
        {
            return Some("不允许的 shell 元字符（如 | ; & $ `）");
        }
    }
    None
}

pub(crate) fn run_allowlisted_shell(command: &str) -> ToolResult {
    let cmd = command.trim();
    if cmd.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("empty command".into()),
        };
    }
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("empty command".into()),
        };
    }
    let prog = parts[0].strip_prefix("./").unwrap_or(parts[0]);
    if !is_allowlisted_shell_program(prog) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!(
                "「{prog}」不在允许列表。可改用其它内置工具，或在 shell 里自行执行。"
            )),
        };
    }
    if let Some(msg) = shell_args_safe(&parts) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(msg.into()),
        };
    }
    let args: Vec<&str> = parts[1..].to_vec();
    let mut r = run_cmd(prog, &args);
    r.output = truncate_tool_output(r.output, RUN_SHELL_OUTPUT_CAP);
    if let Some(e) = r.error.take() {
        r.error = Some(truncate_tool_output(e, RUN_SHELL_OUTPUT_CAP));
    }
    r
}

fn run_cmd(program: &str, args: &[&str]) -> ToolResult {
    let out = Command::new(program).args(args).output();
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            let ok = o.status.success();
            ToolResult {
                success: ok,
                output: stdout,
                error: if ok {
                    None
                } else if stderr.is_empty() {
                    Some(format!("exit {}", o.status))
                } else {
                    Some(stderr)
                },
            }
        }
        Err(e) => ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!("failed to run {program}: {e}")),
        },
    }
}

pub struct ListDirTool;

impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn execute(&self, args: &str) -> ToolResult {
        let path = args.trim();
        let path = if path.is_empty() { "." } else { path };
        list_dir_path(path)
    }
}

fn list_dir_path(path: &str) -> ToolResult {
    if unsafe_path_token(path) {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("unsafe path".into()),
        };
    }
    run_cmd("ls", &["-la", path])
}

pub struct MkdirTool;

impl Tool for MkdirTool {
    fn name(&self) -> &str {
        "mkdir"
    }

    fn execute(&self, args: &str) -> ToolResult {
        mkdir_name(args.trim())
    }
}

fn mkdir_name(name: &str) -> ToolResult {
    if name.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("missing directory name".into()),
        };
    }
    if name.contains('/') || name.contains("..") {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("only a single path segment allowed for now".into()),
        };
    }
    if !Path::new(name)
        .file_name()
        .is_some_and(|s| !s.is_empty())
    {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some("invalid name".into()),
        };
    }
    run_cmd("mkdir", &["-p", name])
}

pub struct ChangeDirTool;

impl Tool for ChangeDirTool {
    fn name(&self) -> &str {
        "change_dir"
    }

    fn execute(&self, args: &str) -> ToolResult {
        change_dir_path(args)
    }
}

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn execute(&self, args: &str) -> ToolResult {
        read_file_path(args, DEFAULT_READ_MAX_BYTES)
    }
}

pub struct RunShellTool;

impl Tool for RunShellTool {
    fn name(&self) -> &str {
        "run_shell"
    }

    fn execute(&self, args: &str) -> ToolResult {
        run_allowlisted_shell(args)
    }
}

/// Run tool from model JSON arguments (`function.arguments` string).
pub fn run_tool_from_json(name: &str, arguments_json: &str) -> Result<ToolResult> {
    let v: Value = serde_json::from_str(arguments_json).context("parse tool arguments JSON")?;
    match name {
        "list_dir" => {
            let path = v
                .get("path")
                .and_then(|x| x.as_str())
                .unwrap_or(".")
                .trim();
            let path = if path.is_empty() { "." } else { path };
            Ok(list_dir_path(path))
        }
        "mkdir" => {
            let name = v
                .get("name")
                .and_then(|x| x.as_str())
                .context("mkdir requires string field \"name\"")?;
            Ok(mkdir_name(name.trim()))
        }
        "change_dir" => {
            let path = v
                .get("path")
                .and_then(|x| x.as_str())
                .context("change_dir requires string field \"path\"")?;
            Ok(change_dir_path(path.trim()))
        }
        "read_file" => {
            let path = v
                .get("path")
                .and_then(|x| x.as_str())
                .context("read_file requires string field \"path\"")?;
            let max = v
                .get("max_bytes")
                .and_then(|x| x.as_u64())
                .unwrap_or(DEFAULT_READ_MAX_BYTES)
                .min(2 * 1024 * 1024);
            Ok(read_file_path(path.trim(), max))
        }
        "run_shell" => {
            let command = v
                .get("command")
                .and_then(|x| x.as_str())
                .context("run_shell requires string field \"command\"")?;
            Ok(run_allowlisted_shell(command.trim()))
        }
        _ => anyhow::bail!("unknown tool: {name}"),
    }
}
