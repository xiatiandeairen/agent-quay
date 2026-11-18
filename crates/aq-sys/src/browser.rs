//! 浏览器：一个 agent-browser 会话（独立的无窗口 Chrome，cookie、存储与其他会话隔离）。
//! 借时用会话名 aq-<账本标识>-<借用id>-<序号> 打开 about:blank，让 Chrome 真正起来（约 1.5–2 秒），设备 id 就是会话名；
//! agent 之后用 `agent-browser --session <id> ...` 操作。还时 `close` 关掉这个会话的 Chrome。
//! 验收测试用 AQ_AGENT_BROWSER 指向假脚本。

use std::time::Duration;

use aq_core::ports::Simulator;
use serde::Deserialize;

use crate::cmd::{ok_text, Call};

pub struct AgentBrowser {
    pub cmd: Vec<String>,
}

impl AgentBrowser {
    pub fn from_env() -> AgentBrowser {
        let cmd = std::env::var("AQ_AGENT_BROWSER").unwrap_or_else(|_| "agent-browser".into());
        AgentBrowser { cmd: vec![cmd] }
    }

    fn call(&self, args: &[&str], secs: u64) -> Result<String, String> {
        let mut argv = self.cmd.clone();
        argv.extend(args.iter().map(|s| s.to_string()));
        ok_text(&Call { argv: &argv, cwd: None, env: &[], stdin: None, timeout: Duration::from_secs(secs) })
    }

    /// agent-browser 当前所有活动会话名（含用户和其他工具开的）。
    fn sessions(&self) -> Result<Vec<String>, String> {
        #[derive(Deserialize)]
        struct Data {
            sessions: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Out {
            data: Data,
        }
        let text = self.call(&["session", "list", "--json"], 30)?;
        let o: Out = serde_json::from_str(&text).map_err(|e| format!("解析 agent-browser 会话列表失败：{e}"))?;
        Ok(o.data.sessions)
    }
}

impl Simulator for AgentBrowser {
    fn create(&self, name: &str) -> Result<String, String> {
        if let Err(e) = self.call(&["--session", name, "open", "about:blank"], 120) {
            let _ = self.call(&["--session", name, "close"], 30); // 起了一半的 Chrome 一并关掉；关不掉由对账兜底
            return Err(format!("agent-browser 打开会话 {name} 失败：{e}"));
        }
        Ok(name.to_string())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        if !self.sessions()?.iter().any(|s| s == id) {
            return Err(format!("no such device: {id}"));
        }
        self.call(&["--session", id, "close"], 60).map(|_| ())
    }

    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.sessions()?.into_iter().filter(|s| s.starts_with("aq-")).map(|s| (s.clone(), s)).collect())
    }
}
