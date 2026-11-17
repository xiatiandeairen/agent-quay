#!/bin/bash
# 插件 SessionStart hook：本机没装 aq 时告诉 agent 怎么装，装了就什么都不说。
# 只读检查，不安装、不联网；aq 是按需使用的，不拦截任何命令。
command -v aq >/dev/null 2>&1 && exit 0
msg='The aq skill is enabled but the aq command is not installed on this Mac. Before using aq, tell the user to install it with: brew install xiatiandeairen/tap/aq'
printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$msg"
