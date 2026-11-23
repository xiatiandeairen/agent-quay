---
name: aq
description: aq 排队：在本机跑编译或测试（xcodebuild、swift build/test、cargo build/test、gradle）、借 iOS/Android 模拟器或真机、借 agent-browser 浏览器或 agent-desktop 桌面会话、借测试账号，或要等待、找回自己提交的任务和设备时使用。用 aq run 代替直接执行和 pgrep/sleep 轮询，用 aq hold/release 代替自己开模拟器和浏览器会话。
---

# aq

这台 Mac 上同时有多个 agent 在编译、测试、用设备，都经过 aq 排队和分配，机器才不会被压垮、设备才不会被抢。输出每个对象一行 JSON，按 `status` 决定下一步。命令写法以 `aq --help`、`aq <子命令> --help` 为准。

## 常用命令写成脚本

命令原文固定，别的 agent 跑同一件事时才能共用结果。

- 命令本身短而固定（如 `cargo test`）就直接用。
- 先用项目已有入口：`just`、`make`、`npm run` 等 → `aq run -- just test`。
- 没有就在仓库 `scripts/` 下建脚本，一个用途一个（如 `scripts/build.sh`、`scripts/test.sh`），设备 id 等变化的值从脚本参数传入。新建脚本提交前先问用户。
- 脚本放在仓库里被 git 跟踪或未忽略的位置；放在忽略目录或仓库外，改了脚本后可能拿到别人用旧脚本跑出的结果。
- 管道、`&&`、环境变量写进脚本里；`aq run` 不经过 shell。

## 跑编译或测试

1. 在要执行的目录里提交：`aq run -- <脚本或命令> [参数...]`。
   - `run`、`hold` 默认最多等 100 秒（`--wait`；`status` 不加就不等），配合 Bash 默认 120 秒超时；你给 Bash 设了更长超时时，把 `--wait` 设成比它少 20 秒。
2. 看 `status`：
   - `queued` / `running`：执行返回里的 `next` 命令接着等；也可以放后台跑。
   - `ok`：继续。带 `content_changed: true` 时，结果可能不对应最新代码：只有你的代码在运行期间改过才需要重跑。
   - `failed`：`exit_code` 是命令自己的退出码，读 `log` 里的输出修代码。`timeout`：看日志判断是卡住还是太慢。`interrupted`：环境问题，直接重试。
   - `rejected`：按 `reason` 或退出码处理（见「输出和退出码」）。
3. 同时跑几个：每个用 `--wait 0` 提交，再 `aq status <id1> <id2> --wait <秒>` 一起等。

这一步结束的标准：每个 id 都拿到终态（ok / failed / timeout / interrupted / cancelled / rejected）。还在 queued、running 时不下结论。

`reused: true` 表示别人正在跑同一份代码的同一条命令，你共用它的结果；需要单独跑时加 `--fresh`。不要的任务用 `aq cancel <id>`；返回 `running` 且带 `cancel_requested: true` 时，稍后用 `aq status <id>` 读回终态。

## 借设备和账号

1. `aq hold <设备类型>... [--account <账号名>]...`，设备类型：`ios-sim`、`android-emu`、`ios-device`、`android-device`、`browser`、`desktop`；要几台写几次。整组一次借到，或整组排队。
   - `waiting`：`aq status <借用id> --wait <秒>` 等到 `held`。
   - 真机（`ios-device`、`android-device`）只借用户在 `~/.config/aq/config.toml` 的 `[ios-device]`、`[android-device]` 段 `ids` 里登记过、此刻连得上的（有线或同网无线都算）。没登记的手机可能是用户自用的，aq 不会借出；这个文件只由用户改，你不要改。
2. 只操作返回的 `devices[].id` 和 `accounts`：
   - xcodebuild 用 `-destination id=<id>`；simctl 用 `<id>`；adb 用 `-s <id>`。
   - browser：`agent-browser --session <id> ...`，aq 已经把这个会话的 Chrome 开好；desktop：`agent-desktop --session <id> ...`，保持默认无窗口模式。
   - 模拟器和浏览器会话由 aq 新建和删除，你只用它给的这些，不要自己删除模拟器、关闭这些浏览器会话。
3. 验证结束立即 `aq release <借用id>`，看到 `released` 才算这一步结束。没还的设备 aq 也会收回（`reclaimed`），那只是兜底，不能代替 release。

## 找回和查看

- 上下文丢了：`aq status` 列出全机活动的执行和未归还的借用，第一行是忙闲概况。
- 要看在等什么、被谁挡住：加 `--human`。

## 输出和退出码

`status` 的终态怎么处理见上面两节；下面是等待和拒绝时要看的字段。

| 输出 | 怎么办 |
| --- | --- |
| `waiting_for` 含 `disk_full` | 磁盘清理之前不会开始。把 `message` 转告用户（还差多少 GB、需要清理磁盘）；不想等就 `aq cancel <id>`。不要绕过 aq 直接跑 |
| `waiting_for` 含 `mem_pressure` | 机器内存吃紧，压力回落后自动开始；`mem_pressure_s` 是已持续的秒数，持续太久会带 `message`：压力来自 aq 以外的程序，转告用户 |
| 带 `blocked_by` | 你要的资源被排在前面的 `<id>` 占着，它开始后就轮到你，接着等即可 |
| `rejected` + `usage` | 写法不对（缺 `--`、设备类型拼错、id 不存在），改调用 |
| `rejected` + `unschedulable` | 需求超过整机容量，或真机比登记的多，永远等不到；告诉用户 |
| `rejected` + `device_unavailable` | 登记过的真机此刻连不上：把 `unavailable[].name` 和 `why` 告诉用户，请用户连上后再借 |
| 拒绝结果带 `undeclared`、`config_snippet` | 有已连接但没登记的真机：把它们的名字和 `config_snippet` 给用户，由用户决定登记哪台、删掉不想借出的再粘进 config.toml |
| 查借用时带 `missing` | 列出的设备已被 aq 以外删掉（如模拟器服务被重置）；借用还在，`aq release` 后重新借 |
| `rejected` + `device_error` | 模拟器或浏览器创建失败，这次什么都没借到，可以重新 hold |
| `reason: busy` | aq 暂时忙，什么都没改，稍后重试同一条命令 |

| 退出码 | 下一步 |
| --- | --- |
| 0 | 完成，继续 |
| 1 | 命令失败：读 `log` 修代码 |
| 2 | 还没完：`aq status <id> --wait` 接着等 |
| 3 | 写法错：改调用 |
| 4 | 原样重试：重新提交或重新借 |
| 5 | 需要人：把 `message` 转告用户 |

多个 id 一起查时，退出码取最需要处理的那个；每个 id 的情况看各自那行的 `status`。

## 出错时

| 现象 | 怎么办 |
| --- | --- |
| 只有 `message`、没有 `reason` | 环境问题（目录不可写、沙箱拦截），把 message 里的路径告诉用户 |
| 找不到 aq，或 `aq --help` 里没有 run/hold | 告诉用户在 aq 仓库里跑 `just install` |
