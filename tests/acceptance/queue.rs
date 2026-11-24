//! 排队与放行，以及多个 agent 并发提交。

use std::time::{Duration, Instant};

use crate::support::*;

fn ai(e: &Env, heavy: bool, mem_gb: f64, net: bool) {
    e.set_ai_output(&format!(r#"{{"heavy": {heavy}, "mem_gb": {mem_gb}, "disk_gb": 0, "net": {net}, "est_minutes": 1, "reason": "x"}}"#));
}

fn running(e: &Env) -> i64 {
    e.q_i64("SELECT count(*) FROM executions WHERE status = 'running'")
}

fn sample_max(e: &Env, secs: u64, f: impl Fn(&Env) -> i64) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut m = 0;
    while Instant::now() < deadline {
        m = m.max(f(e));
        if e.q_i64("SELECT count(*) FROM executions WHERE status IN ('queued','running')") == 0 && m > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    m
}

// CASE-QUEUE-1, CASE-QUEUE-4
#[test]
fn at_most_two_heavy_running() {
    let e = Env::new();
    ai(&e, true, 1.0, false);
    let ids: Vec<String> = (0..4).map(|_| e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]).id()).collect();
    assert_eq!(sample_max(&e, 20, running), 2);
    let mut args = vec!["status", "--wait", "30"];
    args.extend(ids.iter().map(String::as_str));
    assert_eq!(e.aq(&args).code, 0, "CASE-QUEUE-4 排队的最终都能跑上");
}

// CASE-QUEUE-2
#[test]
fn book_memory_over_capacity_queues_not_fails() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"4G\""));
    ai(&e, false, 3.0, false);
    let a = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    let b = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    assert_eq!(sample_max(&e, 20, running), 1);
    let s = e.aq(&["status", &a.id(), &b.id(), "--wait", "30"]);
    assert_eq!(s.code, 0, "{}", s.stdout);
}

// CASE-QUEUE-2
#[test]
fn actual_memory_low_queues() {
    let e = Env::new();
    ai(&e, false, 3.0, false);
    e.set_sys(2_000, 500_000);
    let r = e.aq(&["run", "--wait", "1", "--", "fake-task"]);
    assert_eq!(r.status(), "queued");
    let h = e.aq(&["status", &r.id(), "--human"]);
    assert!(h.stdout.contains("waiting for memory"), "{}", h.stdout);
    e.set_sys(30_000, 500_000);
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
}

// CASE-QUEUE-2
#[test]
fn disk_floor_and_net_slots() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("net_slots = 3", "net_slots = 1"));
    e.set_ai_output(r#"{"heavy": false, "mem_gb": 1, "disk_gb": 1, "net": true, "est_minutes": 1, "reason": "x"}"#);
    e.set_sys(30_000, 50 * 1024);
    let r = e.aq(&["run", "--fresh", "--wait", "1", "--", "fake-task", "--secs", "2"]);
    assert_eq!(r.status(), "queued", "磁盘低于底线");
    let s = e.aq(&["status"]);
    assert_eq!(s.lines[0]["disk_pct"], 94, "CASE-QUEUE-2 disk_pct 反映实际（1000000 中剩 51200）");
    e.set_sys(30_000, 500_000);
    let r2 = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    assert_eq!(sample_max(&e, 20, running), 1, "下载名额 1");
    e.aq(&["status", &r.id(), &r2.id(), "--wait", "30"]);
}

// CASE-QUEUE-3
#[test]
fn hold_waiting_for_phone_lets_run_go_but_not_later_phone_hold() {
    let e = Env::new();
    ai(&e, true, 1.0, false);
    let h0 = e.aq(&["hold", "ios-device"]);
    assert_eq!(h0.status(), "held");
    let h1 = e.aq(&["hold", "ios-device", "--wait", "0"]);
    let run = e.aq(&["run", "--wait", "20", "--", "true"]);
    assert_eq!(run.status(), "ok", "不要真机的 run 先跑");
    let h2 = e.aq(&["hold", "ios-device", "--wait", "0"]);
    e.aq(&["release", &h0.id()]);
    assert_eq!(e.aq(&["status", &h1.id(), "--wait", "10"]).status(), "held", "先到的先拿到");
    assert_eq!(e.aq(&["status", &h2.id()]).status(), "waiting", "后来的不插队");
}

