# aq（agent-quay）

一台 Mac 上多个 AI agent 共用的排队与借还工具。权威文档：[docs/design.md](docs/design.md)（目标、职责边界、取舍、依据、局限）、[docs/cases.md](docs/cases.md)（行为清单）、[skills/aq/SKILL.md](skills/aq/SKILL.md)（给外部 agent 的用法）。

## 完成标准

`just check` 全绿才算完成。只改一块时先跑 `cargo test --test acceptance -- <模块>::`，交付前仍跑全量。

## 硬约束

- 修复和分析只针对 design.md §1.2「aq 负责」里的事；范围外的写进 §5.1 已知局限或转告用户（无检查）。
- `aq-core` 不做 IO、不读时间、不起进程，要用就加 `ports` 里的 trait，由 `aq-sys` 实现（无检查；crate 之间的依赖方向由 Cargo 兜底）。
- 业务判断放在 `aq-core`，`aq-store`、`aq-sys` 只做存取和外部调用（无检查）。
- 每次状态变化调用 `ctx.log_transition`（无检查）。
- 改状态只走 `aq-store` 里带原状态条件的函数（数据库触发器只兜「终态再改」，倒退无检查）。
- 列出进行中的执行用两次等值查询，原因见 design.md §3.5（无检查）。
- 对外命令、输出、退出码变了，同一个提交里改 `skills/aq/SKILL.md`（无检查）。
- 新增用例先在 `docs/cases.md` 加一行，再在守它的测试上方标 `CASE-*` 编号，不守需求行为的测试标 `// 表外：<原因>`（由 `just cases` 兜底）。
- 不用 `unwrap`/`expect`/`panic`，文件不超过 400 行、函数不超过 60 行（由 clippy 和 `scripts/check-size.sh` 兜底）。
- 已发布的迁移和迁移夹具只增不改，结构变更加新迁移（由 `scripts/guard.sh` 兜底）。

## 先问人

- 同一改动里既改实现又改已有验收断言时，断言改动单独提交，交给人确认（由 `scripts/guard.sh` 兜底）。

## 做什么读什么

| 任务 | 读 |
| --- | --- |
| 判断一个问题归不归 aq、修 bug 之前 | design.md §1.2、§1.4 |
| 改机制，或想知道某个规则为什么这样定 | design.md §2、§3 |
| 改对外命令、输出、退出码 | skills/aq/SKILL.md |
| 加或改用例 | docs/cases.md |
| 排查卡住、没复用、被收回 | design.md §4 |
| 本机换成当前代码的 aq | `just install` |

## 坑

- 刚 `git init` 或还没提交时：guard 直接跳过，红区不生效；新检出要用 `ln -sf ../../scripts/pre-commit .git/hooks/pre-commit` 装上钩子。
- 用 `aq run` 跑本仓库测试时：测试进程继承了 nice 10、降不回去，`protect::nice_config_zero_and_out_of_range` 必然失败；验它就直接 `cargo test`。
- 机器忙时跑全量：按 CPU 和计时判断的测试（`reclass::`、`realworld::` 等）会失败，单独重跑再下结论。
