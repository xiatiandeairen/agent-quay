//! --human：给人看的完整文本（目录、指纹、队位、在等什么、各阶段时间、评估来源、借用者与借用时长）。
//! 退出码与不加 --human 相同。

use std::fmt::Write;

use aq_core::model::{ExecStatus, HoldStatus};
use aq_core::ports::{Locks, Procs};
use aq_store::{ExecRow, HoldRow, Store};
use serde_json::Value;

use super::check;
use super::ctx::Ctx;
use super::error::AppResult;

fn dur(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 {
        format!("{}h{}m", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn short(argv: &[String]) -> String {
    let s = argv.join(" ");
    if s.chars().count() > 70 {
        format!("{}…", s.chars().take(70).collect::<String>())
    } else {
        s
    }
}

fn runner_state(ctx: &Ctx, id: &str) -> &'static str {
    match ctx.locks.try_lock(id) {
        Ok(Some(_)) => "runner 不在",
        Ok(None) => "runner 在",
        Err(_) => "runner 未知",
    }
}

/// 评估来自哪里：提交前同类命令已有实测 → 实测；否则有 AI 缓存 → AI 评估；否则保守默认。
pub(super) fn source(store: &Store, e: &ExecRow) -> aq_core::estimate::Source {
    use aq_core::estimate::Source;
    let c = store.conn();
    if aq_store::measured_before(c, &e.profile_key, e.submitted_at).unwrap_or(0) > 0 {
        Source::Measured
    } else if aq_store::get_profile(c, &e.profile_key).ok().flatten().is_some() {
        Source::Ai
    } else {
        Source::Default
    }
}

fn exec_line(ctx: &Ctx, store: &Store, e: &ExecRow, waiting: &std::collections::HashMap<String, Vec<aq_core::admit::Kind>>) -> String {
    let now = ctx.now();
    let fp = e.fingerprint.as_deref().map_or("无（不在 git 或 --fresh）".to_string(), |f| {
        f.chars().take(12).collect::<String>() + if f.contains("dirty") { "+dirty" } else { "" }
    });
    let mut s = format!("{}  {:<11} {}\n    目录 {}  指纹 {}", e.id, e.status.as_str(), short(&e.argv), e.cwd, fp);
    if let Some(r) = e.res {
        let _ = write!(
            s,
            "\n    记账 重任务 {} 内存 {}MB 磁盘 {}MB 下载 {}  超时 {}s  评估来自{}",
            r.heavy,
            r.mem_mb,
            r.disk_mb,
            r.uses_net,
            e.timeout_s.unwrap_or(0),
            source(store, e).label()
        );
    } else {
        s.push_str("\n    资源评估中");
    }
    match e.status {
        ExecStatus::Queued => {
            let why: Vec<String> = waiting.get(&e.id).map(|k| k.iter().map(|x| x.label()).collect()).unwrap_or_default();
            let _ = write!(
                s,
                "\n    已排队 {}  在等 {}  {}",
                dur(now - e.submitted_at),
                if why.is_empty() { "轮到即放行".into() } else { why.join("、") },
                runner_state(ctx, &e.id)
            );
        }
        ExecStatus::Running => {
            let alive =
                e.pgid.zip(e.pgid_started_at).is_some_and(|(p, t)| ctx.procs.group_alive(aq_core::model::ProcId { pid: p, started_at: t }));
            let _ = write!(
                s,
                "\n    已运行 {}  进程组 {} {}  {}",
                dur(now - e.started_at.unwrap_or(now)),
                e.pgid.unwrap_or(0),
                if alive { "在" } else { "已结束" },
                runner_state(ctx, &e.id)
            );
        }
        _ => {
            let _ = write!(
                s,
                "\n    排队 {}  运行 {}  实测 峰值内存 {}MB 平均 {:.1} 核",
                dur(e.started_at.unwrap_or(e.submitted_at) - e.submitted_at),
                dur(e.finished_at.unwrap_or(now) - e.started_at.unwrap_or(now)),
                e.peak_mem_mb.unwrap_or(0),
                e.cpu_avg_cores.unwrap_or(0.0)
            );
        }
    }
    if e.started_at.is_some() {
        let _ = write!(s, "\n    日志 {}", ctx.dirs.log_file(&e.id).display());
    }
    s
}

fn hold_line(ctx: &Ctx, store: &Store, h: &HoldRow) -> AppResult<String> {
    let now = ctx.now();
    let devs = super::view::hold_devices(store.conn(), &h.id)?;
    let devs: Vec<String> = devs.iter().map(|(t, d)| format!("{t} {}", if d.is_empty() { "创建中" } else { d })).collect();
    let who = if h.owner == aq_core::model::ProcId::UNKNOWN {
        "未识别".into()
    } else {
        ctx.procs.ancestors(h.owner.pid).first().map(|a| a.1.clone()).unwrap_or_else(|| "已退出".into())
    };
    let mut s = format!(
        "{}  {:<9} 借用者 {}(pid {})  设备 {}  账号 {}",
        h.id,
        h.status.as_str(),
        who,
        h.owner.pid,
        devs.join("、"),
        h.request.accounts.join("、")
    );
    if h.status == HoldStatus::Held {
        let used = std::fs::metadata(ctx.dirs.used_file(&h.id)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        let idle = used.map_or("未知".into(), |d| dur(d.as_millis() as i64));
        let _ = write!(s, "\n    已借 {}  闲置 {}", dur(now - h.acquired_at.unwrap_or(now)), idle);
    } else if h.status == HoldStatus::Waiting {
        let _ = write!(s, "\n    已等 {}", dur(now - h.requested_at));
    }
    Ok(s)
}

pub fn overview(ctx: &Ctx, store: &mut Store) -> AppResult<(String, i32)> {
    let ck = check::check(ctx, store)?;
    let sum = super::status::summary(ctx, store)?;
    let g = |k: &str| sum.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut s = format!(
        "目录    配置 {}  状态 {}\n任务    运行 {}   排队 {}\n资源    CPU {}%   内存 {}%   磁盘 {}%   真机 {}%   模拟器 {} 台\n",
        ctx.dirs.config_file.display(),
        ctx.dirs.state.display(),
        g("running"),
        g("queued"),
        g("cpu_pct"),
        g("mem_pct"),
        g("disk_pct"),
        g("phone_pct"),
        g("sims")
    );
    for e in aq_store::active_execs(store.conn())? {
        let _ = writeln!(s, "{}", exec_line(ctx, store, &e, &ck.plan.admit.waiting_for));
    }
    for h in aq_store::active_holds(store.conn())? {
        let _ = writeln!(s, "{}", hold_line(ctx, store, &h)?);
    }
    Ok((s, 0))
}

pub fn objects(ctx: &Ctx, store: &mut Store, ids: &[String]) -> AppResult<String> {
    let ck = check::check(ctx, store)?;
    let mut s = String::new();
    for id in ids {
        if let Some(e) = aq_store::get_exec(store.conn(), id)? {
            let _ = writeln!(s, "{}", exec_line(ctx, store, &e, &ck.plan.admit.waiting_for));
        } else if let Some(h) = aq_store::get_hold(store.conn(), id)? {
            let _ = writeln!(s, "{}", hold_line(ctx, store, &h)?);
        } else {
            let _ = writeln!(s, "{id}  不存在（rejected usage）");
        }
    }
    Ok(s)
}
