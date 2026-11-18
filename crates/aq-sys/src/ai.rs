//! 调用 agent 自己的 AI CLI 做资源评估。
//! 在空的临时目录里运行、关掉工具和 MCP、提示词走标准输入，命令原文里夹带的指令无法让 AI 读写文件。

use std::time::Duration;

use aq_core::ports::AiCli;

use crate::cmd::{run, Call};

pub struct AiCommand {
    pub argv: Vec<String>,
    pub timeout: Duration,
}

impl AiCli for AiCommand {
    fn ask(&self, prompt: &str) -> Result<String, String> {
        let dir = std::env::temp_dir().join(format!("aq-ai-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create a temp dir: {e}"))?;
        let r = run(&Call { argv: &self.argv, cwd: Some(&dir), env: &[], stdin: Some(prompt.as_bytes()), timeout: self.timeout });
        let _ = std::fs::remove_dir_all(&dir); // 临时目录删不掉不影响评估结果
        let o = r?;
        if o.status != 0 {
            return Err(format!("AI CLI exited with {}: {}", o.status, o.stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }
}
