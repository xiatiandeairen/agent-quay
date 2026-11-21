# aq（agent-quay）

一台 Mac 上 5–10 个 AI agent 共用的排队与资源管家：`aq run` 排队执行编译测试，`aq hold`/`aq release` 借还模拟器、真机和测试账号。没有常驻服务，所有 aq 进程共用一个 SQLite 账本协作。

**技术方案在 [`docs/design.md`](docs/design.md)，用例表在 [`docs/cases.md`](docs/cases.md)**，都只在仓库里维护。枚举值、字段名与 design.md 一一对应；改行为先改这两份文档并与用户对齐。

给 agent 的使用说明是 [`skills/aq/SKILL.md`](skills/aq/SKILL.md)，随插件发布。输出字段、退出码、命令写法变了要同步改它；它只写 agent 该做什么、会看到什么，不写实现细节（细节留在 design.md）。

## 职责边界

aq 负责的，出问题就改 aq：

| 职责 | 范围 |
| --- | --- |
| 不压垮机器 | 按重任务名额、内存（含系统内存压力）、磁盘底线、模拟器台数、下载名额放行；放不下就排队，并说明在等什么 |
| 复用结果 | 同一份代码、同一执行目录、同一命令原文、上一次还在排队或运行时，合并成一次。「同一份代码」只按 git 看得到的内容判断：已跟踪文件的改动、未忽略的新文件、登记过的子模块 |
| 估算资源 | 按「仓库 + 命令签名」的历史实测估（不在 git 仓库里时按执行目录）；估不准时靠运行中纠正和磁盘底线兜住 |
| 借还设备和账号 | 模拟器、Android 实例、浏览器和桌面会话由 aq 新建和删除；真机只借用户在 config.toml 登记过的；账号只保证同一时刻一个人用 |
| 找回和收尾 | 执行和借用的状态、日志、锁可随时查到；借用者退出、闲置过久或超过借用上限会收回；记录和文件按保留期清理 |

不归 aq 的，写进 design.md 的已知限制或转告用户，不改 aq 去绕：

| 不负责 | 遇到时怎么办 |
| --- | --- |
| git 看不到的内容：环境变量、仓库外或被忽略目录里的脚本、外层没登记的嵌套 git 仓、构建缓存 | 需要区分时调用方加 `--fresh`；嵌套仓由项目登记成子模块 |
| 估算的精度：同一仓库不同子目录的同一条命令共用历史 | 只会多排队，结果不受影响；不按目录拆分 |
| 项目自己的构建环境：架构、部署目标、DerivedData 位置、签名、依赖下载 | 由项目修；aq 只照原样执行命令 |
| aq 以外的程序占的内存和磁盘 | 只排队并说明原因，不去清理或结束别人的进程 |
| 用户的配置文件 `~/.config/aq/config.toml` | 只读，由用户改；真机登不登记由用户决定 |
| 账号的密码和登录 | aq 只管谁在用，不碰凭证 |

## 完成标准

改完跑 `just check`（fmt、clippy -D warnings、全部测试、红区、文件大小、用例表核对），全绿才算完成。只改了某一块时可以先跑 `cargo test --test acceptance -- <模块>::`，交付前仍要跑全量。

## 分层（编译器检查依赖方向）

| crate | 职责 | 不允许 |
| --- | --- | --- |
| `crates/aq-core` | 纯逻辑：状态、放行（`admit`）、检查（`check::plan`）、估算、签名与 hash、ports trait | 任何 IO、时间、进程 |
| `crates/aq-store` | SQLite 账本：迁移、事务、查询 | 业务判断 |
| `crates/aq-sys` | 外部世界：git 指纹、进程探测与脱离、flock、系统资源、simctl、emulator 和 adb、devicectl、agent-browser、agent-desktop、识别调用方 agent、AI CLI、目录、aq.log | 业务判断 |
| `src/`（aq） | `cli.rs` 解析；`app/` 用例编排：开事务 → 读快照 → core 算动作 → 执行 → 提交 | 绕过 core 自己做调度决定 |

runner 就是 aq 自己：`aq __runner <id>`（执行和借用共用），经 `aq __detach` 脱离 agent 会话；命令经 `aq __exec` 过启动闸门；`aq __simgc` 清理残留的 aq 模拟器。

## 测试分层与能不能改

