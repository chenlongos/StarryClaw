//! Minimal tool layer: `ToolResult` + **LangChain 风格**的 `ToolDef`（名字、描述、参数 JSON Schema、`run`）。
//!
//! **加一个新工具**：写底层逻辑（如有）→ `fn tool_xxx(v: &Value) -> Result<ToolResult>` → 紧挨着的 `fn xxx_def() -> ToolDef`（名字、描述、parameters、`run`）→ 在 `build_tool_registry` 里追加 `xxx_def()` 一行。
//!
//! **轮子**：`wheel_move` 成功时固定格式 `【执行命令】 wheel -> …` + 下一行 `ok`（并 `println!`）；可选 `STARRYCLAW_WHEEL_CMD` 时再打一行 `(stub)` 拟执行命令。
//! **机械臂**：`arm_action` 支持 `grab`（抓取）/`release`（放下），输出 `【执行命令】 arm -> …` + `ok`；可选 `STARRYCLAW_ARM_CMD` 仅打印 `(stub)`。

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// 与 LangChain `tool(handler, { name, description, schema })` 类似：名字、说明、JSON Schema、执行函数。
#[derive(Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// `function.parameters`：完整 object schema（`type` / `properties` / `required`）。
    pub parameters: Value,
    pub run: fn(&Value) -> Result<ToolResult>,
}

fn openai_tool_entry(d: &ToolDef) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": d.name,
            "description": d.description,
            "parameters": d.parameters,
        }
    })
}

static TOOL_REGISTRY: OnceLock<Vec<ToolDef>> = OnceLock::new();

fn tool_registry() -> &'static [ToolDef] {
    TOOL_REGISTRY.get_or_init(build_tool_registry).as_slice()
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
            // wheel_move / arm_action 已自带「… ok」结尾，避免再拼一层 [ok]
            let custom_done = (self.output.contains("【执行命令】 wheel ->")
                || self.output.contains("【执行命令】 arm ->"))
                && self.output.trim_end().ends_with("ok");
            if !custom_done {
                s.push_str("[ok]");
            }
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

fn tool_list_dir(v: &Value) -> Result<ToolResult> {
    let path = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or(".")
        .trim();
    let path = if path.is_empty() { "." } else { path };
    Ok(list_dir_path(path))
}

fn list_dir_def() -> ToolDef {
    ToolDef {
        name: "list_dir",
        description: "List files in a directory (runs ls -la). Use for fuzzy requests: 看看、有啥、列一下、当前目录、show what's here、list files、browse、打开看看、里有什么.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path; use . for current directory"
                }
            },
            "required": []
        }),
        run: tool_list_dir,
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

fn tool_mkdir(v: &Value) -> Result<ToolResult> {
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .context("mkdir requires string field \"name\"")?;
    Ok(mkdir_name(name.trim()))
}

fn mkdir_def() -> ToolDef {
    ToolDef {
        name: "mkdir",
        description: "Create one directory (mkdir -p), name only, no path. Fuzzy: 建个xx、新建文件夹、帮我创建、make a folder、mkdir foo、弄个目录叫.",
        parameters: json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Single directory name in the current working directory (no slashes or ..)"
                }
            },
            "required": ["name"]
        }),
        run: tool_mkdir,
    }
}

fn tool_change_dir(v: &Value) -> Result<ToolResult> {
    let path = v
        .get("path")
        .and_then(|x| x.as_str())
        .context("change_dir requires string field \"path\"")?;
    Ok(change_dir_path(path.trim()))
}

fn change_dir_def() -> ToolDef {
    ToolDef {
        name: "change_dir",
        description: "Change current working directory (cd). Use for: 进入xx目录、切换到、cd foo、chdir、去某个文件夹.",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Target directory path (relative or absolute, .. allowed)"
                }
            },
            "required": ["path"]
        }),
        run: tool_change_dir,
    }
}

