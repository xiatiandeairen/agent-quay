# aq（agent-quay）

[English](README.md) · 简体中文

给同一台 Mac 上并行工作的多个 AI agent 会话用的本机排队和设备借还工具。

多个 agent 会话（Claude Code、Codex 等）在同一台机器上干活时，各自决定什么时候编译、跑测试、开模拟器，结果一起挤上来，机器被拖垮，模拟器和测试账号互相覆盖。`aq` 是 agent 改用的一条命令：

| 问题 | aq 怎么做 |
| --- | --- |
| 几个编译同时跑，内存换页拖垮机器 | 按剩余内存和磁盘放行重任务，默认同时最多 2 个 |
| 抢同一台模拟器、真机或测试账号 | 一次只借给一个会话；模拟器每次新建，归还即删 |
| 同一份代码、同一条命令跑两遍 | 正在跑的直接共用，两边拿到同一个结果 |
| agent 写循环反复查结果 | 一条命令等结果，返回结构化状态 |

aq 按需使用：只调度经它提交的任务，不拦截任何其他命令。

## 环境要求

- macOS（在 Apple Silicon 上测试过）
- 借 `ios-sim` 需要 Xcode；借 `android-emu` 需要 Android SDK 模拟器（可选）
- 借 `browser` / `desktop` 时才需要 `agent-browser` / `agent-desktop`

## 安装

```bash
brew install xiatiandeairen/tap/aq
```

从源码安装（Rust 1.89 及以上）：

```bash
cargo install --git https://github.com/xiatiandeairen/agent-quay --locked
```

### Claude Code 插件

插件给每个会话加上 `aq` skill，让 agent 知道什么时候用、怎么用；本机没装 `aq` 时会告诉 agent 怎么装。

```text
/plugin marketplace add xiatiandeairen/agent-quay
/plugin install agent-quay@agent-quay
```

插件包含：

- `aq` skill（`skills/aq/SKILL.md`）：告诉 agent 什么时候、怎么调用 `aq` 的说明，本身不运行任何东西。
- 一个 SessionStart hook（`scripts/session-start.sh`）：检查 `PATH` 里有没有 `aq`；没有时往会话上下文加一行，让 agent 提醒用户运行 `brew install xiatiandeairen/tap/aq`。不安装任何东西，不改设置，不联网。

插件不带 `aq` 程序本身，也没有 MCP server，`aq` 按上面的方式另装。

## 用法

```bash
aq run -- xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build
aq status <id> --wait 60            # 接着等排队中或运行中的任务
aq hold ios-sim --account alice     # 借一台新模拟器和一个测试账号
aq release <借用id>                 # 归还
aq status --human                   # 全机情况，给人看
```

每条命令每个对象输出一行 JSON，按 `status`（`queued`、`running`、`ok`、`failed`、`held`、`rejected` 等）决定下一步。加 `--human` 看细节：任务在等什么、资源估算从哪来。

`hold` 的设备类型：`ios-sim`、`android-emu`、`ios-device`、`android-device`、`browser`、`desktop`。

## 配置

可选。`~/.config/aq/config.toml`（或 `$XDG_CONFIG_HOME/aq/config.toml`）：

```toml
heavy_slots = 2          # 同时最多几个重任务
mem = "28G"              # aq 可分配的内存
disk_floor = "50G"       # 磁盘至少保留这么多
ai_cli = "auto"          # 或写死一条命令，如 "claude -p"

[ios-sim]
model = "iPhone 17 Pro"

[ios-device]
ids = ["00008150-…"]     # 只借这里登记的真机
```

## 原理

- 没有常驻服务：所有 `aq` 进程共用 `~/.local/state/aq/` 下的一个 SQLite 账本。
- 资源需求来自同一仓库里同一条命令的历史实测。第一次遇到的命令，如果调用方 agent 在已验证名单里（目前是 Claude Code 和 Codex），就请它自己的 CLI 估一次；否则用保守默认值。
- 借出的设备在借用的 agent 会话退出后收回；认不出会话时，闲置 2 小时后收回。

## aq 会运行什么

aq 只运行 agent 让它运行的东西：

- `aq run`：轮到时在独立的进程会话里运行给定的命令。
- 代码指纹：在执行目录运行只读的 `git` 命令（`rev-parse`、`diff HEAD`、`ls-files`、`submodule status`），用来判断能不能共用同一次执行。
- `aq hold`：用 `xcrun simctl` 新建、启动、删除模拟器；用 Android SDK 的 `emulator` 和 `adb` 管理安卓模拟器；用只读的 `xcrun devicectl list devices` 和 `adb devices -l` 查找真机；用 `agent-browser` 开关浏览器会话。借 `desktop` 只发一个会话名。
- 第一次遇到某条命令：调用 agent 自己的 CLI 估算资源（见"隐私"）。

## 隐私

aq 在本机运行，不上报任何数据。唯一的对外请求是上面说的首次评估：aq 用 agent 自己的 CLI（如 `claude -p`）发送归一化后的命令、执行目录路径、仓库按扩展名统计的文件数量和大小，以及本机内存容量，走该 CLI 自己的账号和数据政策。把 `ai_cli` 写成固定命令，或使用不在已验证名单里的 agent，可以改变或避免这一点。

## 许可证

可任选 [Apache License 2.0](LICENSE-APACHE) 或 [MIT license](LICENSE-MIT)。
