//! 所有命令共用的「检查」在 app 层的落地：事务里采集探测 → core::plan → 执行动作。

use std::collections::HashMap;
use std::fs::File;

use aq_core::check::{plan, Action, Plan, Probes, Snapshot};
use aq_core::model::{ExecStatus, HoldStatus, ReclaimReason};
use aq_core::ports::{Locks, Procs, SysRes};
use aq_store::Conn;
use aq_sys::lock::inheritable;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::spawn;

/// 检查结果；`self_id` 自己的锁不探测（自己持有）。
pub struct Checked {
    pub plan: Plan,
}

fn mtime_ms(p: &std::path::Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    m.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

fn probe(ctx: &Ctx, s: &Snapshot, self_id: Option<&str>, guards: &mut HashMap<String, File>) -> AppResult<Probes> {
    let mut p = Probes { sys: ctx.sys.sample(), ..Probes::default() };
    ctx.track_pressure(p.sys.mem_tight());
    let mut try_runner = |id: &str| -> AppResult<bool> {
        if Some(id) == self_id {
            return Ok(false);
        }
        match ctx.locks.try_lock(id).map_err(AppError::Fatal)? {
            Some(f) => {
                guards.insert(id.to_string(), f);
                Ok(true)
            }
            None => Ok(false),
        }
    };
    for e in &s.execs {
        let gone = try_runner(&e.id)?;
        p.runner_gone.insert(e.id.clone(), gone);
        if let (ExecStatus::Running, Some(pg)) = (e.status, e.pgid) {
            p.group_mem_mb.insert(e.id.clone(), ctx.procs.group_mem_mb(pg.pid));
            if gone {
                p.group_alive.insert(e.id.clone(), ctx.procs.group_alive(pg));
            }
        }
    }
    for h in &s.holds {
        match h.status {
            HoldStatus::Held => {
                if h.owner != aq_core::model::ProcId::UNKNOWN {
                    p.owner_alive.insert(h.id.clone(), ctx.procs.alive(h.owner));
                    // 未识别的不核对（CASE-RECLAIM-5）
                }
                if let Some(t) = mtime_ms(&ctx.dirs.used_file(&h.id)) {
                    p.last_used_at.insert(h.id.clone(), t);
                }
            }
            HoldStatus::Waiting => {
                let gone = try_runner(&h.id)?;
                p.runner_gone.insert(h.id.clone(), gone);
            }
            _ => {}
        }
    }
    p.in_use = super::usage::in_use(s);
    Ok(p)
}

fn apply(ctx: &Ctx, c: &Conn, a: &Action, guards: &mut HashMap<String, File>) -> AppResult<bool> {
    let now = ctx.now();
    match a {
        Action::Interrupt(id) => {
            if aq_store::interrupt(c, id, now)? {
                ctx.log_transition(id, "running", "interrupted", "runner and command are both gone");
            }
        }
        Action::RespawnExec(id) | Action::RespawnHold(id) => {
            if matches!(a, Action::RespawnHold(_)) {
                aq_store::clear_allocation(c, id)?; // 半成品模拟器由新 runner 启动时按 aq-<id>- 前缀删除
            }
            let Some(lock) = guards.remove(id) else { return Ok(false) };
            let fd = inheritable(&lock).map_err(AppError::Fatal)?;
            spawn::runner(ctx, id, fd)?;
            ctx.log_note(id, "runner was gone; restarted it, queue position kept");
        }
        Action::Reclaim(id, why) => {
            if aq_store::end_hold(c, id, HoldStatus::Reclaimed, now)? {
                ctx.clear_hold_files(id);
                let reason = match why {
                    ReclaimReason::OwnerExit => "holder process exited",
                    ReclaimReason::Idle => "idle past the limit",
                    ReclaimReason::MaxHold => "held past the max hold time",
                };
                ctx.log_transition(id, "held", "reclaimed", reason);
                return Ok(true);
            }
        }
        Action::MarkUsed(id) => super::usage::mark_used(ctx, id),
    }
    Ok(false)
}

/// 在调用方已开的事务里做一次检查。
pub fn check_in(ctx: &Ctx, c: &Conn, self_id: Option<&str>) -> AppResult<Checked> {
    check_submitting(ctx, c, self_id, &[])
}

/// 同 check_in；`submitting` 是同一事务里正要提交的 aq run 命令，参数带着借到的设备 id 时提交那一刻就算在用（CASE-RECLAIM-10）。
pub fn check_submitting(ctx: &Ctx, c: &Conn, self_id: Option<&str>, submitting: &[String]) -> AppResult<Checked> {
    let mut snapshot = aq_store::snapshot(c, ctx.now())?;
    snapshot.submitting = submitting.to_vec();
    let mut guards = HashMap::new();
    let probes = probe(ctx, &snapshot, self_id, &mut guards)?;
    let plan = plan(&snapshot, &probes, &ctx.config.cap, &ctx.limits);
    let mut reclaimed = false;
    for a in &plan.actions {
        reclaimed |= apply(ctx, c, a, &mut guards)?;
    }
    if reclaimed || !aq_store::orphan_sim_rows(c)?.is_empty() || super::simgc::due(ctx) {
        super::simgc::kick(ctx)?;
    }
    Ok(Checked { plan })
}

/// 单独开一个事务做检查（status、cancel、release 开头用）。
pub fn check(ctx: &Ctx, store: &mut aq_store::Store) -> AppResult<Checked> {
    store.tx(|c| check_in(ctx, c, None))
}
