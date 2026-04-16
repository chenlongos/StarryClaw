//! StarryClaw — small agent with OpenAI-compatible chat + local tools (ls / mkdir).
//!
//! **仅在线智能体**：直连本机或局域网 Ollama（OpenAI 兼容 `/v1`），模型通过 tool calling 驱动本地工具。
//!
//! Env:
//!   STARRYCLAW_BASE_URL / STARRYCLAW_MODEL — 覆盖下方默认（仍指向 Ollama 时可只改端口等）
//!   STARRYCLAW_API_KEY / OPENAI_API_KEY — 需要时带 `Authorization: Bearer …`（Ollama 一般不用）
//!   STARRYCLAW_WHEEL_CMD — 可选；`wheel_move` 仅 `println!` 打印「该命令 + forward|backward|left|right」，不 exec
//!   NO_COLOR / STARRYCLAW_NO_COLOR — 若设置则提示符不用 ANSI 颜色

const DEFAULT_OLLAMA_BASE: &str = "http://192.168.123.247:11434/v1";
const DEFAULT_OLLAMA_MODEL: &str = "kimi-k2.5:cloud";

fn color_enabled() -> bool {
    std::env::var("NO_COLOR").is_err() && std::env::var("STARRYCLAW_NO_COLOR").is_err()
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
    let extra: Vec<String> = std::env::args().skip(1).collect();
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

fn print_input_prompt(ollama_model: Option<&str>) {
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

fn flush_std_stdout() {
    let _ = std::io::stdout().flush();
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

mod openai;
mod tools;

use anyhow::{Context, Result};
use openai::{ChatMessage, Client, ToolCall};
use serde_json::Value;
use std::io::Write;
use tokio::io::{self, AsyncBufReadExt, BufReader};

use tools::ToolResult;

// 简化版系统提示（与 Python 测试保持一致）
const SYSTEM_PROMPT: &str = "You are a robot control agent. You MUST use the provided tools to perform actions. Never output plain text steps; instead, call the appropriate tool (wheel_move, arm_action, camera_capture, object_detect). When the task is complete, you may output a short summary.";

#[derive(Clone, Copy, Default)]
struct RobotTaskNeed {
    need_camera_detect: bool,
    need_move: bool,
    need_grab: bool,
    need_release: bool,
    wheel_min: usize,
}

#[derive(Default)]
struct ToolProgress {
    camera_count: usize,
    detect_count: usize,
    wheel_count: usize,
    arm_grab_count: usize,
    arm_release_count: usize,
}

fn infer_robot_task_need(user_text: &str) -> Option<RobotTaskNeed> {
    let t = user_text.to_lowercase();
    let need_move = [
        "走", "前进", "后退", "左转", "右转", "靠近", "放到", "放在", "路径", "绕", "一圈",
        "move", "path", "walk", "drive",
    ]
    .iter()
    .any(|k| t.contains(k));
    let shape_with_motion = need_move
        && (t.contains("正方形")
            || t.contains("三角形")
            || t.contains("圆形")
            || t.contains("矩形")
            || t.contains("长方形")
            || t.contains("square")
            || t.contains("rectangle")
            || t.contains("triangle")
            || t.contains("circle"));
    let has_robot = [
        "小车", "轮子", "机械臂", "抓", "放", "拍照", "识别", "寻找", "找到", "杯子", "衣服",
        "走", "前进", "后退", "左转", "右转", "路径", "圆形",
        "wheel", "arm", "grab", "release", "camera", "detect", "move", "path",
    ]
    .iter()
    .any(|k| t.contains(k))
        || shape_with_motion;
    if !has_robot {
        return None;
    }
    let mut wheel_min = if need_move { 1 } else { 0 };
    if need_move {
        if t.contains("正方形")
            || t.contains("矩形")
            || t.contains("长方形")
            || t.contains("square")
            || t.contains("rectangle")
        {
            wheel_min = wheel_min.max(8);
        } else if t.contains("三角形") || t.contains("triangle") {
            wheel_min = wheel_min.max(6);
        } else if t.contains("圆形") || t.contains("绕一圈") || t.contains("circle") {
            wheel_min = wheel_min.max(8);
        }
    }
    let need_grab = ["抓", "捡", "拿起", "pick", "grab"]
        .iter()
        .any(|k| t.contains(k));
    let explicit_release = [
        "放到", "放在", "放下", "放进", "放入", "装在", "装进", "装入", "塞进", "置入",
        "release", "drop",
    ]
    .iter()
    .any(|k| t.contains(k));
    // “拿起 + 放置目标”搬运语义：即使没说“放下”，也应要求 release 完成闭环。
    let implied_place = need_grab
        && ["到", "进", "入", "里", "内", "中", "袋", "口袋", "框", "盒", "箱"]
            .iter()
            .any(|k| t.contains(k));
    let need_release = explicit_release || implied_place;

    Some(RobotTaskNeed {
        need_camera_detect: ["找", "找到", "寻找", "看见", "识别", "杯子", "衣服"]
            .iter()
            .any(|k| t.contains(k)),
        need_move,
        need_grab,
        need_release,
        wheel_min,
    })
}

fn tool_progress_update(progress: &mut ToolProgress, tc: &ToolCall) {
    match tc.function.name.as_str() {
        "camera_capture" => progress.camera_count += 1,
        "object_detect" => progress.detect_count += 1,
        "wheel_move" => progress.wheel_count += 1,
        "arm_action" => {
            if let Ok(v) = serde_json::from_str::<Value>(&tc.function.arguments) {
                if let Some(raw) = v.get("action").and_then(|x| x.as_str()) {
                    match tools::classify_arm_action(raw) {
                        Some("grab") => progress.arm_grab_count += 1,
                        Some("release") => progress.arm_release_count += 1,
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

fn robot_task_satisfied(need: RobotTaskNeed, p: &ToolProgress) -> bool {
    if need.need_release && need.need_grab && need.need_camera_detect && need.need_move {
        let wmin = need.wheel_min.max(2);
        return p.camera_count >= 2
            && p.detect_count >= 2
            && p.wheel_count >= wmin
            && p.arm_grab_count >= 1
            && p.arm_release_count >= 1;
    }
    if need.need_camera_detect && (p.camera_count == 0 || p.detect_count == 0) {
        return false;
    }
    if need.need_move && p.wheel_count < need.wheel_min {
        return false;
    }
    if need.need_grab && p.arm_grab_count == 0 {
        return false;
    }
    if need.need_release && p.arm_release_count == 0 {
        return false;
    }
    true
}

fn missing_robot_steps(need: RobotTaskNeed, p: &ToolProgress) -> Vec<String> {
    let mut miss = Vec::new();
    if need.need_release && need.need_grab && need.need_camera_detect && need.need_move {
        if p.camera_count < 2 {
            miss.push(format!("camera_capture x2（当前 {}）", p.camera_count));
        }
        if p.detect_count < 2 {
            miss.push(format!("object_detect x2（当前 {}）", p.detect_count));
        }
        let wmin = need.wheel_min.max(2);
        if p.wheel_count < wmin {
            miss.push(format!(
                "wheel_move x{}（当前 {}）",
                wmin, p.wheel_count
            ));
        }
        if p.arm_grab_count < 1 {
            miss.push("arm_action grab x1".into());
        }
        if p.arm_release_count < 1 {
            miss.push("arm_action release x1".into());
        }
        return miss;
    }
    if need.need_camera_detect {
        if p.camera_count < 1 {
            miss.push("camera_capture x1".into());
        }
        if p.detect_count < 1 {
            miss.push("object_detect x1".into());
        }
    }
    if need.need_move && p.wheel_count < need.wheel_min {
        miss.push(format!(
            "wheel_move x{}（当前 {}）",
            need.wheel_min, p.wheel_count
        ));
    }
    if need.need_grab && p.arm_grab_count < 1 {
        miss.push("arm_action grab x1".into());
    }
    if need.need_release && p.arm_release_count < 1 {
        miss.push("arm_action release x1".into());
    }
    miss
}

async fn agent_turn(
    client: &Client,
    api_key: Option<&str>,
    user_text: &str,
    messages: &mut Vec<ChatMessage>,
) -> Result<String> {
    let defs = tools::openai_tool_definitions();
    messages.push(ChatMessage {
        role: "user".into(),
        content: Some(Value::String(user_text.to_string())),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    });

    let task_need = infer_robot_task_need(user_text);
    let max_rounds = if task_need.map(|n| n.wheel_min).unwrap_or(0) > 4 {
        24
    } else {
        8
    };
    let mut final_text = String::new();
    let mut tool_fallback = String::new();
    let mut tool_trace: Vec<String> = Vec::new();
    let tool_choice = if task_need.is_some() {
        Some(serde_json::json!("required"))
    } else {
        None
    };
    let mut progress = ToolProgress::default();
    let mut force_retry_count = 0usize;

    for _round in 0..max_rounds {
        let (assistant, text) = client
            .chat(api_key, messages.clone(), &defs, tool_choice.clone())
            .await
            .context("model request")?;

        let tool_calls = assistant.tool_calls.clone();
        let has_tool_calls = tool_calls
            .as_ref()
            .map(|tcs| !tcs.is_empty())
            .unwrap_or(false);

        // 关键修复：有 tool_calls 时 content 必须为 None（完全省略字段）
        let assistant_content = if has_tool_calls {
            None
        } else if let Some(ref s) = text {
            if !s.is_empty() {
                Some(Value::String(s.clone()))
            } else {
                None
            }
        } else {
            None
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
                tool_progress_update(&mut progress, &tc);
                let out = run_one_tool_call(&tc)?;
                let piece = out.to_tool_message_content();
                tool_trace.push(piece.trim_end().to_string());
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
            if let Some(need) = task_need {
                if !robot_task_satisfied(need, &progress) {
                    let missing = missing_robot_steps(need, &progress).join(", ");
                    messages.push(ChatMessage {
                        role: "user".into(),
                        content: Some(Value::String(
                            format!(
                                "Robot task still missing required steps: {}. Continue with tool calls only; do not output completion text yet.",
                                missing
                            ),
                        )),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                    });
                }
            }
            continue;
        }

        if let Some(t) = text {
            final_text = t;
        }
        if let Some(need) = task_need {
            if !robot_task_satisfied(need, &progress) && force_retry_count < 2 {
                force_retry_count += 1;
                let missing = missing_robot_steps(need, &progress).join(", ");
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: Some(Value::String(
                        format!(
                            "Robot task not finished yet. Missing steps: {}. Continue calling tools; do not output final completion text yet.",
                            missing
                        ),
                    )),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
                continue;
            }
        }
        break;
    }

    let trimmed = final_text.trim();
    if let Some(need) = task_need {
        if !robot_task_satisfied(need, &progress) {
            let missing = missing_robot_steps(need, &progress).join(", ");
            if !tool_fallback.is_empty() {
                let trace = if tool_trace.is_empty() {
                    String::new()
                } else {
                    format!("\n\n已执行命令记录：\n{}", tool_trace.join("\n"))
                };
                return Ok(format!(
                    "任务未完成：缺少步骤 {}。请继续调用工具执行，不要只给计划文本。\n\n最近一次工具输出：\n{}{}",
                    missing, tool_fallback, trace
                ));
            }
            return Ok(format!(
                "任务未完成：缺少步骤 {}。当前没有有效工具调用结果，请继续执行工具链。",
                missing
            ));
        }
    }
    if trimmed.is_empty() && !tool_fallback.is_empty() {
        if tool_trace.is_empty() {
            return Ok(tool_fallback);
        }
        return Ok(format!(
            "已执行命令记录：\n{}\n\n{}",
            tool_trace.join("\n"),
            tool_fallback
        ));
    }

    if task_need.is_some() && !tool_trace.is_empty() {
        return Ok(format!(
            "已执行命令记录：\n{}\n\n{}",
            tool_trace.join("\n"),
            final_text
        ));
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

    let api_key = std::env::var("STARRYCLAW_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .ok();

    let base = std::env::var("STARRYCLAW_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_OLLAMA_BASE.into());
    let model = std::env::var("STARRYCLAW_MODEL")
        .unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.into());

    let client = Client::new(base, model)?;
    let mut messages = vec![ChatMessage {
        role: "system".into(),
        content: Some(Value::String(SYSTEM_PROMPT.to_string())),
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
        print_input_prompt(Some(client.model()));
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

        match agent_turn(&client, api_key.as_deref(), line.trim(), &mut messages).await {
            Ok(reply) => {
                let reply = reply.trim();
                println!();
                if reply.is_empty() {
                    println!("（没有收到模型回复。可再说具体些；列目录/进目录/读文件/建目录/轮子前后左右/日期时间等可说明；其它需求模型可建议你在 shell 里试的命令。）");
                } else {
                    println!("{reply}");
                }
                println!();
            }
            Err(e) => {
                println!();
                eprintln!("error: {e:#}");
                println!();
            }
        }
    }

    Ok(())
}