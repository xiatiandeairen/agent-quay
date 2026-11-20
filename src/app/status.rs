//! aq status。

use std::time::{Duration, Instant};

use aq_core::model::{exit, DeviceType, ExecStatus, HoldStatus, Reason};
use aq_core::ports::SysRes;
use aq_store::Store;
use serde_json::{json, Value};

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, view};

/// 运行中任务里最早结束的剩余秒数（用于排队项 `next` 的等待秒数）。
pub fn soonest_free_s(ctx: &Ctx, store: &Store) -> AppResult<Option<i64>> {
    let now = ctx.now();
    Ok(aq_store::active_execs(store.conn())?
        .iter()
        .filter(|e| e.status == ExecStatus::Running)
        .filter_map(|e| Some((e.started_at? + e.timeout_s? / 3 * 1000 - now) / 1000))
        .map(|s| s.max(0))
        .min())
}

pub fn summary(ctx: &Ctx, store: &Store) -> AppResult<Value> {
    let c = store.conn();
    let execs = aq_store::active_execs(c)?;
    let devs = aq_store::devices(c)?;
    let cap = &ctx.config.cap;
    let sys = ctx.sys.sample();
    let running = execs.iter().filter(|e| e.status == ExecStatus::Running).count();
    let queued = execs.len() - running;
    let mut acct: i64 = execs.iter().filter(|e| e.status == ExecStatus::Running).filter_map(|e| e.res.map(|r| r.mem_mb)).sum();
    acct += devs.iter().filter(|d| d.dtype.is_virtual()).map(|d| cap.sim_mem_mb(d.dtype)).sum::<i64>();
    let used = (sys.mem_total_mb - sys.mem_avail_mb).max(0);
    let pct = |a: i64, b: i64| if b <= 0 { 0 } else { (a * 100 / b).clamp(0, 100) };
    let phones_total = (cap.ios_devices.len() + cap.android_devices.len()) as i64;
    let phones_out = devs.iter().filter(|d| !d.dtype.is_virtual() && d.device.is_some()).count() as i64;
    let sims = devs.iter().filter(|d| d.dtype == DeviceType::IosSim || d.dtype == DeviceType::AndroidEmu).count();
    Ok(json!({
        "running": running,
        "queued": queued,
        "cpu_pct": ctx.sys.cpu_pct(),
        "mem_pct": pct(acct.max(used), cap.mem_mb),
        "disk_pct": pct(sys.disk_total_mb - sys.disk_free_mb, sys.disk_total_mb),
        "phone_pct": pct(phones_out, phones_total),
        "sims": sims,
    }))
}

/// 不带 id：第一行全局概况，之后每行一个活动的执行或未归还的借用。
pub fn overview(ctx: &Ctx, store: &mut Store) -> AppResult<(Vec<Value>, i32)> {
    let waiting = check::check(ctx, store)?.plan.admit;
    let note = pressure_note(ctx);
    let mut out = vec![summary(ctx, store)?];
    let soonest = soonest_free_s(ctx, store)?;
    for e in aq_store::active_execs(store.conn())? {
        out.push(view::exec_json(ctx, &e, None, soonest));
    }
    for h in aq_store::active_holds(store.conn())? {
        let devs = view::hold_devices(store.conn(), &h.id)?;
        out.push(view::hold_json(&h, &devs));
    }
    for v in out.iter_mut().skip(1) {
        view::annotate_waiting(v, &waiting, &note);
    }
    Ok((out, 0))
}

