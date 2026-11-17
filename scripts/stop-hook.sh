#!/bin/bash
# Claude Code Stop hook（G6、RW10）：本会话还借着设备、且闲置超过 5 分钟时提醒一次 aq release，不反复拦。
# 安装（settings.json）：{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"<仓库>/scripts/stop-hook.sh"}]}]}}
# AQ_STOP_IDLE_MIN 改闲置阈值（分钟，默认 5，测试用 0）。
# 判断依据：holds 里 status='held'、借用者是本会话的 claude 进程、holds/<id>.used 超过 5 分钟没更新；提醒过的记 <id>.reminded。
set -u
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/aq"
DB="$STATE/aq.db"
[ -f "$DB" ] || exit 0
# 本会话的 claude 进程：沿父进程链找 argv[0] 为 claude 的进程
pid=$PPID; owner=""
for _ in $(seq 1 20); do
  [ -z "$pid" ] || [ "$pid" -le 1 ] && break
  name=$(ps -o comm= -p "$pid" 2>/dev/null | awk -F/ '{print $NF}')
  if [ "$name" = claude ]; then owner=$pid; break; fi
  pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
done
[ -n "$owner" ] || exit 0
now=$(date +%s)
msg=""
for id in $(sqlite3 -readonly "$DB" "SELECT id FROM holds WHERE status='held' AND owner_pid=$owner" 2>/dev/null); do
  used="$STATE/holds/$id.used"
  [ -e "$STATE/holds/$id.reminded" ] && continue
  last=$(stat -f %m "$used" 2>/dev/null || echo "$now")
  idle=$(( (now - last) / 60 ))
  if [ "$idle" -ge "${AQ_STOP_IDLE_MIN:-5}" ]; then
    touch "$STATE/holds/$id.reminded"
    msg="$msg 借用 ${id} 已闲置 ${idle} 分钟；验证结束请执行 aq release ${id}。"
  fi
done
[ -n "$msg" ] || exit 0
# 只提醒一次：这一次让 agent 看到原因再结束，之后不再拦
printf '{"decision":"block","reason":"%s"}\n' "${msg# }"
