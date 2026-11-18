//! 状态。输出约定、数据库忙、目录与环境。

use std::time::{Duration, Instant};

use crate::support::*;

// CASE-STATUS-1
#[test]
fn overview_has_seven_fields_and_lists_everything() {
    // 只留 1 台真机（iOS），借走 1 台即 100%
    let e = Env::with_config(&DEFAULT_CONFIG.replace("[android-device]\nids = [\"android-1\"]\n", ""));
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "5"]);
    let h = e.aq(&["hold", "ios-device"]);
    let s = e.aq(&["status"]);
    assert_eq!(s.code, 0);
    let first = s.lines[0].as_object().unwrap();
    let keys: Vec<&str> = first.keys().map(String::as_str).collect();
    assert_eq!(keys, ["running", "queued", "cpu_pct", "mem_pct", "disk_pct", "phone_pct", "sims"]);
    assert_eq!(first["phone_pct"], 100);
    assert_eq!(first["disk_pct"], 50, "覆盖文件里 1000000 剩 500000");
    assert_eq!(first["cpu_pct"], 20, "覆盖文件里的 CPU");
    let ids: Vec<&str> = s.lines[1..].iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [r.id().as_str(), h.id().as_str()]);
    e.aq(&["cancel", &r.id()]);
}

// CASE-STATUS-2
#[test]
fn multi_id_wait_exit_code_is_max() {
    let e = Env::new();
    let ok = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    let bad = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "1", "--exit", "2"]);
    let s = e.aq(&["status", &ok.id(), &bad.id(), "--wait", "30"]);
    assert_eq!(s.lines.len(), 2);
    assert_eq!((s.lines[0]["status"].as_str(), s.lines[1]["status"].as_str()), (Some("ok"), Some("failed")));
    assert_eq!(s.code, 1);
    let slow = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    let t = Instant::now();
    let s = e.aq(&["status", &ok.id(), &slow.id(), "--wait", "1"]);
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(s.code, 2, "未结束的取 2");
    e.aq(&["cancel", &slow.id()]);
}

// CASE-RUN-2
#[test]
fn unknown_id_is_usage() {
    let e = Env::new();
    let s = e.aq(&["status", "ex_nope"]);
    assert_eq!((s.code, s.one()["reason"].as_str()), (3, Some("usage")));
}

// CASE-STATUS-4
#[test]
fn human_same_exit_code() {
    let e = Env::new();
    let r = e.aq(&["run", "--human", "--", "sh", "-c", "exit 3"]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.contains("failed") && r.stdout.contains("日志"), "{}", r.stdout);
    let h = e.aq(&["status", "--human"]);
    assert!(h.stdout.contains("目录") && h.stdout.contains("资源"), "{}", h.stdout);
    let x = e.aq(&["status", "ex_nope", "--human"]);
    assert_eq!(x.code, 3);
}

// CASE-STATUS-3
#[test]
fn fields_appear_only_when_applicable() {
    let e = Env::new();
    let ok = e.aq(&["run", "--", "true"]);
    assert!(ok.one().get("exit_code").is_none() && ok.one().get("retry_after_s").is_none() && ok.one().get("content_changed").is_none());
    let q = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "5"]);
    assert!(q.one().get("retry_after_s").is_some() && q.one().get("exit_code").is_none());
    let c = e.aq(&["cancel", &q.id()]);
    assert_eq!(c.code, 0, "cancel 接受即 0");
    let h = e.aq(&["hold", "ios-device"]);
    assert!(h.one().get("accounts").is_none(), "没借账号不出现 accounts");
    assert!(h.one().get("retry_after_s").is_none());
    let w = e.aq(&["hold", "ios-device", "--wait", "0"]);
    assert!(w.one().get("devices").is_none() && w.one().get("retry_after_s").is_some());
    assert_eq!(e.aq(&["release", &w.id()]).code, 0);
    assert_eq!(e.aq(&["release", &h.id()]).code, 0);
    assert_eq!(e.aq(&["status", &h.id()]).code, 0, "released → 0");
}

// CASE-FAULT-9
#[test]
fn write_lock_held_over_5s_is_busy_and_unchanged() {
    let e = Env::new();
    e.aq(&["status"]);
    let c = e.db();
    c.execute_batch("BEGIN IMMEDIATE").unwrap();
    let t = Instant::now();
    let r = e.aq(&["run", "--", "true"]);
    assert!(t.elapsed() >= Duration::from_secs(5));
    assert_eq!((r.code, r.one()["reason"].as_str()), (1, Some("busy")), "{}", r.stdout);
    c.execute_batch("ROLLBACK").unwrap();
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 0, "数据不变");
}

// CASE-FAULT-7
#[test]
fn db_deleted_recreated_and_unwritable_state_reports_path() {
    let e = Env::new();
    e.aq(&["run", "--", "true"]);
    for f in ["aq.db", "aq.db-wal", "aq.db-shm"] {
        let _ = std::fs::remove_file(e.state().join(f));
    }
    assert_eq!(e.aq(&["run", "--", "true"]).status(), "ok", "下一条命令重建空库");
    let ro = e.path("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
    e.env("XDG_STATE_HOME", ro.to_str().unwrap());
    let r = e.aq(&["status"]);
    std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    assert_eq!(r.code, 1);
    let msg = r.one()["message"].as_str().unwrap();
    assert!(msg.contains(ro.to_str().unwrap()) && msg.contains("沙箱"), "CASE-FAULT-7 给出需要放行的路径：{msg}");
}

// CASE-STATUS-4
#[test]
fn human_shows_state_dir() {
    let a = Env::new();
    let b = Env::new();
    let ha = a.aq(&["status", "--human"]).stdout;
    let hb = b.aq(&["status", "--human"]).stdout;
    assert!(ha.contains(a.state().to_str().unwrap()));
    assert_ne!(ha.lines().next(), hb.lines().next());
}

// CASE-STATUS-5
#[test]
fn bad_config_line_is_usage_with_line_number() {
    let e = Env::new();
    e.write_config("heavy_slots = 2\nheavy_slot = 3\n");
    for args in [&["status"][..], &["run", "--", "true"][..], &["hold", "ios-sim"][..]] {
        let r = e.aq(args);
        assert_eq!(r.code, 3);
        assert!(r.one()["message"].as_str().unwrap().contains("line 2"), "{}", r.stdout);
    }
}

// CASE-STATUS-6
#[test]
fn top_draws_one_frame_when_not_a_terminal() {
    let e = Env::new();
    let q = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "5"]);
    let t = e.aq(&["status", "--top"]);
    assert_eq!(t.code, 0);
    assert!(!t.stdout.contains('\x1b'), "非终端不着色、不重画：{:?}", t.stdout);
    assert!(t.stdout.starts_with("aq · running") && t.stdout.contains("HEAVY") && t.stdout.contains("TASKS"), "{}", t.stdout);
    assert!(t.stdout.contains("fake-task --secs 5") && t.stdout.contains("nothing borrowed"), "{}", t.stdout);
    for args in [&["status", "--top", &q.id()][..], &["status", "--top", "--human"][..], &["status", "--top", "--wait", "5"][..]] {
        let r = e.aq(args);
        assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("usage")), "{args:?}");
    }
    assert_eq!(e.aq(&["cancel", &q.id()]).code, 0);
}
