//! 监视运行中的命令：采样进程组内存、进程树 CPU（判轻实重时改记重任务），核对超时（不含睡眠时间）和撤销，限制单个日志大小。
//! 终止：先 SIGTERM 整个进程组，宽限期后 SIGKILL；撤销要等进程组全部结束才写 cancelled。

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

use aq_core::cpu_watch::CpuWatch;
use aq_core::model::ExecStatus;
use aq_core::ports::{Clock, Procs};
use aq_store::{ExecRow, Store};
use aq_sys::spawn::{group_empty, killpg, Exit, Gate, SIGKILL, SIGTERM};

use super::ctx::Ctx;
use super::error::AppResult;

pub struct Outcome {
    pub status: ExecStatus,
    pub exit_code: Option<i64>,
    pub peak_mem_mb: i64,
    pub cpu_avg_cores: f64,
    pub disk_delta_mb: i64,
    pub why: String,
}

const SAMPLE: Duration = Duration::from_millis(500);

fn cap_log(ctx: &Ctx, e: &ExecRow) {
    let p = ctx.dirs.log_file(&e.id);
    let too_big = std::fs::metadata(&p).map(|m| m.len() > ctx.tuning.log_cap_bytes).unwrap_or(false);
    if !too_big {
        return;
    }
    // 命令以追加方式写日志，清空后继续写在文件末尾（CASE-RUN-9、CASE-STATUS-5）
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&p) {
        let mb = ctx.tuning.log_cap_bytes / (1024 * 1024);
        let _ = f.set_len(0).and_then(|_| f.write_all(format!("[aq] 日志超过 {mb}MB，之前的内容已清空\n").as_bytes()));
        // 截断失败下一秒再试
    }
}

/// SIGTERM → 宽限期 → SIGKILL，等组长退出、进程组清空。
fn terminate(ctx: &Ctx, gate: &mut Gate, pgid: i64) -> Option<Exit> {
    killpg(pgid, SIGTERM);
    let deadline = Instant::now() + ctx.tuning.kill_grace;
    let mut st = None;
    while Instant::now() < deadline {
        if st.is_none() {
            st = gate.reap(false).ok().flatten();
        }
        if st.is_some() && group_empty(pgid) {
            return st;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    killpg(pgid, SIGKILL);
    if st.is_none() {
        st = gate.reap(true).ok().flatten();
    }
    let until = Instant::now() + Duration::from_secs(5);
    while !group_empty(pgid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    st
}

/// 运行中纠正：按轻任务放行的执行，窗口内平均 ≥ 2 核就改记重任务，之后的重任务按名额排队（不中止它）。
fn cpu_tick(ctx: &Ctx, store: &Store, e: &ExecRow, pgid: i64, w: &mut CpuWatch, light: &mut bool) -> AppResult<()> {
    w.observe(ctx.clock.uptime_ms(), &ctx.procs.group_cpu_ns(pgid));
    if *light && w.is_heavy() {
        *light = false;
        if aq_store::mark_heavy(store.conn(), &e.id)? {
            let cores = w.window_cores().unwrap_or(0.0);
            ctx.log_note(&e.id, &format!("运行中实测 {cores:.1} 核 ≥ 2，改记重任务（占重任务名额，不中止）"));
        }
    }
    Ok(())
}

fn code_of(s: std::process::ExitStatus) -> i64 {
    s.code().map_or_else(|| 128 + i64::from(s.signal().unwrap_or(0)), i64::from)
}

pub fn watch(ctx: &Ctx, store: &Store, e: &ExecRow, mut gate: Gate) -> AppResult<Outcome> {
    let pgid = gate.pid();
    let start_up = ctx.clock.uptime_ms();
    let start_wall = Instant::now();
    let timeout_ms = e.timeout_s.unwrap_or(ctx.limits.timeout_max_s) * 1000;
    let mut peak = 0i64;
    let mut last_poll = Instant::now();
    let mut rounds = 0u32;
    let mut cpu = CpuWatch::new(ctx.tuning.heavy_window_ms);
    let mut light = e.res.is_some_and(|r| !r.heavy);
    let (status, code, why, exit) = loop {
        if let Ok(Some(x)) = gate.reap(false) {
            let c = code_of(x.status);
            let st = if c == 0 { ExecStatus::Ok } else { ExecStatus::Failed };
            break (st, Some(c), format!("命令退出码 {c}"), Some(x));
        }
        peak = peak.max(ctx.procs.group_mem_mb(pgid));
        cpu_tick(ctx, store, e, pgid, &mut cpu, &mut light)?;
        if ctx.clock.uptime_ms() - start_up > timeout_ms {
            let x = terminate(ctx, &mut gate, pgid);
            break (ExecStatus::Timeout, None, format!("超过超时 {}s，已终止", timeout_ms / 1000), x);
        }
        if last_poll.elapsed() >= ctx.tuning.poll {
            last_poll = Instant::now();
            cap_log(ctx, e);
            let cancel = aq_store::get_exec(store.conn(), &e.id)?.is_some_and(|r| r.cancel_requested);
            if cancel {
                let x = terminate(ctx, &mut gate, pgid);
                break (ExecStatus::Cancelled, None, "运行中被撤销，进程组已结束".into(), x);
            }
        }
        std::thread::sleep(Duration::from_millis(20u64 << rounds.min(5)).min(SAMPLE).min(ctx.tuning.poll));
        rounds += 1;
    };
    // 只取命令这一个进程的用量（wait4）；回收失败时只用采样值（CASE-EST-8）
    let (cpu_s, maxrss_mb) = exit.map_or((0.0, 0), |x| (x.cpu_s, x.maxrss_mb));
    let wall = start_wall.elapsed().as_secs_f64().max(0.001);
    let exit_code = if matches!(status, ExecStatus::Ok | ExecStatus::Failed) { code } else { None };
    Ok(Outcome {
        status,
        exit_code,
        peak_mem_mb: peak.max(maxrss_mb),
        cpu_avg_cores: (cpu_s / wall).max(cpu.overall_cores().unwrap_or(0.0)),
        disk_delta_mb: 0,
        why,
    })
}
