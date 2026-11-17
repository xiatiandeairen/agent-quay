#!/bin/bash
# 红区检查（pre-commit 和 just check 都会跑）。
#  1. 迁移夹具 tests/storage/fixtures/** 与已发布迁移 crates/aq-store/migrations/** 只能新增，不能修改或删除。
#  2. 同一改动里既改了实现（src/、crates/*/src/），又修改或删除了验收断言（tests/acceptance/ 里删掉的 assert 行），要停下交给人确认。
# 人已确认时：AQ_GUARD_ACK=1 git commit …
# 比较范围：pre-commit 时看暂存区；否则看工作区相对 HEAD（没有提交时跳过）。
set -euo pipefail
cd "$(dirname "$0")/.."
git rev-parse --git-dir >/dev/null 2>&1 || { echo "guard: 不是 git 仓库，跳过"; exit 0; }
if [ "${GUARD_STAGED:-}" = 1 ]; then range=(--cached); else
  git rev-parse --verify HEAD >/dev/null 2>&1 || { echo "guard: 还没有提交，跳过"; exit 0; }
  range=(HEAD)
fi
fail=0
frozen=$(git diff "${range[@]}" --name-status -- tests/storage/fixtures crates/aq-store/migrations | awk '$1 != "A" {print $2}')
if [ -n "$frozen" ]; then
  echo "红区：迁移夹具或已发布迁移被修改或删除（只能新增新版本）："; echo "$frozen"; fail=1
fi
impl=$(git diff "${range[@]}" --name-only -- src crates | grep -v '/tests\?/' | grep -v '/tests.rs$' || true)
removed=$(git diff "${range[@]}" -U0 -- tests/acceptance | grep -E '^-[^-].*assert' || true)
if [ -n "$impl" ] && [ -n "$removed" ]; then
  echo "红区：同一改动里既改了实现，又修改或删除了验收断言，需要人确认："; echo "$removed"; fail=1
fi
if [ "$fail" = 1 ] && [ "${AQ_GUARD_ACK:-}" != 1 ]; then exit 1; fi
echo "guard: 通过"