fn one(ctx: &Ctx, store: &Store, id: &str, soonest: Option<i64>) -> AppResult<(Value, i32, bool)> {
    if let Some(e) = aq_store::get_exec(store.conn(), id)? {
        return Ok((view::exec_json(ctx, &e, None, soonest), e.status.exit_code(), e.status.is_terminal()));
    }
    if let Some(h) = aq_store::get_hold(store.conn(), id)? {
        let devs = view::hold_devices(store.conn(), id)?;
        let mut v = view::hold_json(&h, &devs);
        if h.status == HoldStatus::Held {
            let gone = missing_devices(ctx, store, id)?;
            if !gone.is_empty() {
                ctx.log_note(id, &format!("held devices are gone: {}", gone.join(", ")));
                v["missing"] = json!(gone);
            }
        }
        return Ok((v, h.status.exit_code(), h.status.is_settled()));
    }
    Ok((view::rejected_with_id(id, "usage", "no such id (or already pruned by retention)"), Reason::Usage.exit_code(), true))
}

/// 内存压力持续了多久（读状态目录的开始时间，检查时已更新）。
fn pressure_note(ctx: &Ctx) -> view::PressureNote {
    let since = std::fs::metadata(ctx.dirs.state.join("mem-pressure-since"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() as i64);
    view::PressureNote { for_s: since, warn_after_s: ctx.tuning.pressure_warn_ms / 1000 }
}

/// held 的借用里已经不存在的虚拟设备（被外部删掉、模拟器服务被重置）。借用状态不变，由调用方决定归还重借（CASE-HOLD-11）。
/// 列不出设备（工具没装、超时）时判不了，当作都在。desktop 只是会话名，没有实体可查。
fn missing_devices(ctx: &Ctx, store: &Store, hold_id: &str) -> AppResult<Vec<String>> {
    let mine: Vec<(DeviceType, String)> = aq_store::devices(store.conn())?
        .into_iter()
        .filter(|d| d.hold_id == hold_id && d.dtype.is_virtual() && d.dtype != DeviceType::Desktop)
        .filter_map(|d| Some((d.dtype, d.device?)))
        .collect();
    let mut gone = vec![];
    let mut types: Vec<DeviceType> = vec![];
    for (t, _) in &mine {
        if !types.contains(t) {
            types.push(*t);
        }
    }
    for t in types {
        let Some(backend) = ctx.virtual_backend(t) else { continue };
        let Ok(list) = backend.list_aq() else { continue };
        let present: std::collections::HashSet<String> = list.into_iter().map(|(_, id)| id).collect();
        gone.extend(mine.iter().filter(|(dt, id)| *dt == t && !present.contains(id)).map(|(_, id)| id.clone()));
    }
    Ok(gone)
}

/// 带 id：逐个查；带 --wait 每隔 poll 查一次，直到全部结束或到期。退出码按优先级取最需要处理的（CASE-STATUS-2）。
pub fn by_ids(ctx: &Ctx, store: &mut Store, ids: &[String], wait_s: Option<u64>) -> AppResult<(Vec<Value>, i32)> {
    let mut waiting = check::check(ctx, store)?.plan.admit;
    let deadline = Instant::now() + Duration::from_secs(wait_s.unwrap_or(0));
    let mut round = 0;
    loop {
        let soonest = soonest_free_s(ctx, store)?;
        let rows = ids.iter().map(|id| one(ctx, store, id, soonest)).collect::<AppResult<Vec<_>>>()?;
        let done = rows.iter().all(|r| r.2);
        if done || Instant::now() >= deadline {
            if !done && round > 0 {
                waiting = check::check(ctx, store)?.plan.admit; // 等了一阵还没结束：按最新一次检查报在等什么
            }
            let code = exit::worst(rows.iter().map(|r| r.1));
            let mut out: Vec<Value> = rows.into_iter().map(|r| r.0).collect();
            let note = pressure_note(ctx);
            for v in &mut out {
                view::annotate_waiting(v, &waiting, &note);
            }
            return Ok((out, code));
        }
        ctx.pace(round, Some(deadline));
        round += 1;
    }
}

pub fn require_ids(ids: &[String]) -> AppResult<()> {
    if ids.iter().any(|i| i.is_empty()) {
        return Err(AppError::Usage("id cannot be empty".into()));
    }
    Ok(())
}
