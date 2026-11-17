//! 对外 JSON：每个对象一行，只含决策所需字段，按规定时机出现。字段与 docs/design.md 的输出字段表一致。

use aq_core::model::{ExecStatus, HoldStatus};
use aq_core::retry;
use aq_store::{ExecRow, HoldRow};
use serde_json::{json, Map, Value};

use super::ctx::Ctx;

pub fn exec_json(ctx: &Ctx, e: &ExecRow, reused: bool, soonest_free_s: Option<i64>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(e.id));
    m.insert("status".into(), json!(e.status.as_str()));
    m.insert("reused".into(), json!(reused));
    let est = e.timeout_s.map(|t| t / 3);
    match e.status {
        ExecStatus::Running => {
            let elapsed = e.started_at.map_or(0, |s| (ctx.now() - s) / 1000);
            m.insert("retry_after_s".into(), json!(retry::running(est.unwrap_or(60), elapsed)));
        }
        ExecStatus::Queued => {
            m.insert("retry_after_s".into(), json!(retry::queued(soonest_free_s, est)));
        }
        _ => {}
    }
    if e.status == ExecStatus::Failed {
        m.insert("exit_code".into(), json!(e.exit_code));
    }
    if e.started_at.is_some() {
        m.insert("log".into(), json!(ctx.dirs.log_file(&e.id).to_string_lossy()));
    }
    if e.status.is_terminal() && e.content_changed == Some(true) {
        m.insert("content_changed".into(), json!(true));
    }
    if e.status == ExecStatus::Rejected {
        m.insert("reason".into(), json!("unschedulable"));
        m.insert("message".into(), json!("需求超过整机容量，永远不会被满足；见 aq status --human"));
    }
    Value::Object(m)
}

pub fn hold_json(h: &HoldRow, devices: &[(String, String)]) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(h.id));
    m.insert("status".into(), json!(h.status.as_str()));
    match h.status {
        HoldStatus::Held => {
            let d: Vec<Value> = devices.iter().map(|(t, id)| json!({"type": t, "id": id})).collect();
            m.insert("devices".into(), Value::Array(d));
            if !h.request.accounts.is_empty() {
                m.insert("accounts".into(), json!(h.request.accounts));
            }
        }
        HoldStatus::Waiting => {
            m.insert("retry_after_s".into(), json!(retry::queued(None, None)));
        }
        HoldStatus::Rejected => {
            m.insert("reason".into(), json!("device_error"));
            m.insert("message".into(), json!("模拟器创建或启动失败；整组已回滚，可重新 hold"));
        }
        _ => {}
    }
    Value::Object(m)
}

pub fn rejected(reason: &str, message: &str) -> Value {
    json!({"status": "rejected", "reason": reason, "message": message})
}

/// 真机判定拒绝：device_unavailable 带每台不可用的原因；有已连接但未声明的设备时带上它们和可粘贴的配置片段（CASE-DEV-2、CASE-DEV-3）。
pub fn rejected_devices(reason: &str, message: &str, v: &aq_core::devices::Verdict) -> Value {
    let mut o = rejected(reason, message);
    if reason == "device_unavailable" {
        let list: Vec<Value> =
            v.unavailable.iter().map(|d| json!({"type": d.kind.as_str(), "id": d.id, "name": d.name, "why": d.why})).collect();
        o["unavailable"] = json!(list);
    }
    if !v.undeclared.is_empty() {
        let list: Vec<Value> = v.undeclared.iter().map(|d| json!({"type": d.kind.as_str(), "id": d.id, "name": d.name})).collect();
        o["undeclared"] = json!(list);
        o["config_snippet"] = json!(v.snippet);
    }
    o
}

pub fn rejected_with_id(id: &str, reason: &str, message: &str) -> Value {
    json!({"id": id, "status": "rejected", "reason": reason, "message": message})
}

/// 一组借用的设备（按请求顺序）：(类型, id)。
pub fn hold_devices(c: &aq_store::Conn, hold_id: &str) -> aq_store::Result<Vec<(String, String)>> {
    Ok(aq_store::devices(c)?
        .into_iter()
        .filter(|d| d.hold_id == hold_id)
        .map(|d| (d.dtype.as_str().to_string(), d.device.unwrap_or_default()))
        .collect())
}