| 类别 | 位置 | agent 能否修改 |
| --- | --- | --- |
| 验收用例（黑盒，按 aq 命令） | `tests/acceptance/<功能>.rs`；守哪条用例在 `#[test]` 上方注释标 `CASE-*` 编号，场景和预期在 `docs/cases.md` | 可以新增；**修改或删除已有断言要和实现分开提交，并交给人确认**（`scripts/guard.sh` 会拦） |
| 契约用例（真件、假件行为一致） | `tests/contracts/` | 可以改，真件和假件都要通过 |
| 单元用例 | aq-core 模块内的 `#[cfg(test)] mod tests`；放不下时拆到 `<模块>/tests.rs` | 随意改 |
| 存储与迁移夹具 | `tests/storage/`；`tests/storage/fixtures/vN/` | 夹具和 `crates/aq-store/migrations/` 里已有文件**不能改**（红区），结构变更只能加新迁移 |

验收用的假件通过环境变量注入：`AQ_SIMCTL`、`AQ_DEVICECTL`、`AQ_EMULATOR`、`AQ_ADB`、`AQ_AGENT_BROWSER`（假的外部工具）、`AQ_SYS_OVERRIDE`（系统资源）、`AQ_T_*`（缩短时间阈值）、`AQ_TEST_CRASH_AT`（崩溃注入），清单见 design.md「测试与验收」。新增或修改用例：先改 `docs/cases.md`，再在守它的测试上方标编号，跑 `just cases`（`just check` 也会跑）核对用例表和测试对得上；不守需求行为的集成测试标 `// 表外：<原因>`。

## 写代码的约定

- 业务代码不用 `unwrap`/`expect`/`panic`（clippy deny）；错误带上下文（id、路径）往上传；故意忽略的结果在同行注释写原因。
- 单文件 ≤ 400 行，单函数 ≤ 60 行；超了按职责拆。模块开头注释写明职责。
- 每次状态转移调用 `ctx.log_transition`（写 aq.log 一行 JSON）。改状态只走 `aq-store` 里带原状态条件的函数：数据库触发器只拦「终态再改」，running→queued 这类倒退要靠这些函数的原状态条件挡住。
- 系统调用只写在 `aq-sys`（aq 本体不依赖 libc）；`src/` 需要新的系统能力时，在 aq-sys 加函数再调用。
- 列出进行中的执行用两次等值查询（先 running 再 queued），不写 `status IN (...)`（会全表扫描）。
- 表结构只写在 `crates/aq-store/migrations/`，`docs/schema.sql` 是它的副本，有测试比对两者。

## 坑（已踩过）

- Claude Code 的进程名和可执行文件名都是版本号（如 `2.1.322`），只有 argv[0] 是 `claude`：识别借用者按 argv[0]（`aq-sys/src/procs.rs`）。
- xcodebuild 的编译服务 SWBBuildService 是它的子进程，但另开了进程组，swift/clang 都在它下面：按进程组采样 CPU 和内存都会漏掉编译进程，要按进程树（组成员 + 后代，`aq-sys/src/procs.rs` 的 `tree_members`）。
- `proc_pidinfo` 返回的 CPU 时间是 mach 时钟单位（Apple Silicon 上 1 单位 = 125/3 纳秒），要用 `mach_timebase_info` 换算。
- `simctl list runtimes` 里有只支持单一机型的运行时（如 `iOS-27-2-iPhone19-4`），要按 `supportedDeviceTypes` 选。
- 多个进程同时第一次建库会撞迁移（迁移库在事务外读版本）：`Store::open` 用库旁的文件锁排队。
- 测试里不能复制 `/bin/sh` 冒充进程：macOS 拒绝运行复制出的系统程序，还会卡住同时启动的进程；用符号链接。
- android-emu 用 `-read-only` 起实例；如果同一个 AVD 正被不带 `-read-only` 的模拟器开着，emulator 会拒绝，所以 config 里给 aq 配专用 AVD。aq 只用 5600–5680 端口。
- `scripts/guard.sh` 比较的是工作区和 HEAD，没有提交时直接跳过：红区只在有提交之后才生效，pre-commit 钩子要用 `ln -sf ../../scripts/pre-commit .git/hooks/pre-commit` 装上。
- 用 `aq run` 跑本仓库的测试时，测试进程继承了 aq 设的 nice 10，降不回去：`protect::nice_config_zero_and_out_of_range` 必然失败，直接 `cargo test` 才通过。其余测试不受影响。
- 整月回放这类 `--ignored` 长测试用 `just m3`（编译后直接运行测试程序）。