fn tool_read_file(v: &Value) -> Result<ToolResult> {
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

fn read_file_def() -> ToolDef {
    ToolDef {
        name: "read_file",
        description: "Read text file contents (UTF-8 / lossy). Use for: 查看文件、读一下、cat、show content、打开某某文件.",
        parameters: json!({
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
        }),
        run: tool_read_file,
    }
}

fn tool_run_shell(v: &Value) -> Result<ToolResult> {
    let command = v
        .get("command")
        .and_then(|x| x.as_str())
        .context("run_shell requires string field \"command\"")?;
    Ok(run_allowlisted_shell(command.trim()))
}

fn run_shell_def() -> ToolDef {
    ToolDef {
        name: "run_shell",
        description: "Run a single allowlisted program with arguments (no shell, no pipes). For 今天几号/日期/时间 call date (e.g. date +%Y-%m-%d). For kernel/OS: uname -a. Also: pwd, whoami, hostname, uptime, cal, df, env, which, wc, head, tail, id, stat, file, readlink, etc. If not in this tool's allowlist (e.g. free, grep, curl), tell the user to run it in their terminal. Prefer run_shell over only suggesting markdown when the command is allowlisted.",
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "One program name plus args, whitespace-separated (e.g. \"date +%Y-%m-%d\", \"uname -a\"). No ; | & $ ` or newlines."
                }
            },
            "required": ["command"]
        }),
        run: tool_run_shell,
    }
}

fn normalize_wheel_direction(raw: &str) -> Option<&'static str> {
    let t = raw.trim();
    match t {
        "前" | "前进" => return Some("forward"),
        "后" | "后退" | "倒" => return Some("backward"),
        "左" | "左转" => return Some("left"),
        "右" | "右转" => return Some("right"),
        _ => {}
    }
    match t.to_lowercase().as_str() {
        "forward" | "fwd" | "f" | "up" => Some("forward"),
        "backward" | "back" | "b" | "down" => Some("backward"),
        "left" | "l" => Some("left"),
        "right" | "r" => Some("right"),
        _ => None,
    }
}

/// 给人看的动作短句：直行带距离、转向带「转」字。
fn wheel_action_line(canonical: &str, distance: &Option<String>) -> String {
    let d = distance.as_deref();
    match canonical {
        "forward" => match d {
            Some(x) => format!("向前 {x}"),
            None => "向前".into(),
        },
        "backward" => match d {
            Some(x) => format!("向后 {x}"),
            None => "向后".into(),
        },
        "left" => match d {
            Some(x) => format!("向左转 {x}"),
            None => "向左转".into(),
        },
        "right" => match d {
            Some(x) => format!("向右转 {x}"),
            None => "向右转".into(),
        },
        _ => match d {
            Some(x) => format!("{canonical} {x}"),
            None => canonical.to_string(),
        },
    }
}

fn wheel_done_message(action_line: &str) -> String {
    format!("【执行命令】 wheel -> {action_line}\nok\n")
}

/// 可选移动量，如 `5mm`、`1cm`；过长或含危险字符则忽略。
fn sanitize_wheel_distance(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() > 32 {
        return None;
    }
    if s.contains(';')
        || s.contains('|')
        || s.contains('&')
        || s.contains('`')
        || s.contains('$')
        || s.contains('\n')
        || s.contains('\r')
    {
        return None;
    }
    Some(s.to_string())
}

fn wheel_move(direction: &str, distance_raw: Option<&str>) -> ToolResult {
    let Some(canonical) = normalize_wheel_direction(direction) else {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!(
                "未知方向 {direction:?}；请用 forward / backward / left / right 或 前 / 后 / 左 / 右"
            )),
        };
    };
    let distance = sanitize_wheel_distance(distance_raw);
    let action_line = wheel_action_line(canonical, &distance);
    let done = wheel_done_message(&action_line);

    if let Ok(cmd_line) = env::var("STARRYCLAW_WHEEL_CMD") {
        let parts: Vec<&str> = cmd_line.split_whitespace().filter(|s| !s.is_empty()).collect();
        if !parts.is_empty() {
            let prog = parts[0];
            let mut tokens: Vec<String> = parts[1..].iter().map(|s| (*s).to_string()).collect();
            tokens.push(canonical.to_string());
            if let Some(ref d) = distance {
                tokens.push(d.clone());
            }
            let stub_cmd = std::iter::once(prog.to_string())
                .chain(tokens.into_iter())
                .collect::<Vec<_>>()
                .join(" ");
            println!("{}", done.trim_end());
            println!("(stub) {stub_cmd}");
            return ToolResult {
                success: true,
                output: done,
                error: None,
            };
        }
    }

    println!("{}", done.trim_end());
    ToolResult {
        success: true,
        output: done,
        error: None,
    }
}

fn tool_wheel_move(v: &Value) -> Result<ToolResult> {
    let direction = v
        .get("direction")
        .and_then(|x| x.as_str())
        .context("wheel_move requires string field \"direction\"")?;
    let distance = v.get("distance").and_then(|x| x.as_str());
    Ok(wheel_move(direction, distance))
}

