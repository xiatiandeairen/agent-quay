//! aq status。

use std::time::{Duration, Instant};

use aq_core::model::{DeviceType, ExecStatus};
use aq_core::ports::SysRes;
use aq_store::Store;
use serde_json::{json, Value};

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, view};

/// 运行中任务里最早结束的剩余秒数（用于排队项的 retry_after_s）。
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
    check::check(ctx, store)?;
    let mut out = vec![summary(ctx, store)?];
    let soonest = soonest_free_s(ctx, store)?;
    for e in aq_store::active_execs(store.conn())? {
        out.push(view::exec_json(ctx, &e, false, soonest));
    }
    for h in aq_store::active_holds(store.conn())? {
        let devs = view::hold_devices(store.conn(), &h.id)?;
        out.push(view::hold_json(&h, &devs));
    }
    Ok((out, 0))
}

fn one(ctx: &Ctx, store: &Store, id: &str, soonest: Option<i64>) -> AppResult<(Value, i32, bool)> {
    if let Some(e) = aq_store::get_exec(store.conn(), id)? {
        return Ok((view::exec_json(ctx, &e, false, soonest), e.status.exit_code(), e.status.is_terminal()));
    }
    if let Some(h) = aq_store::get_hold(store.conn(), id)? {
        let devs = view::hold_devices(store.conn(), id)?;
        return Ok((view::hold_json(&h, &devs), h.status.exit_code(), h.status.is_settled()));
    }
    Ok((view::rejected_with_id(id, "usage", "id 不存在（或已被保留策略清理）"), 3, true))
}

/// 带 id：逐个查；带 --wait 每隔 poll 查一次，直到全部结束或到期。退出码取最大。
pub fn by_ids(ctx: &Ctx, store: &mut Store, ids: &[String], wait_s: Option<u64>) -> AppResult<(Vec<Value>, i32)> {
    check::check(ctx, store)?;
    let deadline = Instant::now() + Duration::from_secs(wait_s.unwrap_or(0));
    let mut round = 0;
    loop {
        let soonest = soonest_free_s(ctx, store)?;
        let rows = ids.iter().map(|id| one(ctx, store, id, soonest)).collect::<AppResult<Vec<_>>>()?;
        let done = rows.iter().all(|r| r.2);
        if done || Instant::now() >= deadline {
            let code = rows.iter().map(|r| r.1).max().unwrap_or(0);
            return Ok((rows.into_iter().map(|r| r.0).collect(), code));
        }
        ctx.pace(round, Some(deadline));
        round += 1;
    }
}

pub fn require_ids(ids: &[String]) -> AppResult<()> {
    if ids.iter().any(|i| i.is_empty()) {
        return Err(AppError::Usage("id 不能为空".into()));
    }
    Ok(())
}
