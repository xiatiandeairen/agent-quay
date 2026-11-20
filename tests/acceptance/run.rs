//! 提交与执行。撤销、边缘场景。

use std::time::{Duration, Instant};

use crate::support::*;

// CASE-RUN-2
#[test]
fn run_without_command_is_usage() {
    let e = Env::new();
    for args in [&["run"][..], &["run", "true"][..], &["run", "--"][..]] {
        let r = e.aq(args);
        assert_eq!((r.code, r.status()), (3, "rejected"), "{args:?}: {}", r.stdout);
        assert_eq!(r.one()["reason"], "usage");
    }
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 0);
}

// CASE-RUN-1
#[test]
fn true_ok_with_log() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "true"]);
    assert_eq!((r.code, r.status()), (0, "ok"), "{}", r.stdout);
    assert!(r.one().get("reused").is_none(), "没共用时不带 reused");
    assert!(std::path::Path::new(r.one()["log"].as_str().unwrap()).exists());
    let keys: Vec<&String> = r.one().as_object().unwrap().keys().collect();
    assert_eq!(keys, ["id", "status", "log"], "CASE-STATUS-3 字段不多不少");
}

// CASE-RUN-1
#[test]
fn exit_65_failed() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "sh", "-c", "exit 65"]);
    assert_eq!((r.code, r.status(), r.one()["exit_code"].as_i64()), (1, "failed", Some(65)));
}

// CASE-RUN-4
#[test]
fn wait0_returns_fast_with_retry_hint() {
    let e = Env::new();
    let t = Instant::now();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "5"]);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert_eq!(r.code, 2);
    assert!(["queued", "running"].contains(&r.status()));
    let ra = r.one()["retry_after_s"].as_i64().unwrap();
    assert!((10..=300).contains(&ra));
    e.aq(&["cancel", &r.id()]);
}

// CASE-RUN-4
#[test]
fn wait_returns_running_then_status_gets_ok() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "1", "--", "fake-task", "--secs", "3"]);
    assert_eq!((r.code, r.status()), (2, "running"), "{}", r.stdout);
    let s = e.aq(&["status", &r.id(), "--wait", "20"]);
    assert_eq!((s.code, s.status()), (0, "ok"));
}

// CASE-RUN-3
#[test]
fn shell_pipeline_is_one_task() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "sh", "-c", "echo a | wc -l"]);
    assert_eq!(r.status(), "ok");
    let log = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap();
    assert_eq!(log.trim(), "1");
}

// CASE-RUN-3
#[test]
fn runs_in_calling_directory() {
    let e = Env::new();
    let sub = e.work().join("Modules");
    std::fs::create_dir_all(&sub).unwrap();
    let r = e.aq_in(&sub, &["run", "--", "pwd"]);
    let log = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap();
    assert_eq!(std::fs::canonicalize(log.trim()).unwrap(), std::fs::canonicalize(&sub).unwrap());
}

// CASE-REUSE-4
#[test]
fn non_git_same_command_twice_not_merged() {
    let e = Env::new();
    let a = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    let b = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    assert_ne!(a.id(), b.id());
    assert!(b.one().get("reused").is_none(), "没共用时不带 reused");
    assert_eq!(e.q_i64("SELECT count(*) FROM executions WHERE hash IS NULL"), 2);
    let s = e.aq(&["status", &a.id(), &b.id(), "--wait", "20"]);
    assert_eq!(s.code, 0);
}

// CASE-RUN-6
#[test]
fn timeout_kills_whole_group() {
    let e = Env::new();
    e.env("AQ_T_TIMEOUT_MIN_S", "2").env("AQ_T_TIMEOUT_MAX_S", "2");
    let marker = e.path("child-alive");
    let script = format!("sleep 30 & sleep 30; touch {}", marker.display());
    let r = e.aq(&["run", "--wait", "20", "--", "sh", "-c", &script]);
    assert_eq!((r.code, r.status()), (1, "timeout"), "{}", r.stdout);
    assert!(r.one().get("exit_code").is_none());
    let out = std::process::Command::new("pgrep").args(["-f", "sleep 30"]).output().unwrap();
    let _ = out; // 其他测试也可能有 sleep 30；用进程组判断：
    let pgid = e.q_i64(&format!("SELECT pgid FROM executions WHERE id='{}'", r.id()));
    assert!(until(5, || std::process::Command::new("pgrep").args(["-g", &pgid.to_string()]).output().unwrap().stdout.is_empty()));
    assert!(!marker.exists());
}

