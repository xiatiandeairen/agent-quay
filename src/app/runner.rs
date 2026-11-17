//! run 的 runner：评估 → 排队 → 闸门启动 → 监视 → 结束。

use std::fs::File;

use aq_core::admit::{unschedulable, Demand};
use aq_core::model::ExecStatus;
use aq_core::ports::{Procs, SysRes};
use aq_store::{ExecRow, Finish, Store};
use aq_sys::spawn::{spawn_gated, Gate, Launch};

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::monitor::{watch, Outcome};
use super::{check, estimate};

/// 验收用的崩溃注入点（CASE-FAULT-4）：AQ_TEST_CRASH_AT 等于该点名字时 runner 立即异常退出，模拟 kill -9。
fn crash_point(name: &str) {
    if std::env::var("AQ_TEST_CRASH_AT").is_ok_and(|v| v == name) {
        std::process::abort();
    }
}

enum Step {
    Started(Box<Gate>),
    Stop,
    Wait,
}

/// 提交前先在事务里读（等提交方提交），读不到或已结束就退出。
fn load(store: &mut Store, id: &str) -> AppResult<Option<ExecRow>> {
    store.tx(|c| Ok::<_, AppError>(aq_store::get_exec(c, id)?))
}

fn evaluate(ctx: &Ctx, store: &mut Store, e: &ExecRow) -> AppResult<bool> {
    let est = estimate::estimate(ctx, store, e)?;
    ctx.log_note(
        &e.id,
        &format!(
            "资源评估来自{}：重任务 {}，内存 {}MB，磁盘 {}MB，超时 {}s",
            est.source.label(),
            est.res.heavy,
            est.res.mem_mb,
            est.res.disk_mb,
            est.timeout_s
        ),
    );
    if let Some(why) = unschedulable(&Demand::Exec(est.res), &ctx.config.cap) {
        if store.tx(|c| aq_store::reject_exec(c, &e.id, ctx.now()))? {
            ctx.log_transition(&e.id, "queued", "rejected", &why);
        }
        return Ok(false);
    }
    store.tx(|c| aq_store::set_estimate(c, &e.id, &est.res, est.timeout_s))?;
    Ok(true)
}

fn try_start(ctx: &Ctx, c: &aq_store::Conn, e: &ExecRow) -> AppResult<Step> {
    let ck = check::check_in(ctx, c, Some(&e.id))?;
    let Some(now_row) = aq_store::get_exec(c, &e.id)? else { return Ok(Step::Stop) };
    if now_row.status != ExecStatus::Queued {
        return Ok(Step::Stop);
    }
    if !ck.plan.admit.admitted.iter().any(|a| a.id == e.id) {
        return Ok(Step::Wait);
    }
    let log = ctx.dirs.log_file(&e.id);
    let env = [("AQ_EXECUTION_ID", e.id.clone())];
    let launch = Launch { exe: &ctx.exe, argv: &e.argv, cwd: std::path::Path::new(&e.cwd), log: &log, env: &env };
    let now = ctx.now();
    let gate = match spawn_gated(&launch) {
        Ok(g) => g,
        Err(err) => {
            // 起不来（如执行目录已被删）：按命令找不到处理，failed 127，原因写进日志
            let _ = std::fs::write(&log, format!("aq: {err}\n")); // 日志写失败不影响结果
            let me = std::process::id() as i64;
            aq_store::mark_running(c, &e.id, now, me, aq_sys::procs::self_start_ms())?;
            aq_store::finish(
                c,
                &e.id,
                &Finish {
                    status: ExecStatus::Failed,
                    finished_at: now,
                    exit_code: Some(127),
                    peak_mem_mb: 0,
                    cpu_avg_cores: 0.0,
                    disk_delta_mb: 0,
                    content_changed: false,
                },
            )?;
            ctx.log_transition(&e.id, "queued", "failed", &err);
            return Ok(Step::Stop);
        }
    };
    let start = ctx.procs.start_time(gate.pid()).unwrap_or(now);
    crash_point("before_commit");
    if !aq_store::mark_running(c, &e.id, now, gate.pid(), start)? {
        return Ok(Step::Stop); // 闸门随 gate 丢弃而关闭，命令从未执行
    }
    Ok(Step::Started(Box::new(gate)))
}

fn queue(ctx: &Ctx, store: &mut Store, e: &ExecRow) -> AppResult<Option<Gate>> {
    let mut round = 0;
    loop {
        match store.tx(|c| try_start(ctx, c, e))? {
            Step::Started(mut g) => {
                crash_point("after_commit");
                g.open().map_err(AppError::Fatal)?;
                ctx.log_transition(&e.id, "queued", "running", "放行");
                return Ok(Some(*g));
            }
            Step::Stop => return Ok(None),
            Step::Wait => {
                ctx.pace(round, None);
                round += 1;
            }
        }
    }
}

fn finish(ctx: &Ctx, store: &mut Store, e: &ExecRow, o: &Outcome) -> AppResult<()> {
    let fp_now = aq_core::ports::Git::locate(&ctx.git, &e.cwd).ok().flatten().and_then(|r| r.fingerprint);
    let changed = e.fingerprint.is_some() && fp_now != e.fingerprint;
    let f = Finish {
        status: o.status,
        finished_at: ctx.now(),
        exit_code: o.exit_code,
        peak_mem_mb: o.peak_mem_mb,
        cpu_avg_cores: o.cpu_avg_cores,
        disk_delta_mb: o.disk_delta_mb,
        content_changed: changed,
    };
    let removed = store.tx(|c| {
        aq_store::finish(c, &e.id, &f)?;
        Ok::<_, AppError>(aq_store::prune(c, ctx.tuning.keep)?)
    })?;
    ctx.log_transition(&e.id, "running", o.status.as_str(), &o.why);
    for id in removed {
        let _ = std::fs::remove_file(ctx.dirs.log_file(&id)); // 日志可能已被总量清理删掉
    }
    super::logs::enforce_total(ctx, store)?;
    Ok(())
}

pub fn run(ctx: &Ctx, store: &mut Store, id: &str, _lock: File) -> AppResult<()> {
    let Some(mut e) = load(store, id)? else { return Ok(()) };
    if e.status != ExecStatus::Queued {
        return Ok(());
    }
    if e.res.is_none() {
        if !evaluate(ctx, store, &e)? {
            return Ok(());
        }
        let Some(fresh) = load(store, id)? else { return Ok(()) };
        e = fresh;
    }
    let disk_before = ctx.sys.sample().disk_free_mb;
    let Some(gate) = queue(ctx, store, &e)? else { return Ok(()) };
    let Some(row) = aq_store::get_exec(store.conn(), id)? else { return Ok(()) };
    let mut o = watch(ctx, store, &row, gate)?;
    o.disk_delta_mb = (disk_before - ctx.sys.sample().disk_free_mb).max(0);
    finish(ctx, store, &row, &o)?;
    let _ = std::fs::remove_file(ctx.dirs.lock_file(id)); // 已是终态，没人再探测这把锁
    Ok(())
}
