# StarryClaw

**StarryClaw** 是为 [辰龙操作系统 **StarryOS**](https://github.com/Starry-OS/StarryOS) 准备的轻量级 **Agent（智能体）** 实验：在类 Unix 环境（含 StarryOS）里，用「对话 + 本地工具」的方式协助用户完成常见文件与系统操作，探索怎样让辰龙机器人操作系统上的交互更自然、更智能。

这不是替代完整机器人栈的「大脑」，而是一块可嵌入、可演进的拼图：先打通 **OpenAI 兼容的对话接口** 与 **受限、可审计的本地工具**，再逐步与 StarryOS 上的服务、策略与硬件能力对接。

## 能做什么

- **在线智能体（唯一模式）**：连接 **OpenAI 兼容** 的 Chat Completions API（开发时通常指向本机 **Ollama** `/v1`）。模型可发起 **function / tool calling**，由本程序在本地执行工具并把结果回传给模型，形成多轮推理。
- **无 Rust 二进制时的兜底**：仓库提供 `scripts/starryclaw-agent.sh`，在仅有 `/bin/sh` 的 StarryOS 镜像上也可做最简单的「查询 / 创建 / ls / mkdir / cd / cat」交互（与 Rust 版不同，不连模型）。

当前 Rust 版内置工具包括（名称以实际 schema 为准）：**列目录**、**建目录（单层名）**、**切换工作目录**、**读文本文件（有大小上限）**、**受限 shell（仅允许名单内只读类命令等）**。超出工具能力时，Agent 会尽量用自然语言建议你在终端里自行尝试的命令（只读、安全导向）。

## 为什么面向 StarryOS

StarryOS 是面向机器人与嵌入式场景的 OS 形态：在资源、网络与安全约束下，仍希望系统能「听得懂人话、办得成事」。StarryClaw 选择：

- **小依赖、可静态链接**（`Cargo.toml` 中 release 使用 `panic = "abort"`，便于与 musl 等场景配合）。
- **本地执行工具**，不把任意 shell 交给模型，降低误操作面。
- **明确区分虚拟机内外**：在 QEMU 里跑的 StarryOS 若要访问宿主机上的 Ollama，不能用 `localhost` 指宿主机，需通过环境变量配置可路由的地址（例如 QEMU user 网络下常见的宿主机 `10.0.2.2`）。

这些设计都是为了在 StarryOS 上长期迭代时，**安全边界清晰、部署路径简单**。

## 构建与运行

需要安装 [Rust](https://www.rust-lang.org/) 与 `cargo`。

```bash
cargo build --release
./target/release/starryclaw
```

### RISC-V 版本
```bash
cargo build --release --target riscv64gc-unknown-linux-musl
```
注意 这个编译(riscv64gc-unknown-linux-musl) 的名字不能变，如果改变了 .cargo/config.toml 要对应修改

开发调试：

```bash
cargo run
# 或优化版
cargo run --release
```

在 StarryOS 或任意 POSIX 环境仅用 shell：

```bash
sh scripts/starryclaw-agent.sh
```

## 环境变量

| 变量 | 含义 |
|------|------|
| `STARRYCLAW_BASE_URL` | API 根地址，需包含 `/v1`（如 `http://127.0.0.1:11434/v1`）。QEMU 内访问宿主机 Ollama 时可设为 `http://10.0.2.2:11434/v1` 等。 |
| `STARRYCLAW_MODEL` | 模型名（与 Ollama / 网关一致）。 |
| `STARRYCLAW_API_KEY` / `OPENAI_API_KEY` | 需要 Bearer 鉴权时设置（Ollama 本地常可不设）。 |
| `NO_COLOR` / `STARRYCLAW_NO_COLOR` | 关闭提示符 ANSI 颜色。 |

默认的 `STARRYCLAW_BASE_URL` / `STARRYCLAW_MODEL` 以源码中常量为准；**部署到不同机器时请用环境变量覆盖**，勿硬编码在业务脚本里。

## 交互说明

- 启动后在提示符输入问题或指令，**回车发送**。
- 输入 `quit` 或 `exit` 退出。

## 许可与状态

本项目处于早期实验阶段，API 与工具集合可能随 StarryOS 场景继续扩展。具体许可证以仓库内声明为准（若尚未添加，以后续 `LICENSE` 文件为准）。

---

**StarryClaw** — 为辰龙 **StarryOS** 而生的 Agent 尝试，目标是把机器人操作系统上的日常操作与推理做得更顺手、更可控。
