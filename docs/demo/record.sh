#!/bin/bash
# Re-records the demo in an isolated aq ledger, then returns the borrowed simulators.
set -euo pipefail
export LANG=${LANG:-en_US.UTF-8}  # tmux draws non-ASCII (bars, …) as _ without a UTF-8 locale
here=$(cd "$(dirname "$0")" && pwd)
demo=/tmp/aq-demo
# Record with this checkout's aq, not whatever aq is installed.
(cd "$here/../.." && cargo build --release --quiet)
export AQ_BIN_DIR="$here/../../target/release"
"$here/setup.sh" "$demo"
tmux -L aqdemo kill-server 2>/dev/null || true
"$here/layout.sh" "$demo"
(cd "$here/../.." && vhs docs/demo/demo.tape)
tmux -L aqdemo kill-server 2>/dev/null || true
source "$demo/env"
aq status | jq -r 'select(.status == "held") | .id' | while read -r id; do aq release "$id" >/dev/null; done