// CASE-QUEUE-3
#[test]
fn head_build_waiting_for_memory_blocks_later_sim() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"6G\"").replace("mem = \"3G\"", "mem = \"2G\""));
    ai(&e, false, 4.0, false);
    let a = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    assert!(until(10, || running(&e) == 1));
    let b = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    // 没评估完的执行还不在排队顺位里，等 b 评估完再借，否则机器忙时借用会赶在 b 前面
    let evaluated = format!("SELECT count(*) FROM executions WHERE id = '{}' AND heavy IS NOT NULL", b.id());
    assert!(until(10, || e.q_i64(&evaluated) == 1));
    let h = e.aq(&["hold", "ios-sim", "--wait", "1"]);
    assert_eq!(h.status(), "waiting", "排头缺内存，后来的模拟器不能占走");
    let s = e.aq(&["status", &a.id(), &b.id(), "--wait", "30"]);
    assert_eq!(s.code, 0);
    assert_eq!(e.aq(&["status", &h.id(), "--wait", "10"]).status(), "held");
}

// CASE-CANCEL-4
#[test]
fn cancel_racing_admission_never_leaves_running_command() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    ai(&e, true, 1.0, false);
    for i in 0..5 {
        let blocker = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]);
        let mark = e.path(&format!("ran{i}"));
        let victim = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "30", "--count", mark.to_str().unwrap()]);
        std::thread::sleep(Duration::from_millis(900 + i * 50));
        e.aq(&["cancel", &victim.id()]);
        let v = e.aq(&["status", &victim.id(), "--wait", "20"]);
        assert_eq!(v.status(), "cancelled");
        if mark.exists() {
            let pid = std::fs::read_to_string(&mark).unwrap().trim().to_string();
            assert!(
                until(3, || !std::process::Command::new("kill").args(["-0", &pid]).status().unwrap().success()),
                "状态 cancelled 时命令不能还在跑"
            );
        }
        e.aq(&["status", &blocker.id(), "--wait", "20"]);
    }
}

