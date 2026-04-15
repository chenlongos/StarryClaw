//! StarryClaw — small agent with OpenAI-compatible chat + local tools (ls / mkdir).
//!
//! **默认即连线模式**：直连本机 Ollama（OpenAI 兼容 `/v1`），无需任何开关。
//! 只有显式设置 `STARRYCLAW_OFFLINE=1` 时才走离线模糊规则（不访问网络）。
//!
//! Env:
//!   STARRYCLAW_BASE_URL / STARRYCLAW_MODEL — 覆盖下方默认（仍指向 Ollama 时可只改端口等）
//!   STARRYCLAW_API_KEY / OPENAI_API_KEY — 需要时带 `Authorization: Bearer …`（Ollama 一般不用）
//!   STARRYCLAW_OFFLINE=1 — 离线，不连 Ollama
//!   NO_COLOR / STARRYCLAW_NO_COLOR — 若设置则提示符不用 ANSI 颜色
//!
//! **主机用 localhost、QEMU 里 StarryOS 怎么访问 PC 上的 Ollama？**\
//! - `localhost` / `127.0.0.1` 永远指**当前这台系统自己**。在虚拟机里写 `localhost:11434` 只会找 **StarryOS 自己**，不会穿透到外面的 PC。\
//! - 要在 StarryOS 里连 **宿主机（你跑 QEMU 的那台 PC）** 上的 Ollama，必须用「从虚拟机视角能到达宿主机」的地址，并用环境变量配置（不要改源码里的默认）：\
//!   1. **PC 上** Ollama 必须监听 `0.0.0.0:11434`（例如 `OLLAMA_HOST=0.0.0.0 ollama serve`），否则只绑 `127.0.0.1` 时连 SLIRP 都进不来。\
//!   2. **QEMU 默认 user 网络 (SLIRP)**：在 StarryOS 里设 `STARRYCLAW_BASE_URL=http://10.0.2.2:11434/v1`（`10.0.2.2` 是 QEMU 规定的「宿主机」地址，相当于从虚拟机看 PC）。\
//!   3. **桥接 / 与 PC 同一局域网**：在 StarryOS 里设 `STARRYCLAW_BASE_URL=http://<PC的局域网IP>:11434/v1`（如 `192.168.1.x`）。\
//! 4. PC 防火墙放行 TCP **11434**；StarryOS 镜像需启用网络（如 `NET=y`）。
//!
//! 运行：\
//! - 调试：`cargo run`（`target/debug/starryclaw`）\
//! - **发布优化**：`cargo run --release`（注意是**两个减号** `--release`，不是 `cargo run release`）\
//! - 或直接：`./target/release/starryclaw`（需先 `cargo build --release`）\
//! 不要写成 `cargo run / target/release/...`（会把路径当成程序参数）。

/// 默认 Ollama base（本机开发）；QEMU 内请用环境变量 STARRYCLAW_BASE_URL 指向 10.0.2.2 或宿主机局域网 IP
const DEFAULT_OLLAMA_BASE: &str = "http://192.168.1.8:11435/v1";
const DEFAULT_OLLAMA_MODEL: &str = "kimi-k2.5:cloud";

fn color_enabled() -> bool {
    env::var("NO_COLOR").is_err() && env::var("STARRYCLAW_NO_COLOR").is_err()
}

