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
    assert!(h.stdout.contains("在等 内存"), "{}", h.stdout);
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