// CASE-FAULT-8, CASE-QUEUE-1
#[test]
fn ten_agents_mixed_load() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"8G\""));
    ai(&e, true, 1.5, false);
    let kids: Vec<_> = (0..10)
        .map(|i| {
            if i % 4 == 3 {
                e.spawn(&["hold", "ios-sim", "--wait", "60"])
            } else {
                e.spawn(&["run", "--fresh", "--wait", "60", "--", "fake-task", "--secs", "1"])
            }
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut max_heavy = 0;
    let mut max_mem = 0;
    while Instant::now() < deadline {
        max_heavy = max_heavy.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
        let mem = e.q_i64("SELECT coalesce(sum(mem_mb),0) FROM executions WHERE status='running'")
            + 2048 * e.q_i64("SELECT count(*) FROM hold_devices");
        max_mem = max_mem.max(mem);
        if e.q_i64("SELECT count(*) FROM executions WHERE status IN ('queued','running')") == 0
            && e.q_i64("SELECT count(*) FROM executions") >= 7
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let outs: Vec<Res> = kids.into_iter().map(|k| parse(k.wait_with_output().unwrap())).collect();
    assert!(outs.iter().all(|o| o.one().get("reason").is_none_or(|r| r != "busy")), "无 busy 报错");
    assert!(outs.iter().all(|o| ["ok", "held"].contains(&o.status())), "{:?}", outs.iter().map(|o| o.stdout.clone()).collect::<Vec<_>>());
    assert!(max_heavy <= 2, "重任务 {max_heavy}");
    assert!(max_mem <= 8 * 1024, "内存记账 {max_mem}");
    let ok: String = e.db().query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ok, "ok");
    let c = e.db();
    let pragmas: (i64, String) =
        (c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap(), c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap());
    assert_eq!(pragmas.1, "wal", "CASE-FAULT-8");
}

fn ai_disk(e: &Env, mem_gb: f64, disk_gb: f64) {
    e.set_ai_output(&format!(
        r#"{{"heavy": false, "mem_gb": {mem_gb}, "disk_gb": {disk_gb}, "net": false, "est_minutes": 1, "reason": "x"}}"#
    ));
}

fn estimated(e: &Env, id: &str) -> bool {
    until(10, || e.q_i64(&format!("SELECT disk_mb IS NOT NULL FROM executions WHERE id='{id}'")) == 1)
}

// CASE-QUEUE-8, CASE-QUEUE-12, CASE-STATUS-4
#[test]
fn disk_full_head_lets_small_tasks_run_and_resumes_after_cleanup() {
    let e = Env::new();
    e.set_sys(30_000, 57 * 1024);
    ai_disk(&e, 0.5, 12.0);
    let head = e.aq(&["run", "--fresh", "--wait", "0", "--", "sh", "-c", "fake-task --secs 0"]);
    assert!(estimated(&e, &head.id()));
    let s = e.aq(&["status", &head.id()]);
    assert_eq!(s.one()["waiting_for"], serde_json::json!(["disk_full"]));
    let msg = s.one()["message"].as_str().unwrap_or("").to_string();
    assert!(msg.starts_with("Needs 5.0 GB more disk to start: 57.0 GB free, 50.0 GB kept in reserve"), "{msg}");
    assert!(msg.contains("someone has to free up disk space"), "{msg}");
    assert!(s.one().get("blocked_by").is_none());
    let human = e.command(&["status", &head.id(), "--human"]).output().unwrap();
    assert!(String::from_utf8_lossy(&human.stdout).contains("5.0 GB short, someone has to free up disk space"));

    ai_disk(&e, 0.5, 0.18);
    let small = e.aq(&["run", "--fresh", "--wait", "20", "--", "fake-task", "--secs", "0"]);
    assert_eq!(small.status(), "ok", "排头的磁盘等不到，不挡 186MB 的小任务");
    assert_eq!(e.aq(&["status", &head.id()]).status(), "queued");

    e.set_sys(30_000, 500_000);
    assert_eq!(e.wait_status(&head.id(), 20, &["ok"]).status(), "ok", "腾出空间后自动开始，不用重新提交");
}

// CASE-QUEUE-3, CASE-QUEUE-13, CASE-STATUS-4
#[test]
fn blocked_task_reports_head_and_logs_how_long() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"6G\""));
    ai_disk(&e, 4.0, 0.0);
    let a = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    assert!(until(10, || running(&e) == 1));
    let head = e.aq(&["run", "--fresh", "--wait", "0", "--", "sh", "-c", "fake-task --secs 0"]);
    assert!(estimated(&e, &head.id()));
    ai_disk(&e, 1.0, 0.0);
    let c = e.aq(&["run", "--fresh", "--wait", "0", "--", "sh", "-c", "fake-task --secs 0 --exit 0"]);
    assert!(estimated(&e, &c.id()));
    let s = e.aq(&["status", &c.id()]);
    assert_eq!(s.one()["blocked_by"].as_str(), Some(head.id().as_str()), "{}", s.stdout);
    assert_eq!(s.one()["waiting_for"], serde_json::json!(["mem"]));
    let human = e.command(&["status", &c.id(), "--human"]).output().unwrap();
    assert!(String::from_utf8_lossy(&human.stdout).contains(&format!("reserved by head {}", head.id())));
    e.aq(&["status", &a.id(), &head.id(), &c.id(), "--wait", "30"]);
    let log = e.aq_log();
    assert!(
        log.lines()
            .any(|l| l.contains(&c.id()) && l.contains(&format!("blocked by {} on [mem]", head.id())) && l.contains("admitted after")),
        "{log}"
    );
}

// CASE-QUEUE-13
#[test]
fn self_waiting_task_logs_what_it_waited_for() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"6G\""));
    ai_disk(&e, 4.0, 0.0);
    let a = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    assert!(until(10, || running(&e) == 1));
    let b = e.aq(&["run", "--fresh", "--wait", "0", "--", "sh", "-c", "fake-task --secs 0"]);
    assert!(estimated(&e, &b.id()));
    e.aq(&["status", &a.id(), &b.id(), "--wait", "30"]);
    let log = e.aq_log();
    assert!(log.lines().any(|l| l.contains(&b.id()) && l.contains("admitted after") && l.contains("waiting for [mem]")), "{log}");
}
