#!/bin/bash
# Builds the demo's 2x2 tmux layout: three "agents" and a live aq view. Run after setup.sh.
demo=${1:-/tmp/aq-demo}
here=$(cd "$(dirname "$0")" && pwd)
init="source $demo/env; cd $demo/app; export PS1='\$ ' BASH_SILENCE_DEPRECATION_WARNING=1; clear"
tmux -u -f /dev/null -L aqdemo new-session -d -s demo -x 200 -y 50 "bash --norc"
tmux -L aqdemo set -g status off \; set -g pane-border-status top \; set -g pane-border-format ' #{pane_title} ' \
  \; set -g pane-active-border-style fg=colour39 \; set -g mouse off
a=$(tmux -L aqdemo display -p '#{pane_id}')
c=$(tmux -L aqdemo split-window -h -t "$a" -P -F '#{pane_id}' "bash --norc")
b=$(tmux -L aqdemo split-window -v -t "$a" -P -F '#{pane_id}' "bash --norc")
w=$(tmux -L aqdemo split-window -v -t "$c" -P -F '#{pane_id}' "bash --norc")
for p in $a $b $c; do tmux -L aqdemo send-keys -t "$p" "$init" Enter; done
tmux -L aqdemo send-keys -t "$w" "source $demo/env; clear; aq status --top" Enter
tmux -L aqdemo select-pane -t "$a" -T "agent A" \; select-pane -t "$b" -T "agent B" \
  \; select-pane -t "$c" -T "agent C" \; select-pane -t "$w" -T "aq status --top" \; select-pane -t "$a"
# Prefix (Ctrl+B) then a/b/c jumps between the agent panes while recording.
tmux -L aqdemo bind a select-pane -t "$a" \; bind b select-pane -t "$b" \; bind c select-pane -t "$c"
