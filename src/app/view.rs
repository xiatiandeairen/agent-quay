//! 对外 JSON：每个对象一行，只含决策所需字段，按规定时机出现。字段与 docs/design.md 的输出字段表一致。

use aq_core::model::{ExecStatus, HoldStatus};
use aq_core::retry;
use aq_store::{ExecRow, HoldRow};
use serde_json::{json, Map, Value};

use super::ctx::Ctx;

/// `reused` 只有 `aq run` 的返回才有意义（这次提交是否接上了已有的同一执行）；status、cancel 传 None，不输出这个字段。
pub fn exec_json(ctx: &Ctx, e: &ExecRow, reused: Option<bool>, soonest_free_s: Option<i64>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(e.id));
    m.insert("status".into(), json!(e.status.as_str()));
    if let Some(r) = reused {
        m.insert("reused".into(), json!(r));
    }
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

/// 内存压力已持续多少秒（没有压力时为 None），以及超过多久要加说明。
pub struct PressureNote {
    pub for_s: Option<i64>,
    pub warn_after_s: i64,
}

/// 排队、等待中的对象加上 `waiting_for`：在等哪类资源（CASE-QUEUE-5、CASE-QUEUE-6、CASE-HOLD-12）。
/// 在等内存压力回落时另加 `mem_pressure_s`（已持续多久）；超过上限再加 `message`，仍然排队、不失败。
pub fn annotate_waiting(v: &mut Value, waiting: &std::collections::HashMap<String, Vec<aq_core::admit::Kind>>, p: &PressureNote) {
    let pending = matches!(v["status"].as_str(), Some("queued" | "waiting"));
    let Some(kinds) = v["id"].as_str().and_then(|id| waiting.get(id)) else { return };
    if !pending || kinds.is_empty() {
        return;
    }
    v["waiting_for"] = json!(kinds.iter().map(|k| k.code()).collect::<Vec<_>>());
    if let (true, Some(s)) = (kinds.contains(&aq_core::admit::Kind::MemPressure), p.for_s) {
        v["mem_pressure_s"] = json!(s);
        if s >= p.warn_after_s {
            v["message"] =
                json!(format!("系统内存压力已持续 {} 分钟，压力来自 aq 以外的程序；仍在排队，关掉占内存的程序后会自动开始", s / 60));
        }
    }
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
