//! status --top：给演示和旁观者看的英文仪表盘（CASE-STATUS-6）。终端里每 0.5 秒原地重画，Ctrl-C 退出；
//! 输出不是终端时只画一帧。占用比例取自 status::summary，和 aq status 的 JSON 同源。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Duration;

use aq_core::admit::Kind;
use aq_core::estimate::Source;
use aq_core::model::{ExecStatus, HoldStatus, ProcId};
use aq_store::{ExecRow, HoldRow, Store};
use serde_json::Value;

use super::ctx::Ctx;
use super::error::AppResult;

/// 一个活动的执行或借用。
pub struct Row {
    pub id: String,
    pub status: String,
    pub secs: i64,
    pub what: String,
    pub note: String,
}

/// 一帧要画的全部数据。
pub struct Frame {
    pub running: i64,
    pub queued: i64,
    pub held: i64,
    pub cpu_pct: i64,
    pub mem_pct: i64,
    pub disk_pct: i64,
    pub heavy_used: i64,
    pub heavy_slots: i64,
    pub tasks: Vec<Row>,
    pub holds: Vec<Row>,
}

const BAR: usize = 20;
/// 一个仪表的可见宽度：标签 6 + 方括号和条 22 + 空格 1 + 数值 5。
const GAUGE_W: usize = 34;

