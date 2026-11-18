//! 故障与恢复：进程被杀、崩溃、重启后的状态和资源。

use std::process::Command;
use std::time::Duration;

use crate::support::*;

fn runner_pid(id: &str) -> Option<String> {
    let o = Command::new("pgrep").args(["-f", &format!("__runner {id}")]).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().next().map(String::from)
}

fn kill9(pid: &str) {
    Command::new("kill").args(["-9", pid]).status().unwrap();
}

fn ppid(pid: &str) -> String {
    let o = Command::new("ps").args(["-o", "ppid=", "-p", pid]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

// CASE-RUN-5
#[test]
fn runner_detached_from_caller() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    let pid = runner_pid(&r.id()).expect("runner 在");
    assert_eq!(ppid(&pid), "1", "runner 的父进程是 launchd");
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
}

// CASE-FAULT-1
#[test]
fn kill_runner_only_command_continues_then_interrupted() {
    let e = Env::new();
    let done = e.path("done");
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "2", "--touch", done.to_str().unwrap()]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    kill9(&runner_pid(&r.id()).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(e.aq(&["status", &r.id()]).status(), "running", "命令还在，继续计占用");
    assert!(until(10, || done.exists()), "命令照常跑完");
    std::thread::sleep(Duration::from_millis(300));
    let s = e.aq(&["status", &r.id()]);
    assert_eq!((s.code, s.status()), (1, "interrupted"));
    assert_eq!(
        e.q_i64(&format!("SELECT exit_code IS NULL AND finished_at IS NOT NULL FROM executions WHERE id='{}'", r.id())),
        1,
        "CASE-STORE-1"
    );
}

// CASE-FAULT-2
#[test]
fn kill_runner_and_command_then_interrupted_and_reclaimed() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "30"]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    let pgid = e.q_i64(&format!("SELECT pgid FROM executions WHERE id='{}'", r.id()));
    kill9(&runner_pid(&r.id()).unwrap());
    Command::new("kill").args(["-9", &format!("-{pgid}")]).status().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(e.aq(&["status", &r.id()]).status(), "interrupted");
    let after = e.aq(&["run", "--", "true"]);
    assert_eq!(after.status(), "ok", "资源已释放");
}

// CASE-RUN-5
#[test]
fn killing_foreground_does_not_stop_task() {
    let e = Env::new();
    let mut fg = e.spawn(&["run", "--", "fake-task", "--secs", "2"]);
    assert!(until(10, || e.q_i64("SELECT count(*) FROM executions") == 1));
    fg.kill().unwrap();
    let _ = fg.wait();
    let id = e.q_str("SELECT id FROM executions").unwrap();
    assert_eq!(e.aq(&["status", &id, "--wait", "20"]).status(), "ok");
    let mut fg = e.spawn(&["hold", "ios-sim"]);
    std::thread::sleep(Duration::from_millis(200));
    fg.kill().unwrap();
    let _ = fg.wait();
    assert!(until(10, || e.q_i64("SELECT count(*) FROM holds WHERE status='held'") == 1), "借用在后台照常完成");
}

// CASE-FAULT-3
#[test]
fn queued_runner_killed_respawned_keeps_position() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    e.set_ai_output(r#"{"heavy": true, "mem_gb": 1, "disk_gb": 0, "net": false, "est_minutes": 1, "reason": "x"}"#);
    let a = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    let b = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    let c = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    assert!(until(10, || e.aq(&["status", &b.id()]).status() == "queued" && runner_pid(&b.id()).is_some()));
    let sub0 = e.q_i64(&format!("SELECT submitted_at FROM executions WHERE id='{}'", b.id()));
    kill9(&runner_pid(&b.id()).unwrap());
    e.aq(&["status"]);
    assert!(until(5, || runner_pid(&b.id()).is_some()), "重新拉起");
    e.aq(&["status", &a.id(), &b.id(), &c.id(), "--wait", "30"]);
    let order: Vec<String> = {
        let db = e.db();
        let mut q = db.prepare("SELECT id FROM executions ORDER BY started_at").unwrap();
        q.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(order, [a.id(), b.id(), c.id()], "排队位置不变");
    assert_eq!(e.q_i64(&format!("SELECT submitted_at FROM executions WHERE id='{}'", b.id())), sub0);
    assert_eq!(e.aq(&["status", &b.id()]).status(), "ok", "不出现 interrupted");
}

// CASE-FAULT-4
#[test]
fn crash_before_commit_reruns_once_after_commit_interrupted() {
    let e = Env::new();
    let count = e.path("count");
    e.env("AQ_TEST_CRASH_AT", "before_commit");
    let r = e.aq(&["run", "--wait", "1", "--", "fake-task", "--count", count.to_str().unwrap()]);
    assert_eq!(r.status(), "queued", "提交前崩溃：仍是 queued");
    // 去掉崩溃注入后，任意一条命令的检查会重拉 runner，最终只执行 1 次
    e.extra.borrow_mut().retain(|(k, _)| k != "AQ_TEST_CRASH_AT");
    let s = e.aq(&["status", &r.id(), "--wait", "20"]);
    assert_eq!(s.status(), "ok", "{}", s.stdout);
    assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1, "命令只执行 1 次");

    let e2 = Env::new();
    let count2 = e2.path("count");
    e2.env("AQ_TEST_CRASH_AT", "after_commit");
    let r2 = e2.aq(&["run", "--wait", "1", "--", "fake-task", "--count", count2.to_str().unwrap()]);
    e2.extra.borrow_mut().clear();
    assert!(until(10, || e2.aq(&["status", &r2.id()]).status() == "interrupted"), "提交后、放行前崩溃：interrupted");
    assert!(!count2.exists(), "命令从未执行");
}

