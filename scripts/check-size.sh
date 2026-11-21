#!/bin/bash
# 单文件不超过 400 行（函数 ≤ 60 行由 clippy too_many_lines 检查）。
set -euo pipefail
cd "$(dirname "$0")/.."
bad=0
while IFS= read -r f; do
  n=$(wc -l < "$f" | tr -d ' ')
  if [ "$n" -gt 400 ]; then echo "文件超过 400 行：${f}（${n} 行），按职责拆分"; bad=1; fi
done < <(find src crates tests -name '*.rs')
exit $bad