fn wheel_move_def() -> ToolDef {
    ToolDef {
        name: "wheel_move",
        description: "机器人底盘轮子：直行/后退带 optional distance（如 backward + 4m → 向后 4m）；左转/右转无距离时说「向左转」。终端与工具结果会输出两行：「【执行命令】 wheel -> …」与「ok」。可选 STARRYCLAW_WHEEL_CMD：额外 println 一行 (stub) 拟执行命令，不 exec。",
        parameters: json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "description": "One of: forward, backward, left, right（或 前、后、左、右 / 前进、后退、左转、右转）."
                },
                "distance": {
                    "type": "string",
                    "description": "Optional move amount, e.g. 5mm, 1cm, 15deg for turns."
                }
            },
            "required": ["direction"]
        }),
        run: tool_wheel_move,
    }
}

fn normalize_arm_action(raw: &str) -> Option<&'static str> {
    let t = raw.trim();
    match t {
        "抓" | "抓取" | "抓住" => return Some("grab"),
        "放" | "放下" | "松开" => return Some("release"),
        _ => {}
    }
    match t.to_lowercase().as_str() {
        "grab" | "grip" | "pick" | "pick_up" => Some("grab"),
        "release" | "drop" | "put_down" | "open" => Some("release"),
        _ => None,
    }
}

fn arm_action_line(canonical: &str) -> &'static str {
    match canonical {
        "grab" => "抓取",
        "release" => "放下",
        _ => "动作",
    }
}

fn arm_done_message(action_line: &str) -> String {
    format!("【执行命令】 arm -> {action_line}\nok\n")
}

fn arm_action(action: &str) -> ToolResult {
    let Some(canonical) = normalize_arm_action(action) else {
        return ToolResult {
            success: false,
            output: String::new(),
            error: Some(format!(
                "未知机械臂动作 {action:?}；请用 grab/release 或 抓取/放下"
            )),
        };
    };

    let action_line = arm_action_line(canonical);
    let done = arm_done_message(action_line);

    if let Ok(cmd_line) = env::var("STARRYCLAW_ARM_CMD") {
        let parts: Vec<&str> = cmd_line.split_whitespace().filter(|s| !s.is_empty()).collect();
        if !parts.is_empty() {
            let prog = parts[0];
            let mut tokens: Vec<String> = parts[1..].iter().map(|s| (*s).to_string()).collect();
            tokens.push(canonical.to_string());
            let stub_cmd = std::iter::once(prog.to_string())
                .chain(tokens.into_iter())
                .collect::<Vec<_>>()
                .join(" ");
            println!("{}", done.trim_end());
            println!("(stub) {stub_cmd}");
            return ToolResult {
                success: true,
                output: done,
                error: None,
            };
        }
    }

    println!("{}", done.trim_end());
    ToolResult {
        success: true,
        output: done,
        error: None,
    }
}

fn tool_arm_action(v: &Value) -> Result<ToolResult> {
    let action = v
        .get("action")
        .and_then(|x| x.as_str())
        .context("arm_action requires string field \"action\"")?;
    Ok(arm_action(action))
}

fn arm_action_def() -> ToolDef {
    ToolDef {
        name: "arm_action",
        description: "机械臂动作：抓取或放下。用户说「机械臂抓取」「抓住」「机械臂放下」「松开」时调用。输出两行：`【执行命令】 arm -> 抓取/放下` 与 `ok`。可选 STARRYCLAW_ARM_CMD：额外 println 一行 (stub) 拟执行命令，不 exec。",
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "One of: grab, release（或 抓取、放下、抓、放、松开）."
                }
            },
            "required": ["action"]
        }),
        run: tool_arm_action,
    }
}

/// 集中注册：顺序即对外暴露给模型的工具顺序。加工具时在文件上方写 `*_def` + `tool_*`，再在此处追加一行。
fn build_tool_registry() -> Vec<ToolDef> {
    vec![
        list_dir_def(),
        mkdir_def(),
        change_dir_def(),
        read_file_def(),
        run_shell_def(),
        wheel_move_def(),
        arm_action_def(),
    ]
}

/// Chat Completions `tools` 数组（由注册表生成）。
pub fn openai_tool_definitions() -> Vec<Value> {
    tool_registry().iter().map(openai_tool_entry).collect()
}

/// Run tool from model JSON arguments (`function.arguments` string).
pub fn run_tool_from_json(name: &str, arguments_json: &str) -> Result<ToolResult> {
    let v: Value = serde_json::from_str(arguments_json).context("parse tool arguments JSON")?;
    let def = tool_registry()
        .iter()
        .find(|d| d.name == name)
        .with_context(|| format!("unknown tool: {name}"))?;
    (def.run)(&v)
}
