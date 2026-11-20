//! aq release 与 aq cancel：接受即退出码 0；id 不存在为 rejected usage（3）。

use std::time::{Duration, Instant};

use aq_core::model::{exit, ExecStatus, HoldStatus, ProcId};
use aq_core::ports::{Locks, Procs};
use aq_store::Store;
use aq_sys::spawn::{group_empty, killpg, SIGKILL, SIGTERM};
use serde_json::Value;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, view};

pub fn release(ctx: &Ctx, store: &mut Store, id: &str) -> AppResult<(Value, i32)> {
    let (h, changed, from) = store.tx(|c| {
        check::check_in(ctx, c, None)?;
        let h = aq_store::get_hold(c, id)?.ok_or_else(|| AppError::Usage(format!("no such hold: {id}")))?;
        let changed = aq_store::end_hold(c, id, HoldStatus::Released, ctx.now())?;
        let h2 = aq_store::get_hold(c, id)?.unwrap_or(h.clone());
        Ok::<_, AppError>((h2, changed, h.status))
    })?;
    if changed {
        ctx.clear_hold_files(id);
        ctx.log_transition(id, from.as_str(), "released", "aq release");
        if h.request.devices.iter().any(|d| d.is_virtual()) {
            super::simgc::kick(ctx)?;
        }
    }
    Ok((view::hold_json(&h, &[]), 0))
}

/// 撤销：排队中的直接 cancelled；运行中的写入撤销请求后最多等「宽限期 + 5 秒」，等到终态再返回（CASE-CANCEL-2、5、6）。
/// runner 已不在、命令还在跑时没人处理撤销请求，由 cancel 自己终止进程组（CASE-CANCEL-10）。
pub fn cancel(ctx: &Ctx, store: &mut Store, id: &str) -> AppResult<(Value, i32)> {
    let e = store.tx(|c| {
        check::check_in(ctx, c, None)?;
        let e = aq_store::get_exec(c, id)?.ok_or_else(|| AppError::Usage(format!("no such execution: {id}")))?;
        match e.status {
            ExecStatus::Queued if aq_store::cancel_queued(c, id, ctx.now())? => {
                ctx.log_transition(id, "queued", "cancelled", "aq cancel");
            }
            ExecStatus::Running if !e.cancel_requested => {
                aq_store::request_cancel(c, id)?;
                ctx.log_note(id, "cancel requested; waiting for the runner to stop the process group");
            }
            _ => {}
        }
        Ok::<_, AppError>(aq_store::get_exec(c, id)?.unwrap_or(e))
    })?;
    let e = if e.status == ExecStatus::Running { wait_cancelled(ctx, store, &e)? } else { e };
    let mut v = view::exec_json(ctx, &e, None, None);
    // 已停完（或本来就结束了）即 0；进程还没停，调用方要接着等，与 running 同为 2（CASE-CANCEL-6）
    let code = if e.status == ExecStatus::Running {
        v["cancel_requested"] = serde_json::json!(true);
        exit::PENDING
    } else {
        exit::DONE
    };
    Ok((v, code))
}

/// 等运行中的执行到终态，最多「宽限期 + 5 秒」。runner 不在时自己终止：拿到 runner 的锁说明 runner 已退出，
/// 进程组还在（组长号和启动时间都对得上）才发信号，不会误杀复用了同一进程号的新进程。
fn wait_cancelled(ctx: &Ctx, store: &mut Store, e: &aq_store::ExecRow) -> AppResult<aq_store::ExecRow> {
    let deadline = Instant::now() + ctx.tuning.kill_grace + Duration::from_secs(5);
    if let Some(_runner_lock) = ctx.locks.try_lock(&e.id).map_err(AppError::Fatal)? {
        if let Some(pg) = e.pgid.zip(e.pgid_started_at).map(|(pid, started_at)| ProcId { pid, started_at }) {
            if ctx.procs.group_alive(pg) {
                stop_group(ctx, pg.pid);
            }
        }
        let ended = store.tx(|c| Ok::<_, AppError>(aq_store::cancel_running(c, &e.id, ctx.now())?))?;
        if ended {
            ctx.log_transition(&e.id, "running", "cancelled", "aq cancel; runner was gone, stopped the process group itself");
        }
    }
    loop {
        let now = aq_store::get_exec(store.conn(), &e.id)?.unwrap_or_else(|| e.clone());
        if now.status.is_terminal() || Instant::now() >= deadline {
            return Ok(now);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// SIGTERM → 宽限期 → SIGKILL，等进程组清空（与 runner 的终止相同，只是没有子进程句柄可回收）。
fn stop_group(ctx: &Ctx, pgid: i64) {
    killpg(pgid, SIGTERM);
    let grace = Instant::now() + ctx.tuning.kill_grace;
    while !group_empty(pgid) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !group_empty(pgid) {
        killpg(pgid, SIGKILL);
        let until = Instant::now() + Duration::from_secs(5);
        while !group_empty(pgid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