// CASE-CANCEL-1
#[test]
fn cancel_queued_never_runs() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    let first = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "4"]);
    let count = e.path("ran");
    let second = e.aq(&["run", "--wait", "0", "--", "fake-task", "--count", count.to_str().unwrap()]);
    let r = e.aq(&["cancel", &second.id()]);
    assert_eq!(r.code, 0);
    assert_eq!(e.aq(&["status", &second.id()]).status(), "cancelled");
    e.aq(&["status", &first.id(), "--wait", "20"]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!count.exists(), "被撤销的任务不应执行");
}

// CASE-CANCEL-2
#[test]
fn cancel_running_ends_group_and_sharers_see_cancelled() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let a = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "30"]);
    let b = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "30"]);
    assert_eq!(b.one()["reused"], true);
    assert!(until(10, || e.aq(&["status", &a.id()]).status() == "running"));
    let t = Instant::now();
    assert_eq!(e.aq(&["cancel", &a.id()]).code, 0);
    let s = e.aq(&["status", &a.id(), &b.id(), "--wait", "15"]);
    assert!(t.elapsed() < Duration::from_secs(10));
    assert!(s.lines.iter().all(|l| l["status"] == "cancelled"), "{}", s.stdout);
}

// CASE-CANCEL-3
#[test]
fn cancel_finished_returns_as_is() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "true"]);
    let c = e.aq(&["cancel", &r.id()]);
    assert_eq!((c.code, c.status()), (0, "ok"));
}

// CASE-RUN-2
#[test]
fn cancel_unknown_is_usage() {
    let e = Env::new();
    let c = e.aq(&["cancel", "ex_nope"]);
    assert_eq!((c.code, c.one()["reason"].as_str()), (3, Some("usage")));
}

// CASE-RUN-1
#[test]
fn command_not_found_is_127() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "no-such-command-aq"]);
    assert_eq!((r.status(), r.one()["exit_code"].as_i64()), ("failed", Some(127)));
    let log = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap();
    assert!(log.contains("no-such-command-aq"), "{log}");
}

// CASE-RUN-8
#[test]
fn nested_run_executes_directly() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    let r = e.aq(&["run", "--wait", "20", "--", "sh", "-c", "aq run -- sh -c 'echo inner'"]);
    assert_eq!(r.status(), "ok", "{}", r.stdout);
    let log = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap();
    assert!(log.contains("inner"));
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 1, "内层不排队、不入库");
}

// CASE-RUN-4
#[test]
fn wait_zero_and_very_long() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    assert_eq!(r.code, 2);
    let r = e.aq(&["run", "--wait", "86400", "--", "fake-task", "--secs", "1", "--exit", "0", "--mem", "1M"]);
    assert_eq!(r.status(), "ok");
}

// CASE-RUN-2
#[test]
fn status_of_pruned_id_is_usage() {
    let e = Env::new();
    e.env("AQ_T_KEEP", "1");
    let a = e.aq(&["run", "--", "true"]);
    std::thread::sleep(Duration::from_millis(20));
    e.aq(&["run", "--", "sh", "-c", "true"]);
    let s = e.aq(&["status", &a.id()]);
    assert_eq!((s.code, s.one()["reason"].as_str()), (3, Some("usage")), "{}", s.stdout);
}

/// 忽略 SIGTERM、睡 60 秒的命令（只能被 SIGKILL 结束）；装好信号处理后建 argv[1] 这个文件。
const IGNORE_TERM: &str =
    "import signal, sys, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); open(sys.argv[1], 'w').close(); time.sleep(60)";

/// 提交 IGNORE_TERM，等到它已经在忽略 SIGTERM。
fn run_ignoring_term(e: &Env, env: &[(&str, &str)]) -> Res {
    let ready = e.path(&format!("ready-{}", std::process::id()));
    let _ = std::fs::remove_file(&ready);
    let r = aq_with(e, env, &["run", "--fresh", "--wait", "0", "--", "python3", "-c", IGNORE_TERM, ready.to_str().unwrap()]);
    assert!(until(10, || ready.exists()), "命令没起来：{}", r.stdout);
    r
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// 用单独的环境变量跑一次 aq（只影响这一个进程，比如 runner 和 cancel 用不同的宽限期）。
fn aq_with(e: &Env, env: &[(&str, &str)], args: &[&str]) -> Res {
    let mut c = e.command(args);
    c.envs(env.iter().copied()).stdin(std::process::Stdio::null());
    parse(c.output().expect("aq"))
}

fn transitions_to(e: &Env, id: &str, to: &str) -> usize {
    e.aq_log().lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|v| v["id"] == id && v["to"] == to).count()
}

