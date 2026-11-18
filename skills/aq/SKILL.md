---
name: aq
description: aq 排队：在本机跑编译或测试（xcodebuild、swift build/test、cargo build/test、gradle）、借 iOS/Android 模拟器或真机、借 agent-browser 浏览器或 agent-desktop 桌面会话、借测试账号，或要等待、找回自己提交的任务和设备时使用。用 aq run 代替直接执行和 pgrep/sleep 轮询，用 aq hold/release 代替自己开模拟器和浏览器会话。
---

# aq

这台 Mac 上同时有多个 agent 在编译、测试、用设备。aq 负责排队、合并重复的编译、分配设备和账号；所有人都经过它，机器才不会被压垮、设备才不会被抢。输出每个对象一行 JSON，按 `status` 决定下一步。命令写法以 `aq --help`、`aq <子命令> --help` 为准。

## 常用命令写成脚本

aq 按「代码指纹 + 执行目录 + 命令原文」判断是不是同一次执行，按「仓库 + 命令签名」积累资源历史。命令原文固定，别的 agent 跑同一件事才能共用结果，估算也越跑越准。

- 命令本身短而固定（如 `cargo test`）就直接用。
- 先用项目已有入口：`just`、`make`、`npm run` 等 → `aq run -- just test`。
- 没有就在仓库 `scripts/` 下建脚本，一个用途一个（如 `scripts/build.sh`、`scripts/test.sh`），设备 id 等变化的值从脚本参数传入。新建脚本提交前先问用户。
- 脚本放在被 git 跟踪或未忽略的位置：aq 的代码指纹只看这些文件，放在忽略目录或仓库外，改了脚本 aq 也察觉不到。
- 管道、`&&`、环境变量写进脚本里；`aq run` 不经过 shell。
- 改脚本后第一次运行会重新评估资源，属正常。

## 跑编译或测试

1. 在要执行的目录里提交：`aq run -- <脚本或命令> [参数...]`。
   - `run`、`hold` 默认最多等 100 秒（`--wait`；`status` 不加就不等），配合 Bash 默认 120 秒超时；你给 Bash 设了更长超时时，把 `--wait` 设成比它少 20 秒。
2. 看 `status`：
   - `queued` / `running`：`aq status <id> --wait <retry_after_s 或更长>` 接着等；也可以放后台跑。
   - `ok`：继续。出现 `content_changed: true`（只在为真时出现）表示运行期间仓库文件变了；命令自己生成的未忽略文件（如第一次生成的 `Cargo.lock`）也算，只有你的代码改了才需要重跑。
   - `failed`：读 `log` 里的输出修代码。`timeout`：看日志判断是卡住还是太慢。`interrupted`：环境问题，直接重试。
   - `rejected`：按 `reason` 改调用（见「出错时」）。
3. 同时跑几个：每个用 `--wait 0` 提交，再 `aq status <id1> <id2> --wait <秒>` 一起等。

这一步结束的标准：每个 id 都拿到终态（ok / failed / timeout / interrupted / cancelled / rejected）。还在 queued、running 时不下结论。

`reused: true` 表示别人正在跑同一份代码的同一条命令，你共用它的结果；需要单独跑时加 `--fresh`。不要的任务用 `aq cancel <id>`：运行中的会等到进程组结束再返回 `cancelled`（最多十几秒）；返回 `running` 且带 `cancel_requested: true` 时，稍后用 `aq status <id>` 读回终态。

## 借设备和账号

1. `aq hold <设备类型>... [--account <账号名>]...`，设备类型：`ios-sim`、`android-emu`、`ios-device`、`android-device`、`browser`、`desktop`；要几台写几次。`browser`、`desktop` 可以多个 agent 同时借，只按内存排队。整组一次借到，或整组排队。
   - `waiting`：`aq status <借用id> --wait <秒>` 等到 `held`。
   - 真机（`ios-device`、`android-device`）只借用户在 `~/.config/aq/config.toml` 的 `[ios-device]`、`[android-device]` 段 `ids` 里登记过、此刻连得上的（有线或同网无线都算）。没登记的手机可能是用户自用的，aq 不会借出；这个文件只由用户改，你不要改。
2. 只操作返回的 `devices[].id` 和 `accounts`：
   - xcodebuild 用 `-destination id=<id>`；simctl 用 `<id>`；adb 用 `-s <id>`。
   - browser：`agent-browser --session <id> ...`，aq 已经把这个会话的 Chrome 开好；desktop：`agent-desktop --session <id> ...`，保持默认无窗口模式。
   - 模拟器和浏览器会话由 aq 新建和删除，你只用它给的这些，不要自己 `close`。
3. 验证结束立即 `aq release <借用id>`，看到 `released` 才算这一步结束；模拟器和浏览器会话随后几秒内由 aq 在后台删除。借用者进程退出或闲置 2 小时也会被收回（`reclaimed`），那只是兜底。

## 找回和查看

- 上下文丢了：`aq status` 列出全机活动的执行和未归还的借用，第一行是忙闲概况。
- 要排查（在等什么资源、估算从哪来）：加 `--human`。

## 出错时

| 情况 | 怎么办 |
| --- | --- |
| `waiting_for` 含 `disk_full` | 磁盘不够，等运行中的任务结束也腾不出来。把 `message` 转告用户（还差多少 GB、需要清理磁盘）；不想等就 `aq cancel <id>`。不要绕过 aq 直接跑 |
| 带 `blocked_by` | 你要的资源被排头 `<id>` 预留着，排头开始后你就会跟着放行；想知道排头在等什么用 `aq status <排头id>` |
| `rejected` + `usage` | 写法不对（缺 `--`、设备类型拼错、id 不存在），改调用 |
| `rejected` + `unschedulable` | 需求超过整机容量，或真机比登记的多，永远等不到；告诉用户 |
| `rejected` + `device_unavailable` | 登记过的真机此刻连不上：把 `unavailable[].name` 和 `why` 告诉用户，请用户连上后再借 |
| 拒绝结果带 `undeclared`、`config_snippet` | 有已连接但没登记的真机：把它们的名字和 `config_snippet` 给用户，由用户决定登记哪台、删掉不想借出的再粘进 config.toml |
| `rejected` + `device_error` | 模拟器或浏览器创建失败，整组已回滚，可以重新 hold |
| `reason: busy`（退出码 1） | 数据库忙，稍后重试同一条命令 |
| 只有 `message`、没有 `reason` | 环境问题（目录不可写、沙箱拦截），把 message 里的路径告诉用户 |
| 找不到 aq，或 `aq --help` 里没有 run/hold | 告诉用户在 aq 仓库里跑 `just install` |

退出码：0 成功或已借到，1 失败，2 还在排队或运行，3 被拒绝。
