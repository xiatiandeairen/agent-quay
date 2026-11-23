//! 桌面：一个 agent-desktop 会话名。agent 用 `agent-desktop --session <id> ...` 操作，默认无窗口模式
//! （走辅助功能接口，不动鼠标、不抢焦点），多个 agent 可同时操作不同 App。
//! agent-desktop 的 --session 只是快照引用的命名空间，不起进程，所以借时只发名字、还时无事可做；
//! 也没有可列出的实体，不参与按名字对账，不走 Simulator 契约（契约要求建完能列出来）。

use aq_core::devspec::DeviceSpec;
use aq_core::ports::Simulator;

pub struct AgentDesktop;

impl Simulator for AgentDesktop {
    fn create(&self, name: &str, _spec: &DeviceSpec) -> Result<String, String> {
        Ok(name.to_string())
    }

    fn delete(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }

    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        Ok(vec![])
    }
}
