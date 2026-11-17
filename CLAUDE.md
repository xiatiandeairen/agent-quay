# aq（agent-quay）

一台 Mac 上 5–10 个 AI agent 共用的排队与资源管家：`aq run` 排队执行编译测试，`aq hold`/`aq release` 借还模拟器、真机和测试账号。没有常驻服务，所有 aq 进程共用一个 SQLite 账本协作。

**技术方案在 [`docs/design.md`](docs/design.md)，用例表在 [`docs/cases.md`](docs/cases.md)**，都只在仓库里维护。枚举值、字段名与 design.md 一一对应；改行为先改这两份文档并与用户对齐。

## 完成标准

改完跑 `just check`（fmt、clippy -D warnings、全部测试、红区、文件大小、用例表核对），全绿才算完成。只改了某一块时可以先跑 `cargo test --test acceptance -- <模块>::`，交付前仍要跑全量。

## 分层（编译器检查依赖方向）

| crate | 职责 | 不允许 |
| --- | --- | --- |
| `crates/aq-core` | 纯逻辑：状态、放行（`admit`）、检查（`check::plan`）、估算、签名与 hash、ports trait | 任何 IO、时间、进程 |
| `crates/aq-store` | SQLite 账本：迁移、事务、查询 | 业务判断 |
| `crates/aq-sys` | 外部世界：git 指纹、进程探测与脱离、flock、系统资源、simctl、AI CLI、目录、aq.log | 业务判断 |
| `src/`（aq） | `cli.rs` 解析；`app/` 用例编排：开事务 → 读快照 → core 算动作 → 执行 → 提交 | 绕过 core 自己做调度决定 |

runner 就是 aq 自己：`aq __runner <id>`（执行和借用共用），经 `aq __detach` 脱离 agent 会话；命令经 `aq __exec` 过启动闸门。

## 测试分层与能不能改

| 类别 | 位置 | agent 能否修改 |
| --- | --- | --- |
| 验收用例（黑盒，按 aq 命令） | `tests/acceptance/<功能>.rs`；守哪条用例在 `#[test]` 上方注释标 `CASE-*` 编号，场景和预期在 `docs/cases.md` | 可以新增；**修改或删除已有断言要和实现分开提交，并交给人确认**（`scripts/guard.sh` 会拦） |
| 契约用例（真件、假件行为一致） | `tests/contracts/` | 可以改，真件和假件都要通过 |
| 单元用例 | aq-core 模块内的 `#[cfg(test)] mod tests`；放不下时拆到 `<模块>/tests.rs` | 随意改 |
| 存储与迁移夹具 | `tests/storage/`；`tests/storage/fixtures/vN/` | 夹具和 `crates/aq-store/migrations/` 里已有文件**不能改**（红区），结构变更只能加新迁移 |

验收用的假件通过环境变量注入：`AQ_SIMCTL`（假 simctl）、`AQ_SYS_OVERRIDE`（系统资源）、`AQ_T_*`（缩短时间阈值）、`AQ_TEST_CRASH_AT`（崩溃注入），清单见 design.md「测试与验收」。新增或修改用例：先改 `docs/cases.md`，再在守它的测试上方标编号，跑 `just cases`（`just check` 也会跑）核对用例表和测试对得上；不守需求行为的集成测试标 `// 表外：<原因>`。

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
- demo-ios 的 App 实机演练：xcodebuild 要加 `ARCHS=arm64`，`-derivedDataPath` 放在仓库外；部分依赖还要 `IPHONEOS_DEPLOYMENT_TARGET=15.0`。这些是项目构建环境的问题，不要改 aq 去绕。
- 本机 cargo 的测试如果经全机统一排队的 test runner 运行（`CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER`）：后台在跑 `--ignored` 的长测试（如整月回放）时，`cargo test`、`just check` 会一直等，换 `CARGO_TARGET_DIR` 也没用。长测试用 `just m3`（编译后直接运行测试程序，不经 runner）。
