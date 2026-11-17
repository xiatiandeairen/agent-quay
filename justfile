# 统一检查入口：格式、lint、单元、契约、存储、验收、红区、文件大小、文档用例核对。改完代码跑 `just check`，全绿才算完成。
check: fmt clippy test guard size cases

fmt:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

# 红区：迁移夹具、已发布迁移不能改；同一改动里既动实现又删改验收断言要人确认
guard:
    scripts/guard.sh

size:
    scripts/check-size.sh

# 发版前实机演练用：真 simctl 建删一台模拟器（约 30–45 秒）
drill-sim:
    cargo test --test contracts simulator_real_xcrun -- --ignored

# 长测试（M3：30 天记录按 1000 倍速回放，约 45 分钟）。先编译再直接运行测试程序：全机统一排队的 test runner（CARGO_TARGET_*_RUNNER），
# 经它跑会让同时进行的 just check 一直等；换构建目录也躲不开
m3:
    #!/usr/bin/env bash
    set -euo pipefail
    bin=$(cargo test --test acceptance --no-run --message-format=json | jq -r 'select(.target.name == "acceptance" and .executable != null) | .executable')
    "$bin" replay_full_month_at_1000x --ignored

# 核对用例表 docs/cases.md 与测试是否对得上
cases:
    scripts/check-cases.py

# 安装到 ~/.local/bin/aq（会覆盖旧版 aq）
install:
    cargo install --path . --root ~/.local --locked --force