// CASE-CANCEL-2, CASE-STATUS-3
#[test]
fn cancel_returns_after_group_is_gone() {
    let e = Env::new();
    let pids = e.path("pids");
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "30", "--count", pids.to_str().unwrap()]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    assert!(until(10, || pids.exists()));
    let pid = std::fs::read_to_string(&pids).unwrap().trim().to_string();
    let c = e.aq(&["cancel", &r.id()]);
    assert_eq!((c.code, c.status()), (0, "cancelled"), "{}", c.stdout);
    assert!(c.one().get("cancel_requested").is_none(), "到终态后不带 cancel_requested");
    assert!(!alive(&pid), "cancel 返回时命令进程已结束");
}

// CASE-CANCEL-5
#[test]
fn cancel_waits_through_sigkill_for_term_ignoring_command() {
    let e = Env::new();
    let r = run_ignoring_term(&e, &[]);
    let t = Instant::now();
    let c = e.aq(&["cancel", &r.id()]);
    assert_eq!((c.code, c.status()), (0, "cancelled"), "{}", c.stdout);
    let took = t.elapsed();
    assert!(took >= Duration::from_millis(900) && took < Duration::from_secs(6), "宽限期 1 秒后 SIGKILL：{took:?}");
}

// CASE-CANCEL-6, CASE-STATUS-3
#[test]
fn cancel_gives_up_waiting_with_cancel_requested() {
    let e = Env::new();
    // runner 的宽限期 20 秒；cancel 自己只等 1 + 5 秒
    let r = run_ignoring_term(&e, &[("AQ_T_KILL_GRACE_MS", "20000")]);
    let t = Instant::now();
    let c = e.aq(&["cancel", &r.id()]);
    assert!(t.elapsed() < Duration::from_secs(8));
    assert_eq!((c.code, c.status()), (2, "running"), "{}", c.stdout);
    assert_eq!(c.one()["cancel_requested"], true);
    assert_eq!(e.wait_status(&r.id(), 30, &["cancelled"]).status(), "cancelled");
}

// CASE-CANCEL-7
#[test]
fn cancel_racing_natural_exit_reports_real_outcome() {
    let e = Env::new();
    // runner 5 秒才查一次撤销请求（排队轮询也是 5 秒），命令开始后 1 秒就自己结束：撤销请求还没被处理，命令已经退出
    e.set_sys_with(serde_json::json!({"pressure": "normal", "swapping": false})); // 不受本机真实内存压力影响
    let started = e.path("started");
    let script = format!("touch {}; sleep 1", started.display());
    let r = aq_with(&e, &[("AQ_T_POLL_MS", "5000")], &["run", "--wait", "0", "--", "sh", "-c", &script]);
    assert!(until(20, || started.exists()), "命令没起来：{}", r.stdout);
    let c = e.aq(&["cancel", &r.id()]);
    assert_eq!((c.code, c.status()), (0, "ok"), "{}", c.stdout);
}

// CASE-CANCEL-8
#[test]
fn concurrent_cancels_stop_once() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "30"]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    let kids: Vec<_> = (0..2).map(|_| e.spawn(&["cancel", &r.id()])).collect();
    for k in kids {
        let out = k.wait_with_output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["status"], "cancelled");
    }
    assert_eq!(transitions_to(&e, &r.id(), "cancelled"), 1, "{}", e.aq_log());
}

// CASE-CANCEL-9
#[test]
fn interrupted_cancel_still_cancels() {
    let e = Env::new();
    let r = run_ignoring_term(&e, &[("AQ_T_KILL_GRACE_MS", "3000")]);
    let mut k = e.spawn(&["cancel", &r.id()]);
    assert!(until(5, || e.q_i64(&format!("SELECT cancel_requested FROM executions WHERE id='{}'", r.id())) == 1));
    std::process::Command::new("kill").args(["-INT", &k.id().to_string()]).status().unwrap();
    let _ = k.wait();
    assert_eq!(e.wait_status(&r.id(), 20, &["cancelled"]).status(), "cancelled");
}