pub fn run(ctx: &Ctx, store: &mut Store) -> AppResult<i32> {
    let tty = aq_sys::term::is_tty();
    let mut out = std::io::stdout();
    if tty && out.write_all(b"\x1b[2J").is_err() {
        return Ok(0);
    }
    loop {
        let text = render(&collect(ctx, store)?, aq_sys::term::width().unwrap_or(100), tty);
        if !tty {
            print!("{text}");
            return Ok(0);
        }
        // 原地重画：回到左上角，每行清掉行尾残留，最后清掉多出来的旧行，不闪屏
        let mut buf = String::from("\x1b[H");
        for l in text.lines() {
            buf.push_str(l);
            buf.push_str("\x1b[K\n");
        }
        buf.push_str("\x1b[J");
        if out.write_all(buf.as_bytes()).and_then(|()| out.flush()).is_err() {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn collect(ctx: &Ctx, store: &mut Store) -> AppResult<Frame> {
    let ck = super::check::check(ctx, store)?;
    let sum = super::status::summary(ctx, store)?;
    let g = |k: &str| sum.get(k).and_then(Value::as_i64).unwrap_or(0);
    let now = ctx.now();
    let execs = aq_store::active_execs(store.conn())?;
    let heavy_used = execs.iter().filter(|e| e.status == ExecStatus::Running && e.res.is_some_and(|r| r.heavy)).count() as i64;
    let tasks = execs.iter().map(|e| task_row(store, e, now, &ck.plan.admit.waiting_for)).collect();
    let holds = aq_store::active_holds(store.conn())?.iter().map(|h| hold_row(store, h, now)).collect::<AppResult<Vec<_>>>()?;
    Ok(Frame {
        running: g("running"),
        queued: g("queued"),
        held: holds.iter().filter(|r| r.status == HoldStatus::Held.as_str()).count() as i64,
        cpu_pct: g("cpu_pct"),
        mem_pct: g("mem_pct"),
        disk_pct: g("disk_pct"),
        heavy_used,
        heavy_slots: ctx.config.cap.heavy_slots,
        tasks,
        holds,
    })
}

fn kind_en(k: &Kind) -> String {
    match k {
        Kind::Heavy => "heavy slot".into(),
        Kind::Mem => "memory".into(),
        Kind::Disk => "disk".into(),
        Kind::Net => "download slot".into(),
        Kind::Device(t) => t.to_string(),
        Kind::Account(a) => format!("account {a}"),
    }
}

fn task_row(store: &Store, e: &ExecRow, now: i64, waiting: &HashMap<String, Vec<Kind>>) -> Row {
    let (since, note) = if e.status == ExecStatus::Queued {
        let why: Vec<String> = waiting.get(&e.id).map(|k| k.iter().map(kind_en).collect()).unwrap_or_default();
        (e.submitted_at, if why.is_empty() { "next up".into() } else { format!("wait: {}", why.join(", ")) })
    } else {
        let src = match super::human::source(store, e) {
            Source::Measured => "measured",
            Source::Ai => "AI",
            Source::Default => "default",
        };
        (e.started_at.unwrap_or(now), format!("estimate: {src}"))
    };
    Row { id: e.id.clone(), status: e.status.as_str().into(), secs: (now - since) / 1000, what: e.argv.join(" "), note }
}

fn hold_row(store: &Store, h: &HoldRow, now: i64) -> AppResult<Row> {
    let devs = super::view::hold_devices(store.conn(), &h.id)?;
    let mut what: Vec<String> = devs
        .iter()
        .map(|(t, d)| if d.is_empty() { format!("{t} (creating)") } else { format!("{t} {}", d.chars().take(8).collect::<String>()) })
        .collect();
    what.extend(h.request.accounts.iter().map(|a| format!("account {a}")));
    let note = if h.owner == ProcId::UNKNOWN { "owner unidentified".into() } else { format!("owner pid {}", h.owner.pid) };
    let since = if h.status == HoldStatus::Held { h.acquired_at.unwrap_or(now) } else { h.requested_at };
    Ok(Row { id: h.id.clone(), status: h.status.as_str().into(), secs: (now - since) / 1000, what: what.join(", "), note })
}

struct Paint(bool);

impl Paint {
    fn sgr(&self, code: &str, s: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn by_pct(&self, pct: i64, s: &str) -> String {
        self.sgr(
            if pct >= 95 {
                "31"
            } else if pct >= 80 {
                "33"
            } else {
                "32"
            },
            s,
        )
    }
    fn by_status(&self, status: &str, s: &str) -> String {
        match status {
            "running" => self.sgr("32", s),
            "queued" => self.sgr("33", s),
            "held" => self.sgr("36", s),
            "waiting" => self.sgr("35", s),
            _ => s.to_string(),
        }
    }
}

fn gauge(p: &Paint, label: &str, n: i64, d: i64, value: &str) -> String {
    let pct = if d <= 0 { 0 } else { (n * 100 / d).clamp(0, 100) };
    let fill = (pct as usize * BAR + 50) / 100;
    let bar = format!("{}{}", "█".repeat(fill), "░".repeat(BAR - fill));
    format!("{label:<6}[{}] {value:>5}", p.by_pct(pct, &bar))
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        format!("{s:<n$}")
    } else {
        format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

fn clock(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn table(s: &mut String, p: &Paint, title: &str, rows: &[Row], width: usize, empty: &str) {
    const FIXED: usize = 2 + 10 + 10 + 9;
    let rest = width.saturating_sub(FIXED).max(24);
    let what_w = (rest * 45 / 100).max(12);
    let note_w = rest.saturating_sub(what_w + 2).max(8);
    let _ = writeln!(s, "{}", p.sgr("1", title));
    if rows.is_empty() {
        let _ = writeln!(s, "  {}", p.sgr("2", empty));
        return;
    }
    let head = format!("  {:<10}{:<10}{:>7}  {}  {}", "ID", "STATUS", "TIME", cut("COMMAND / DEVICE", what_w), "NOTE");
    let _ = writeln!(s, "{}", p.sgr("2", head.trim_end()));
    for r in rows {
        let id: String = r.id.chars().take(3).chain(r.id.chars().skip(r.id.chars().count().saturating_sub(6))).collect();
        let status = p.by_status(&r.status, &format!("{:<10}", r.status));
        let line = format!("  {id:<10}{status}{:>7}  {}  {}", clock(r.secs), cut(&r.what, what_w), cut(&r.note, note_w));
        let _ = writeln!(s, "{}", line.trim_end());
    }
}

pub fn render(f: &Frame, width: usize, color: bool) -> String {
    let p = Paint(color);
    let mut s = String::new();
    let head = format!("aq · running {} · queued {} · held {}", f.running, f.queued, f.held);
    let hint = "Ctrl-C to quit";
    let pad = width.saturating_sub(head.chars().count() + hint.len()).max(2);
    let _ = writeln!(s, "{}{}{}", p.sgr("1", &head), " ".repeat(pad), p.sgr("2", hint));
    let gauges = [
        gauge(&p, "CPU", f.cpu_pct, 100, &format!("{}%", f.cpu_pct)),
        gauge(&p, "MEM", f.mem_pct, 100, &format!("{}%", f.mem_pct)),
        gauge(&p, "DISK", f.disk_pct, 100, &format!("{}%", f.disk_pct)),
        gauge(&p, "HEAVY", f.heavy_used, f.heavy_slots, &format!("{}/{}", f.heavy_used, f.heavy_slots)),
    ];
    let per_line = if width >= 2 * GAUGE_W + 4 { 2 } else { 1 };
    for pair in gauges.chunks(per_line) {
        let _ = writeln!(s, "{}", pair.join("    "));
    }
    s.push('\n');
    table(&mut s, &p, "TASKS", &f.tasks, width, "no tasks");
    s.push('\n');
    table(&mut s, &p, "HOLDS", &f.holds, width, "nothing borrowed");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, status: &str, what: &str, note: &str) -> Row {
        Row { id: id.into(), status: status.into(), secs: 75, what: what.into(), note: note.into() }
    }

    fn frame() -> Frame {
        Frame {
            running: 1,
            queued: 1,
            held: 1,
            cpu_pct: 97,
            mem_pct: 40,
            disk_pct: 85,
            heavy_used: 1,
            heavy_slots: 2,
            tasks: vec![
                row("ex_01m7ngpn0nz24jpj1bwqs808jc", "running", "./build.sh", "estimate: measured"),
                row("ex_01m7ngpn0nz24jpj1bwqw589x0", "queued", "./test.sh", "wait: heavy slot"),
            ],
            holds: vec![row("hd_01m7nh7rvj937frahkt556b2qw", "held", "ios-sim 770B7751", "owner pid 3746")],
        }
    }

    // CASE-STATUS-6
    #[test]
    fn frame_shows_gauges_and_one_row_per_object() {
        let s = render(&frame(), 120, false);
        assert!(!s.contains('\x1b'), "不着色时没有控制码：{s}");
        assert!(s.starts_with("aq · running 1 · queued 1 · held 1"));
        let cpu = s.lines().find(|l| l.starts_with("CPU")).unwrap_or_default();
        assert!(cpu.contains(&format!("[{}{}]", "█".repeat(19), "░")) && cpu.contains("MEM"), "宽屏两个仪表一行：{cpu}");
        assert!(s.contains(&format!("[{}{}]   1/2", "█".repeat(10), "░".repeat(10))), "重任务名额条：{s}");
        assert!(s.contains("ex_s808jc") && s.contains("1:15") && s.contains("wait: heavy slot"), "{s}");
        assert!(s.contains("hd_56b2qw") && s.contains("ios-sim 770B7751") && s.contains("owner pid 3746"), "{s}");
    }

    // CASE-STATUS-6
    #[test]
    fn narrow_terminal_stacks_gauges_and_fits_width() {
        let mut f = frame();
        f.tasks[0].what = "xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build".into();
        let s = render(&f, 60, false);
        assert!(s.lines().all(|l| l.chars().count() <= 60), "每行不超过终端宽度：\n{s}");
        assert!(s.lines().any(|l| l.starts_with("CPU") && !l.contains("MEM")), "窄屏一个仪表一行：\n{s}");
        assert!(s.contains("xcodebuild -…") && s.contains('…'), "长命令截断：\n{s}");
        let empty = Frame { tasks: vec![], holds: vec![], ..frame() };
        let e = render(&empty, 60, false);
        assert!(e.contains("no tasks") && e.contains("nothing borrowed"), "{e}");
    }

    // CASE-STATUS-6
    #[test]
    fn color_marks_status_and_high_usage() {
        let s = render(&frame(), 120, true);
        assert!(s.contains("\x1b[31m█"), "CPU 97% 标红：{s:?}");
        assert!(s.contains("\x1b[33m█"), "磁盘 85% 标黄：{s:?}");
        assert!(s.contains("\x1b[32mrunning") && s.contains("\x1b[33mqueued") && s.contains("\x1b[36mheld"), "{s:?}");
    }
}
