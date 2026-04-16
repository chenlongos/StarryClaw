#!/usr/bin/env python3
"""
Single-file StarryClaw agent (OpenAI-compatible API + local tools).

Run:
  python3 starryclaw_agent.py

Env:
  STARRYCLAW_BASE_URL   default: http://127.0.0.1:11434/v1
  STARRYCLAW_MODEL      default: qwen2.5:7b-instruct
  STARRYCLAW_API_KEY    optional bearer token
"""

from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple
from urllib import request, error


DEFAULT_BASE_URL = "http://127.0.0.1:11434/v1"
DEFAULT_MODEL = "kimi-k2.5:cloud"   # 或换成 qwen2.5:7b
READ_MAX_BYTES = 256 * 1024


def tool_result(output: str = "", ok: bool = True, err: Optional[str] = None) -> str:
    if output and not output.endswith("\n"):
        output += "\n"
    if ok:
        return output + "[ok]"
    return output + f"[error] {err or 'unknown error'}"


def run_cmd(argv: List[str]) -> Tuple[bool, str, Optional[str]]:
    try:
        cp = subprocess.run(argv, text=True, capture_output=True, check=False)
        ok = cp.returncode == 0
        out = cp.stdout or ""
        err = None if ok else (cp.stderr.strip() or f"exit {cp.returncode}")
        return ok, out, err
    except Exception as e:
        return False, "", str(e)


def t_list_dir(args: Dict[str, Any]) -> str:
    p = (args.get("path") or ".").strip() or "."
    print(f"[TOOL] list_dir {p}", file=sys.stderr)
    ok, out, err = run_cmd(["ls", "-la", p])
    return tool_result(out, ok, err)


def t_mkdir(args: Dict[str, Any]) -> str:
    name = (args.get("name") or "").strip()
    if not name:
        return tool_result(ok=False, err='mkdir requires "name"')
    print(f"[TOOL] mkdir {name}", file=sys.stderr)
    ok, out, err = run_cmd(["mkdir", "-p", name])
    return tool_result(out, ok, err)


def t_change_dir(args: Dict[str, Any]) -> str:
    p = (args.get("path") or "").strip()
    if not p:
        return tool_result(ok=False, err='change_dir requires "path"')
    print(f"[TOOL] cd {p}", file=sys.stderr)
    try:
        os.chdir(p)
        return tool_result(f"cwd: {os.getcwd()}\n", True, None)
    except Exception as e:
        return tool_result(ok=False, err=str(e))


def t_read_file(args: Dict[str, Any]) -> str:
    p = (args.get("path") or "").strip()
    if not p:
        return tool_result(ok=False, err='read_file requires "path"')
    print(f"[TOOL] read_file {p}", file=sys.stderr)
    max_bytes = int(args.get("max_bytes") or READ_MAX_BYTES)
    max_bytes = min(max_bytes, 2 * 1024 * 1024)
    fp = Path(p)
    if not fp.is_file():
        return tool_result(ok=False, err="not a regular file")
    try:
        b = fp.read_bytes()
        if len(b) > max_bytes:
            return tool_result(ok=False, err=f"file too large ({len(b)} > {max_bytes})")
        return tool_result(b.decode("utf-8", "replace"), True, None)
    except Exception as e:
        return tool_result(ok=False, err=str(e))


ALLOWLIST = {
    "date", "uname", "pwd", "whoami", "hostname", "uptime", "cal", "df",
    "env", "which", "wc", "head", "tail", "id", "stat", "file", "readlink",
    "arch", "basename", "dirname", "echo", "groups", "nproc", "seq", "tty",
}


def t_run_shell(args: Dict[str, Any]) -> str:
    cmd = (args.get("command") or "").strip()
    if not cmd:
        return tool_result(ok=False, err='run_shell requires "command"')
    parts = shlex.split(cmd)
    if not parts:
        return tool_result(ok=False, err="empty command")
    prog = parts[0]
    if prog not in ALLOWLIST:
        return tool_result(ok=False, err=f"{prog} not allowlisted")
    print(f"[TOOL] run_shell {cmd}", file=sys.stderr)
    ok, out, err = run_cmd(parts)
    return tool_result(out, ok, err)


