//! 认出调用方 agent，并查「已验证 agent 表」。
//! 环境变量识别交给 is-ai-agent；只有表里的 agent 才用它的 CLI 评估、按它的会话主进程认借用者。

use is_ai_agent::AgentId;

/// 已验证 agent 表的一行。
pub struct Verified {
    /// 会话主进程的 argv[0]（Claude Code 的进程名是版本号，只有 argv[0] 是 claude）。
    pub argv0: String,
    /// 关掉全部工具和 MCP 的非交互评估命令（CASE-EST-6）。
    pub eval: Vec<String>,
}

/// 按本进程的环境变量认出调用方 agent；认不出或不在已验证表里返回 None。
/// 测试用 AQ_T_AGENT_ARGV0 把会话主进程的 argv[0] 换成假进程名。
pub fn verified() -> Option<Verified> {
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let (argv0, eval) = match is_ai_agent::detect()?.id {
        AgentId::ClaudeCode => {
            ("claude", v(&["claude", "-p", "--tools", "", "--strict-mcp-config", "--no-session-persistence", "--output-format", "text"]))
        }
        AgentId::Codex => ("codex", v(&["codex", "exec", "--sandbox", "read-only", "--skip-git-repo-check", "-"])),
        _ => return None,
    };
    let argv0 = std::env::var("AQ_T_AGENT_ARGV0").unwrap_or_else(|_| argv0.into());
    Some(Verified { argv0, eval })
}
