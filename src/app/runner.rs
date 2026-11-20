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
    /// 没放行；只因排头预留时带上排头和它占着的资源。
    Wait(Option<(String, Vec<String>)>),
}

/// 提交前先在事务里读（等提交方提交），读不到或已结束就退出。
fn load(store: &mut Store, id: &str) -> AppResult<Option<ExecRow>> {
    store.tx(|c| Ok::<_, AppError>(aq_store::get_exec(c, id)?))
}

/// 返回 None 表示已拒绝；Some(true) 表示首次见到、放行后要在后台评估。
fn evaluate(ctx: &Ctx, store: &mut Store, e: &ExecRow) -> AppResult<Option<bool>> {
    let est = estimate::estimate(ctx, store, e)?;
    ctx.log_note(
        &e.id,
        &format!(
            "estimate from {}: heavy {}, mem {}MB, disk {}MB, timeout {}s",
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
        return Ok(None);
    }
    store.tx(|c| aq_store::set_estimate(c, &e.id, &est.res, est.timeout_s))?;
    Ok(Some(est.source == aq_core::estimate::Source::FirstSeen))
}

fn try_start(ctx: &Ctx, c: &aq_store::Conn, e: &ExecRow) -> AppResult<Step> {
    let ck = check::check_in(ctx, c, Some(&e.id))?;
    let Some(now_row) = aq_store::get_exec(c, &e.id)? else { return Ok(Step::Stop) };
    if now_row.status != ExecStatus::Queued {
        return Ok(Step::Stop);
    }
    let adm = &ck.plan.admit;
    if !adm.admitted.iter().any(|a| a.id == e.id) {
        let head = adm.blocked_by.get(&e.id).map(|h| {
            let kinds = adm.waiting_for.get(&e.id).map(|k| k.iter().map(|x| x.code()).collect()).unwrap_or_default();
            (h.clone(), kinds)
        });
        return Ok(Step::Wait(head));
    }
    let log = ctx.dirs.log_file(&e.id);
    let env = [("AQ_EXECUTION_ID", e.id.clone())];
    let launch = Launch { exe: &ctx.exe, argv: &e.argv, cwd: std::path::Path::new(&e.cwd), log: &log, env: &env, nice: ctx.config.nice };
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
                    disk_reliable: false,
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

/// 排队直到放行；返回闸门和命令启动前一刻的剩余磁盘（排队期间别人写的不算这次的，CASE-EST-15）。
fn queue(ctx: &Ctx, store: &mut Store, e: &ExecRow) -> AppResult<Option<(Gate, i64)>> {
    let mut round = 0;
    // 被排头预留挡过：第一次挡住的时刻、最后一次是谁挡的、挡在哪类资源上。放行时写进 aq.log，用来判断要不要给更多资源做回填（CASE-QUEUE-13）
    let mut blocked: Option<(i64, String, Vec<String>)> = None;
    loop {
        match store.tx(|c| try_start(ctx, c, e))? {
            Step::Started(mut g) => {
                crash_point("after_commit");
                let disk_before = ctx.sys.sample().disk_free_mb;
                g.open().map_err(AppError::Fatal)?;
                if let Some((since, head, kinds)) = &blocked {
                    let s = (ctx.now() - since) / 1000;
                    ctx.log_note(&e.id, &format!("admitted after {s}s blocked by {head} on [{}]", kinds.join(", ")));
                }
                ctx.log_transition(&e.id, "queued", "running", "admitted");
                return Ok(Some((*g, disk_before)));
            }
            Step::Stop => return Ok(None),
            Step::Wait(head) => {
                if let Some((h, k)) = head {
                    let since = blocked.as_ref().map_or(ctx.now(), |b| b.0);
                    blocked = Some((since, h, k));
                }
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
        disk_reliable: o.disk_reliable,
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
    super::sweep::locks(ctx, store)?;
    Ok(())
}

pub fn run(ctx: &Ctx, store: &mut Store, id: &str, _lock: File) -> AppResult<()> {
    let Some(mut e) = load(store, id)? else { return Ok(()) };
    if e.status != ExecStatus::Queued {
        return Ok(());
    }
    let mut learn = false;
    if e.res.is_none() {
        let Some(first) = evaluate(ctx, store, &e)? else { return Ok(()) };
        learn = first;
        let Some(fresh) = load(store, id)? else { return Ok(()) };
        e = fresh;
    }
    // 首次见到的命令：AI 评估和排队、执行同时进行，不挡放行（CASE-EST-13）；runner 退出前等它写完缓存
    std::thread::scope(|s| {
        if learn {
            let row = e.clone();
            s.spawn(move || estimate::learn(ctx, &row));
        }
        execute(ctx, store, &e)
    })
}

fn execute(ctx: &Ctx, store: &mut Store, e: &ExecRow) -> AppResult<()> {
    let id = e.id.as_str();
    let Some((gate, disk_before)) = queue(ctx, store, e)? else { return Ok(()) };
    let Some(row) = aq_store::get_exec(store.conn(), id)? else { return Ok(()) };
    let mut o = watch(ctx, store, &row, gate)?;
    o.disk_delta_mb = (disk_before - ctx.sys.sample().disk_free_mb).max(0);
    finish(ctx, store, &row, &o)?;
    let _ = std::fs::remove_file(ctx.dirs.lock_file(id)); // 已是终态，没人再探测这把锁
    Ok(())
}