def _wheel_line(direction: str, distance: Optional[str]) -> str:
    m = {
        "forward": "向前",
        "backward": "向后",
        "left": "向左转",
        "right": "向右转",
    }
    base = m.get(direction, direction)
    return f"{base} {distance}".strip() if distance else base


def t_wheel_move(args: Dict[str, Any]) -> str:
    raw = (args.get("direction") or "").strip().lower()
    alias = {
        "前": "forward", "前进": "forward", "forward": "forward", "fwd": "forward",
        "后": "backward", "后退": "backward", "backward": "backward", "back": "backward",
        "左": "left", "左转": "left", "left": "left",
        "右": "right", "右转": "right", "right": "right",
    }
    d = alias.get(raw)
    if not d:
        return tool_result(ok=False, err="wheel_move direction invalid")
    dist = (args.get("distance") or "").strip() or None
    line = _wheel_line(d, dist)
    #print(f"[TOOL] wheel_move {d} {dist or ''}", file=sys.stderr)
    return f"【执行命令】 wheel -> {line}\nok"


def t_arm_action(args: Dict[str, Any]) -> str:
    raw = (args.get("action") or "").strip().lower()
    alias = {
        "grab": "抓取", "grip": "抓取", "pick": "抓取", "抓": "抓取", "抓取": "抓取",
        "release": "放下", "drop": "放下", "放": "放下", "放下": "放下", "松开": "放下",
    }
    act = alias.get(raw)
    if not act:
        return tool_result(ok=False, err="arm_action action invalid")
    #print(f"[TOOL] arm_action {act}", file=sys.stderr)
    return f"【执行命令】 arm -> {act}\nok"


def t_camera_capture(_: Dict[str, Any]) -> str:
    #print("[TOOL] camera_capture", file=sys.stderr)
    return "【执行命令】 camera -> 拍照\nok"


def t_object_detect(args: Dict[str, Any]) -> str:
    target = (args.get("target") or "目标").strip() or "目标"
    #print(f"[TOOL] object_detect {target}", file=sys.stderr)
    return f"【执行命令】 vision -> 识别 {target}\n结果: 距离=1.2m, 位置=左前\nok"


TOOL_RUNNERS = {
    "list_dir": t_list_dir,
    "mkdir": t_mkdir,
    "change_dir": t_change_dir,
    "read_file": t_read_file,
    "run_shell": t_run_shell,
    "wheel_move": t_wheel_move,
    "arm_action": t_arm_action,
    "camera_capture": t_camera_capture,
    "object_detect": t_object_detect,
}


def tool_defs() -> List[Dict[str, Any]]:
    return [
        {"type": "function", "function": {"name": "list_dir", "description": "List files in directory", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": []}}},
        {"type": "function", "function": {"name": "mkdir", "description": "Create directory", "parameters": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}}},
        {"type": "function", "function": {"name": "change_dir", "description": "Change cwd", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "read_file", "description": "Read file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "max_bytes": {"type": "integer"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "run_shell", "description": "Run allowlisted shell command", "parameters": {"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}}},
        {"type": "function", "function": {"name": "wheel_move", "description": "Wheel move: forward/backward/left/right (+ optional distance)", "parameters": {"type": "object", "properties": {"direction": {"type": "string"}, "distance": {"type": "string"}}, "required": ["direction"]}}},
        {"type": "function", "function": {"name": "arm_action", "description": "Arm action: grab/release", "parameters": {"type": "object", "properties": {"action": {"type": "string"}}, "required": ["action"]}}},
        {"type": "function", "function": {"name": "camera_capture", "description": "Take photo", "parameters": {"type": "object", "properties": {}, "required": []}}},
        {"type": "function", "function": {"name": "object_detect", "description": "Detect object and return distance/position", "parameters": {"type": "object", "properties": {"target": {"type": "string"}}, "required": []}}},
    ]