fn truncate_model_label(m: &str) -> String {
    let mut it = m.chars();
    let head: String = it.by_ref().take(32).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn warn_extra_cli_args() {
    let extra: Vec<String> = env::args().skip(1).collect();
    if extra.is_empty() {
        return;
    }
    if extra.len() == 1 && extra[0] == "release" {
        eprintln!(
            "提示：要跑 release 优化版请用：cargo run --release（--release 是两个减号，写在 cargo 后面）。\n\
             「cargo run release」里的 release 会被当成程序参数，所以仍是 debug。\n\
             或直接：cargo build --release && ./target/release/starryclaw"
        );
        return;
    }
    eprintln!(
        "警告：忽略多余参数 {:?}。正确示例：cargo run、cargo run --release、./target/release/starryclaw。不要 cargo run / path。",
        extra
    );
}

fn print_input_prompt(offline: bool, ollama_model: Option<&str>) {
    if offline {
        if color_enabled() {
            print!("\x1b[1;36mStarryClaw\x1b[0m \x1b[90m· 离线\x1b[0m › ");
        } else {
            print!("StarryClaw · 离线 › ");
        }
        return;
    }
    let label = ollama_model.map(truncate_model_label).unwrap_or_else(|| "?".into());
    if color_enabled() {
        print!(
            "\x1b[1;36mStarryClaw\x1b[0m \x1b[90m· ollama · {}\x1b[0m › ",
            label
        );
    } else {
        print!("StarryClaw · ollama · {label} › ");
    }
}

/// `print!` 走 stdio 缓冲，必须用 std::stdout 刷新，tokio::stdout().flush 刷不到
fn flush_std_stdout() {
    let _ = std::io::stdout().flush();
}

fn print_banner_offline() {
    println!();
    if color_enabled() {
        println!("\x1b[90m┌─ StarryClaw · 离线\x1b[0m");
        println!("\x1b[90m│  此处可输入：口语列目录、建文件夹等；按 Enter 发送，quit 退出\x1b[0m");
        println!("\x1b[90m└──────────────────────────────────────────────\x1b[0m");
    } else {
        println!("── StarryClaw · 离线 ──");
        println!("  在下方提示符后输入，按 Enter 发送；quit 退出");
        println!("────────────────────────");
    }
    println!();
}

fn print_banner_online(model: &str) {
    let label = truncate_model_label(model);
    println!();
    if color_enabled() {
        println!("\x1b[90m┌─ StarryClaw · 已连接 Ollama · {}\x1b[0m", label);
        println!("\x1b[90m│  在提示符后输入问题或指令，按 Enter 发送；quit 退出\x1b[0m");
        println!("\x1b[90m└──────────────────────────────────────────────\x1b[0m");
    } else {
        println!("── StarryClaw · Ollama · {} ──", label);
        println!("  在下方提示符后输入，按 Enter 发送；quit 退出");
        println!("────────────────────────");
    }
    println!();
}

mod intent;
mod openai;
mod tools;

use anyhow::{Context, Result};
use openai::{openai_tool_definitions, ChatMessage, Client, ToolCall};
use serde_json::Value;
use std::env;
use std::io::Write;
use std::time::Instant;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use tools::{ChangeDirTool, ListDirTool, MkdirTool, ReadFileTool, RunShellTool, Tool, ToolResult};

fn dispatch_offline(line: &str) -> String {
    if line.trim().is_empty() {
        return String::new();
    }
    intent::dispatch_fuzzy_offline(line).unwrap_or_else(|| {
        "没听懂内置的列目录/建目录/进目录/读文件/受限 shell（date、uname 等）。你可以在 shell 里自行试其它命令（如 grep、find、free）；若仍无法完成，请把需求说具体些。"
            .into()
    })
}

async fn agent_turn(
    client: &Client,
    api_key: Option<&str>,
    user_text: &str,
    messages: &mut Vec<ChatMessage>,
) -> Result<String> {
    let defs = openai_tool_definitions();
    let user_payload = format!(
        "User instruction (may be vague, colloquial, or Chinese short phrases — infer intent):\n{}",
        user_text
    );
    messages.push(ChatMessage {
        role: "user".into(),
        content: Some(Value::String(user_payload)),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });

    let max_rounds = 8;
    let mut final_text = String::new();
    let mut tool_fallback = String::new();

    for _ in 0..max_rounds {
        let (assistant, text) = client
            .chat(api_key, messages.clone(), &defs)
            .await
            .context("model request")?;

        let tool_calls = assistant.tool_calls.clone();

        let assistant_content = match (&text, &tool_calls) {
            (Some(s), _) => Some(Value::String(s.clone())),
            (None, Some(tcs)) if !tcs.is_empty() => Some(Value::Null),
            _ => None,
        };

        let assistant_msg = ChatMessage {
            role: "assistant".into(),
            content: assistant_content,
            tool_calls: tool_calls.clone(),
            tool_call_id: None,
            name: None,
        };
        messages.push(assistant_msg);

        if let Some(tcs) = tool_calls.filter(|v| !v.is_empty()) {
            let mut batch = String::new();
            for tc in tcs {
                let out = run_one_tool_call(&tc)?;
                let piece = out.to_tool_message_content();
                if !batch.is_empty() {
                    batch.push('\n');
                }
                batch.push_str(&piece);
                messages.push(ChatMessage {
                    role: "tool".into(),
                    content: Some(Value::String(piece)),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: None,
                });
            }
            tool_fallback = batch;
            continue;
        }

        if let Some(t) = text {
            final_text = t;
        }
        break;
    }

    let trimmed = final_text.trim();
    if trimmed.is_empty() && !tool_fallback.is_empty() {
        return Ok(tool_fallback);
    }
    if trimmed.is_empty() {
        if let Some(fuzzy) = intent::dispatch_fuzzy_offline(user_text) {
            return Ok(fuzzy);
        }
    }

    Ok(final_text)
}

fn run_one_tool_call(tc: &ToolCall) -> Result<ToolResult> {
    if tc.call_type != "function" {
        anyhow::bail!("unsupported tool type: {}", tc.call_type);
    }
    tools::run_tool_from_json(&tc.function.name, &tc.function.arguments)
}

#[tokio::main]
async fn main() -> Result<()> {
    warn_extra_cli_args();

    let offline = env::var("STARRYCLAW_OFFLINE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let api_key = env::var("STARRYCLAW_API_KEY")
        .or_else(|_| env::var("OPENAI_API_KEY"))
        .ok();

    if offline {
        let list = ListDirTool;
        let mkdir = MkdirTool;
        let cd = ChangeDirTool;
        let cat = ReadFileTool;
        let run_sh = RunShellTool;
        eprintln!(
            "StarryClaw 离线模式（环境变量 STARRYCLAW_OFFLINE=1）。要默认连线 Ollama：unset STARRYCLAW_OFFLINE 后重新运行。"
        );
        eprintln!(
            "工具: {} / {} / {} / {} / {}",
            list.name(),
            mkdir.name(),
            cd.name(),
            cat.name(),
            run_sh.name()
        );
        print_banner_offline();
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();
        loop {
            line.clear();
            print_input_prompt(true, None);
            flush_std_stdout();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            if line.trim() == "quit" || line.trim() == "exit" {
                break;
            }
            let out = dispatch_offline(&line);
            if !out.is_empty() {
                println!();
                println!("{out}");
                println!();
            }
        }
        return Ok(());
    }

    let base = env::var("STARRYCLAW_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_OLLAMA_BASE.into());
    let model = env::var("STARRYCLAW_MODEL")
        .unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into());

    let client = Client::new(base, model)?;

    let mut messages = vec![ChatMessage {
        role: "system".into(),
        content: Some(Value::String(
            "You are StarryClaw (spell the name exactly StarryClaw, never StaryClaw), an autonomous agent on a Unix-like system (e.g. StarryOS). \
             Tools: list_dir; mkdir (single segment under cwd); change_dir; read_file (text, size-capped); run_shell (single allowlisted program + args, no pipes/shell—see tool schema). \
             Prefer tools when they match: 查目录/列文件→list_dir; 进目录→change_dir; 看文件内容→read_file; 建文件夹→mkdir; 日期/时间/今天几号/uname/pwd/whoami/df/cal 等只读系统信息→run_shell（如 date、date +%Y-%m-%d、uname -a）. \
             When the need is NOT covered by any tool (e.g. grep, curl, free, ps): tell them they can try in their terminal. Start with 「可在 shell 中自行尝试：」and give 1–3 concrete commands with a one-line explanation each. Prefer read-only suggestions; never suggest curl|sh or rm -rf /. \
             If nothing fits (pure chat, too vague), say e.g. 「当前没有合适的内置工具，也想不到可建议的系统命令。」and optionally one short clarifying question. \
             After tool calls, summarize briefly in the user's language."
                .into(),
        )),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }];

    print_banner_online(client.model());

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        print_input_prompt(false, Some(client.model()));
        flush_std_stdout();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        if line.trim() == "quit" || line.trim() == "exit" {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let started = Instant::now();
        match agent_turn(&client, api_key.as_deref(), line.trim(), &mut messages).await {
            Ok(reply) => {
                let reply = reply.trim();
                let elapsed_secs = started.elapsed().as_secs_f64();
                println!();
                println!("elapsed: {:.3}s", elapsed_secs);
                if reply.is_empty() {
                    println!("（没有收到模型回复。可再说具体些；列目录/进目录/读文件/建目录/日期时间等可说明；其它需求模型可建议你在 shell 里试的命令。）");
                } else {
                    println!("{reply}");
                }
                println!();
            }
            Err(e) => {
                let elapsed_secs = started.elapsed().as_secs_f64();
                println!();
                eprintln!("elapsed: {:.3}s", elapsed_secs);
                eprintln!("error: {e:#}");
                println!();
            }
        }
    }

    Ok(())
}
