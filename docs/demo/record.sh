#!/bin/bash
# Re-records the demo in an isolated aq ledger, returns the borrowed simulators,
# then adds the camera moves: docs/demo/demo.gif and demo.mp4.
# Needs vhs, tmux, ffmpeg and python3. Set AQ_DEMO_SKIP_RECORD=1 to only redo the camera moves
# from the last raw recording (/tmp/aq-demo/raw.mp4).
set -euo pipefail
export LANG=${LANG:-en_US.UTF-8}  # tmux draws non-ASCII (bars, …) as _ without a UTF-8 locale
here=$(cd "$(dirname "$0")" && pwd)
demo=/tmp/aq-demo
if [[ -z ${AQ_DEMO_SKIP_RECORD:-} ]]; then
  # Record with this checkout's aq, not whatever aq is installed.
  (cd "$here/../.." && cargo build --release --quiet)
  export AQ_BIN_DIR="$here/../../target/release"
  "$here/setup.sh" "$demo"
  # Background around the terminal window, in the logo's colors.
  ffmpeg -loglevel error -y -f lavfi \
    -i "gradients=s=2880x1620:c0=0x1f3b8f:c1=0x1596b8:x0=0:y0=0:x1=2880:y1=1620:nb_colors=2" \
    -frames:v 1 "$demo/bg.png"
  tmux -L aqdemo kill-server 2>/dev/null || true
  "$here/layout.sh" "$demo"
  (cd "$here/../.." && vhs docs/demo/demo.tape)
  tmux -L aqdemo kill-server 2>/dev/null || true
  source "$demo/env"
  aq status | jq -r 'select(.status == "held") | .id' | while read -r id; do aq release "$id" >/dev/null; done
fi
python3 "$here/zoom.py" "$demo/raw.mp4" "$here"
