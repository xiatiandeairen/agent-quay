//! 运行中纠正重任务。按轻任务放行、实际很吃 CPU 的执行，运行满窗口后改记重任务，不中止。
//! 窗口用 AQ_T_HEAVY_WINDOW_S 缩短到 2 秒。

use crate::support::*;

fn env() -> Env {
    let e = Env::new();
    e.env("AQ_T_HEAVY_WINDOW_S", "2");
    e
}

fn ai(e: &Env, heavy: bool) {
    e.set_ai_output(&format!(r#"{{"heavy": {heavy}, "mem_gb": 0.5, "disk_gb": 0, "net": false, "est_minutes": 1, "reason": "x"}}"#));
}

/// 评估完成前 heavy 为空，记作 -1。
fn heavy_of(e: &Env, id: &str) -> i64 {
    e.q_i64(&format!("SELECT IFNULL(heavy, -1) FROM executions WHERE id = '{id}'"))
}

fn reclassified(e: &Env, id: &str) -> bool {
    e.aq_log().lines().any(|l| l.contains(id) && l.contains("now booked as heavy"))
}

/// 同一条命令（命令文本固定，--cpu-from 文件决定这次占几核）。
const CMD: [&str; 5] = ["fake-task", "--cpu-from", "../cores", "--secs", "4"];

fn same_cmd(e: &Env, cores: u32, wait: &str) -> Res {
    std::fs::write(e.path("cores"), cores.to_string()).unwrap();
    let mut args = vec!["run", "--fresh", "--wait", wait, "--"];
    args.extend_from_slice(&CMD);
    e.aq(&args)
}

/// 判轻放行的新命令：AI 评估为轻，实际占 cores 核。
fn hog(e: &Env, cores: u32, extra: &[&str]) -> Res {
    ai(e, false);
    std::fs::write(e.path("cores"), cores.to_string()).unwrap();
    let mut args = vec!["run", "--fresh", "--wait", "0", "--", "fake-task", "--cpu-from", "../cores"];
    args.extend_from_slice(extra);
    e.aq(&args)
}

fn heavy_running(e: &Env) -> i64 {
    e.q_i64("SELECT count(*) FROM executions WHERE status = 'running' AND heavy = 1")
}

// CASE-EST-10, CASE-EST-11
#[test]
fn light_by_history_heavy_now_is_reclassified() {
    let e = env();
    ai(&e, false);
    for _ in 0..3 {
        assert_eq!(same_cmd(&e, 0, "30").status(), "ok");
    }
    // CASE-EST-10：历史 3 次都轻 → 立即放行；约 2 秒后改记重任务，不中止
    let r = same_cmd(&e, 4, "0");
    assert!(until(3, || e.aq(&["status", &r.id()]).status() == "running"), "判轻，立即运行");
    assert_eq!(heavy_of(&e, &r.id()), 0);
    assert!(until(4, || heavy_of(&e, &r.id()) == 1), "窗口满后改记重任务");
    assert!(reclassified(&e, &r.id()), "{}", e.aq_log());
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok", "不中止");
    // CASE-EST-11：最新一次重、之前 3 次轻 → 多数仍轻，再次被纠正；连续两次重之后，提交时就按重任务记账
    let r2 = same_cmd(&e, 4, "30");
    assert_eq!((r2.status(), reclassified(&e, &r2.id())), ("ok", true), "第二次仍判轻，被纠正");
    let r3 = same_cmd(&e, 4, "30");
    assert_eq!((r3.status(), heavy_of(&e, &r3.id())), ("ok", 1));
    assert!(!reclassified(&e, &r3.id()), "一开始就按重任务记账，不需要纠正");
}

// CASE-EST-11
#[test]
fn reclassified_task_blocks_new_heavy_until_slots_free() {
    let e = env();
    ai(&e, true);
    let h1 = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "6"]);
    let h2 = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "6"]);
    assert!(until(10, || heavy_running(&e) == 2));
    let x = hog(&e, 4, &["--secs", "12"]);
    assert!(until(5, || e.aq(&["status", &x.id()]).status() == "running"), "名额满也照样放行（判轻）");
    assert!(until(6, || heavy_of(&e, &x.id()) == 1));
    ai(&e, true);
    let y = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let s = e.aq(&["status", &y.id(), "--human"]);
    assert!(s.stdout.contains("heavy slot"), "{}", s.stdout);
    assert_eq!(heavy_running(&e), 3, "超出 1 个，X 不中止");
    assert_eq!(e.aq(&["status", &y.id(), "--wait", "30"]).status(), "ok");
    let at = |col: &str, id: &str| e.q_i64(&format!("SELECT {col} FROM executions WHERE id = '{id}'"));
    assert!(at("started_at", &y.id()) >= at("finished_at", &h1.id()).max(at("finished_at", &h2.id())), "降到 1 个重任务后 Y 才开始");
    assert_eq!(e.aq(&["status", &x.id(), "--wait", "30"]).status(), "ok", "X 跑完");
}

// CASE-EST-10
#[test]
fn truly_light_task_runs_when_heavy_slots_full() {
    let e = env();
    ai(&e, true);
    for _ in 0..2 {
        e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "8"]);
    }
    assert!(until(10, || heavy_running(&e) == 2));
    let l = hog(&e, 1, &["--secs", "4"]);
    assert!(until(5, || e.aq(&["status", &l.id()]).status() == "running"), "立即运行");
    assert_eq!(e.aq(&["status", &l.id(), "--wait", "20"]).status(), "ok");
    assert_eq!(heavy_of(&e, &l.id()), 0, "1 核不纠正");
    assert!(!reclassified(&e, &l.id()));
}

// CASE-EST-10
#[test]
fn reclassification_never_reverts() {
    let e = env();
    let r = hog(&e, 4, &["--cpu-secs", "3", "--secs", "8"]);
    assert!(until(8, || heavy_of(&e, &r.id()) == 1));
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
    assert_eq!(heavy_of(&e, &r.id()), 1, "后半段不占 CPU 也不改回");
    assert_eq!(e.aq_log().lines().filter(|l| l.contains(&r.id()) && l.contains("now booked as heavy")).count(), 1);
}

// CASE-EST-10
#[test]
fn short_lived_children_are_measured() {
    let e = env();
    ai(&e, false);
    let r = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--burst", "4", "--secs", "6"]);
    assert!(until(10, || heavy_of(&e, &r.id()) == 1), "不断起停的短命子进程也测到 ≥ 2 核：{}", e.aq_log());
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
}

// CASE-EST-10
#[test]
fn cpu_in_other_process_group_is_measured() {
    // xcodebuild 的 SWBBuildService 另开进程组，编译进程都在它下面（CASE-EST-10 实测）
    let e = env();
    let r = hog(&e, 4, &["--other-group", "--secs", "6"]);
    assert!(until(10, || heavy_of(&e, &r.id()) == 1), "子进程另开进程组也测到：{}", e.aq_log());
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
}

// CASE-EST-10
#[test]
fn memory_in_other_process_group_is_counted() {
    let e = env();
    ai(&e, false);
    let r = e.aq(&["run", "--fresh", "--", "fake-task", "--kids", "2", "--kid-mem", "200M", "--other-group", "--secs", "3"]);
    assert_eq!(r.status(), "ok");
    let peak = e.q_i64(&format!("SELECT peak_mem_mb FROM executions WHERE id = '{}'", r.id()));
    assert!(peak >= 380, "2 个各占 200M 的子进程合计计入峰值，实测 {peak}M");
}