// CASE-STORE-1, CASE-STORE-3
#[test]
fn full_lifecycle_fields_and_terminal_immutable() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let r = e.aq_in(&repo, &["run", "--", "fake-task", "--mem", "20M"]);
    let row: (i64, i64, i64, i64, i64, i64) = e
        .db()
        .query_row(
            &format!("SELECT started_at IS NOT NULL, pgid IS NOT NULL, heavy IS NOT NULL, finished_at IS NOT NULL, exit_code, content_changed FROM executions WHERE id='{}'", r.id()),
            [],
            |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?, x.get(5)?)),
        )
        .unwrap();
    assert_eq!(row, (1, 1, 1, 1, 0, 0));
    assert!(e.q_i64(&format!("SELECT peak_mem_mb FROM executions WHERE id='{}'", r.id())) >= 20);
    assert!(e.db().execute(&format!("UPDATE executions SET status='queued' WHERE id='{}'", r.id()), []).is_err(), "终态不可改");
    assert_eq!(e.aq(&["cancel", &r.id()]).status(), "ok");
    let log = e.aq_log();
    assert!(log.contains("\"to\":\"running\"") && log.contains("\"to\":\"ok\""), "每次状态转移写一行 aq.log");
}

// CASE-RUN-9
#[test]
fn huge_output_capped() {
    let e = Env::new();
    e.env("AQ_T_LOG_CAP_MB", "1");
    let r = e.aq(&["run", "--wait", "30", "--", "fake-task", "--out", "6000000", "--secs", "2"]);
    assert_eq!(r.status(), "ok");
    let len = std::fs::metadata(r.one()["log"].as_str().unwrap()).unwrap().len();
    assert!(len < 3 * 1024 * 1024, "日志截断：{len}");
    let text = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap_or_default();
    assert!(text.contains("[aq] log exceeded") || len < 1024 * 1024 + 70_000);
}

// CASE-RUN-10
#[test]
fn setsid_escapee_not_waited() {
    let e = Env::new();
    let script = "python3 -c 'import os,time; os.setsid(); time.sleep(20)' & exit 0";
    let t = std::time::Instant::now();
    let r = e.aq(&["run", "--wait", "30", "--", "sh", "-c", script]);
    assert_eq!(r.status(), "ok");
    assert!(t.elapsed() < Duration::from_secs(10), "结束时不等脱离进程组的子进程");
    let _ = Command::new("pkill").args(["-f", "os.setsid(); time.sleep(20)"]).status();
}

/// CASE-RUN-7（mock）：运行中「睡眠」2 小时——真实时间跳 2 小时、不含睡眠的时钟不动——不判超时，任务照常结束。
// CASE-RUN-7
#[test]
fn sleep_not_counted_toward_timeout() {
    let e = Env::new();
    let skew = e.path("skew");
    std::fs::write(&skew, "0").unwrap();
    e.env("AQ_TEST_WALL_SKEW_FILE", skew.to_str().unwrap()).env("AQ_T_TIMEOUT_MIN_S", "3").env("AQ_T_TIMEOUT_MAX_S", "3");
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    std::fs::write(&skew, (2 * 3600 * 1000).to_string()).unwrap();
    let s = e.aq(&["status", &r.id(), "--wait", "20"]);
    assert_eq!(s.status(), "ok", "睡眠时间不计入超时：{}", s.stdout);
    let wall = e.q_i64(&format!("SELECT finished_at - started_at FROM executions WHERE id='{}'", r.id()));
    assert!(wall >= 2 * 3600 * 1000, "*_at 仍记真实时间（含睡眠）：{wall}");
}

// CASE-CANCEL-10
#[test]
fn cancel_with_runner_gone_stops_command_itself() {
    let e = Env::new();
    let pids = e.path("pids");
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "60", "--count", pids.to_str().unwrap()]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    assert!(until(10, || pids.exists()));
    kill9(&runner_pid(&r.id()).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    let pid = std::fs::read_to_string(&pids).unwrap().trim().to_string();
    let t = std::time::Instant::now();
    let c = e.aq(&["cancel", &r.id()]);
    assert_eq!((c.code, c.status()), (0, "cancelled"), "{}", c.stdout);
    assert!(t.elapsed() < Duration::from_secs(6));
    let gone = Command::new("kill").args(["-0", &pid]).stderr(std::process::Stdio::null()).status().map(|s| !s.success()).unwrap_or(true);
    assert!(gone, "命令进程已被 cancel 终止");
}