def post_chat(base_url: str, api_key: Optional[str], body: Dict[str, Any]) -> Dict[str, Any]:
    data = json.dumps(body).encode("utf-8")
    req = request.Request(
        url=base_url.rstrip("/") + "/chat/completions",
        method="POST",
        data=data,
        headers={"Content-Type": "application/json"},
    )
    if api_key:
        req.add_header("Authorization", f"Bearer {api_key}")
    try:
        with request.urlopen(req, timeout=120) as resp:
            return json.loads(resp.read().decode("utf-8", "replace"))
    except error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        raise RuntimeError(f"HTTP {e.code}: {body}") from e


def agent_turn(
    base_url: str,
    model: str,
    api_key: Optional[str],
    messages: List[Dict[str, Any]],
    user_text: str,
) -> str:
    # 添加用户消息
    messages.append({"role": "user", "content": f"User instruction:\n{user_text}"})
    defs = tool_defs()
    max_rounds = 8
    round_num = 0

    for _ in range(max_rounds):
        round_num += 1
        payload = {
            "model": model,
            "messages": messages,
            "tools": defs,
            "tool_choice": "required",
        }
        data = post_chat(base_url, api_key, payload)
        choice = (data.get("choices") or [{}])[0]
        msg = choice.get("message") or {}

        # 处理 assistant 消息，如果有 tool_calls 则 content 设为 None
        assistant_content = msg.get("content") if not msg.get("tool_calls") else None
        messages.append({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": msg.get("tool_calls"),
        })

        tcs = msg.get("tool_calls") or []
        if tcs:
            print(f"\n[Round {round_num}] Model requested {len(tcs)} tool call(s):", file=sys.stderr)
            for tc in tcs:
                fn = tc.get("function", {})
                name = fn.get("name", "")
                raw_args = fn.get("arguments", "{}")
                try:
                    args = json.loads(raw_args) if raw_args else {}
                except Exception:
                    args = {}
                print(f"  -> {name}({args})", file=sys.stderr)
                runner = TOOL_RUNNERS.get(name)
                out = runner(args) if runner else tool_result(ok=False, err=f"unknown tool: {name}")
                # 将工具结果添加到消息历史
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc.get("id"),
                    "content": out,
                })
                # 同时打印工具结果到 stderr（方便查看）
                print(f"  <- {out[:200]}", file=sys.stderr)
            # 继续循环，让模型处理工具结果
            continue

        # 没有 tool_calls，模型返回最终文本
        content = msg.get("content")
        if isinstance(content, str) and content.strip():
            return content.strip()
        return "（无模型文本回复）"

    return "（达到最大轮次，停止）"


def main() -> int:
    base_url = os.getenv("STARRYCLAW_BASE_URL", DEFAULT_BASE_URL)
    model = os.getenv("STARRYCLAW_MODEL", DEFAULT_MODEL)
    api_key = os.getenv("STARRYCLAW_API_KEY") or os.getenv("OPENAI_API_KEY")

    system = (
        "You are StarryClaw. For robot tasks, call tools first and only then summarize. "
        "Never claim task completion without tool outputs."
    )
    messages: List[Dict[str, Any]] = [{"role": "system", "content": system}]

    print(f"StarryClaw · {model} (base={base_url})")
    print("输入 quit/exit 退出。")
    while True:
        try:
            line = input("StarryClaw › ").strip()
        except EOFError:
            break
        if not line:
            continue
        if line in {"quit", "exit"}:
            break
        try:
            reply = agent_turn(base_url, model, api_key, messages, line)
            print()
            print(reply)
            print()
        except Exception as e:
            print()
            print(f"error: {e}")
            print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())