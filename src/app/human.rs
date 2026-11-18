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
use super::view::gb;

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
        Ok(Some(_)) => "runner gone",
        Ok(None) => "runner alive",
        Err(_) => "runner unknown",
    }
}

/// 评估来自哪里：提交前同类命令已有实测 → 实测；否则有 AI 缓存 → AI 评估；否则首次见到。
pub(super) fn source(store: &Store, e: &ExecRow) -> aq_core::estimate::Source {
    use aq_core::estimate::Source;
    let c = store.conn();
    if aq_store::measured_before(c, &e.profile_key, e.submitted_at).unwrap_or(0) > 0 {
        Source::Measured
    } else if aq_store::get_profile(c, &e.profile_key).ok().flatten().is_some() {
        Source::Ai
    } else {
        Source::FirstSeen
    }
}

/// 在等什么：资源类，加上磁盘缺口（需要人清理）或挡住它的排头（CASE-STATUS-4）。
fn waiting_why(adm: &aq_core::admit::AdmitResult, id: &str) -> Vec<String> {
    let mut why: Vec<String> = adm.waiting_for.get(id).map(|k| k.iter().map(|x| x.label()).collect()).unwrap_or_default();
    if let Some(g) = adm.disk_full.get(id) {
        why.push(format!("{} GB short, someone has to free up disk space", gb(g.short_mb())));
    }
    if let Some(head) = adm.blocked_by.get(id) {
        why.push(format!("reserved by head {head}"));
    }
    why
}

fn exec_line(ctx: &Ctx, store: &Store, e: &ExecRow, adm: &aq_core::admit::AdmitResult) -> String {
    let now = ctx.now();
    let fp = e.fingerprint.as_deref().map_or("none (not in git, or --fresh)".to_string(), |f| {
        f.chars().take(12).collect::<String>() + if f.contains("dirty") { "+dirty" } else { "" }
    });
    let mut s = format!("{}  {:<11} {}\n    dir {}  fingerprint {}", e.id, e.status.as_str(), short(&e.argv), e.cwd, fp);
    if let Some(r) = e.res {
        let _ = write!(
            s,
            "\n    booked heavy {} mem {}MB disk {}MB download {}  timeout {}s  estimate from {}",
            r.heavy,
            r.mem_mb,
            r.disk_mb,
            r.uses_net,
            e.timeout_s.unwrap_or(0),
            source(store, e).label()
        );
    } else {
        s.push_str("\n    estimating resources");
    }
    match e.status {
        ExecStatus::Queued => {
            let why = waiting_why(adm, &e.id);
            let _ = write!(
                s,
                "\n    queued {}  waiting for {}  {}",
                dur(now - e.submitted_at),
                if why.is_empty() { "nothing, starts on its turn".into() } else { why.join(", ") },
                runner_state(ctx, &e.id)
            );
        }
        ExecStatus::Running => {
            let alive =
                e.pgid.zip(e.pgid_started_at).is_some_and(|(p, t)| ctx.procs.group_alive(aq_core::model::ProcId { pid: p, started_at: t }));
            let _ = write!(
                s,
                "\n    running {}  process group {} {}  {}",
                dur(now - e.started_at.unwrap_or(now)),
                e.pgid.unwrap_or(0),
                if alive { "alive" } else { "ended" },
                runner_state(ctx, &e.id)
            );
        }
        _ => {
            let _ = write!(
                s,
                "\n    queued {}  ran {}  measured peak mem {}MB avg {:.1} cores",
                dur(e.started_at.unwrap_or(e.submitted_at) - e.submitted_at),
                dur(e.finished_at.unwrap_or(now) - e.started_at.unwrap_or(now)),
                e.peak_mem_mb.unwrap_or(0),
                e.cpu_avg_cores.unwrap_or(0.0)
            );
        }
    }
    if e.started_at.is_some() {
        let _ = write!(s, "\n    log {}", ctx.dirs.log_file(&e.id).display());
    }
    s
}

fn hold_line(ctx: &Ctx, store: &Store, h: &HoldRow) -> AppResult<String> {
    let now = ctx.now();
    let devs = super::view::hold_devices(store.conn(), &h.id)?;
    let devs: Vec<String> = devs.iter().map(|(t, d)| format!("{t} {}", if d.is_empty() { "creating" } else { d })).collect();
    let who = if h.owner == aq_core::model::ProcId::UNKNOWN {
        "unknown".into()
    } else {
        ctx.procs.ancestors(h.owner.pid).first().map(|a| a.1.clone()).unwrap_or_else(|| "exited".into())
    };
    let mut s = format!(
        "{}  {:<9} holder {}(pid {})  devices {}  accounts {}",
        h.id,
        h.status.as_str(),
        who,
        h.owner.pid,
        devs.join(", "),
        h.request.accounts.join(", ")
    );
    if h.status == HoldStatus::Held {
        let used = std::fs::metadata(ctx.dirs.used_file(&h.id)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        let idle = used.map_or("unknown".into(), |d| dur(d.as_millis() as i64));
        let _ = write!(s, "\n    held {}  idle {}", dur(now - h.acquired_at.unwrap_or(now)), idle);
    } else if h.status == HoldStatus::Waiting {
        let _ = write!(s, "\n    waiting {}", dur(now - h.requested_at));
    }
    Ok(s)
}

pub fn overview(ctx: &Ctx, store: &mut Store) -> AppResult<(String, i32)> {
    let ck = check::check(ctx, store)?;
    let sum = super::status::summary(ctx, store)?;
    let g = |k: &str| sum.get(k).and_then(Value::as_i64).unwrap_or(0);
    let mut s = format!(
        "dirs       config {}  state {}\ntasks      running {}   queued {}\nresources  CPU {}%   mem {}%   disk {}%   phones {}%   simulators {}\n",
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
        let _ = writeln!(s, "{}", exec_line(ctx, store, &e, &ck.plan.admit));
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
            let _ = writeln!(s, "{}", exec_line(ctx, store, &e, &ck.plan.admit));
        } else if let Some(h) = aq_store::get_hold(store.conn(), id)? {
            let _ = writeln!(s, "{}", hold_line(ctx, store, &h)?);
        } else {
            let _ = writeln!(s, "{id}  not found (rejected usage)");
        }
    }
    Ok(s)
}
