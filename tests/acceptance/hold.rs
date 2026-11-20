//! 借用、归还与自动收回。

use std::process::Command;
use std::time::Duration;

use crate::support::*;

fn created_log(e: &Env) -> String {
    std::fs::read_to_string(e.path("sim/created.log")).unwrap_or_default()
}

// CASE-HOLD-1, CASE-HOLD-8
#[test]
fn sim_created_named_and_booted() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim"]);
    assert_eq!((r.code, r.status()), (0, "held"), "{}", r.stdout);
    let id = r.id();
    let d = &r.one()["devices"][0];
    assert_eq!(d["type"], "ios-sim");
    assert_eq!(e.sims(), vec![e.dev_name(&id, 1)]);
    assert!(created_log(&e).contains("iPhone 17 Pro com.apple.CoreSimulator.SimRuntime.iOS-27-2"), "机型为配置值、运行时为最新");
    assert!(std::fs::read_to_string(e.path("sim/calls.log")).unwrap().contains("boot"));
    assert!(e.state().join(format!("holds/{id}.used")).exists(), "CASE-HOLD-8");
}

// CASE-HOLD-4
#[test]
fn devices_and_accounts() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim", "--account", "test-a", "--account", "test-b"]);
    assert_eq!(r.one()["accounts"], serde_json::json!(["test-a", "test-b"]));
    let r = e.aq(&["hold", "ios-sim", "ios-sim", "--account", "a", "--account", "b"]);
    assert_eq!(r.one()["devices"].as_array().unwrap().len(), 2);
    let r = e.aq(&["hold", "ios-sim", "ios-device", "--account", "c"]);
    let d = r.one()["devices"].as_array().unwrap();
    assert_eq!(
        (d[0]["type"].as_str(), d[1]["type"].as_str(), d[1]["id"].as_str()),
        (Some("ios-sim"), Some("ios-device"), Some("phone-1")),
        "顺序与请求一致"
    );
    assert_eq!(r.one()["accounts"], serde_json::json!(["c"]));
}

// CASE-HOLD-5, CASE-HOLD-4
#[test]
fn account_taken_waits_then_gets_after_release() {
    let e = Env::new();
    let a = e.aq(&["hold", "ios-sim", "--account", "test-a"]);
    let b = e.aq(&["hold", "ios-sim", "--account", "test-a", "--wait", "1"]);
    assert_eq!((b.code, b.status()), (2, "waiting"));
    e.aq(&["release", &a.id()]);
    let s = e.aq(&["status", &b.id(), "--wait", "10"]);
    assert_eq!((s.code, s.status()), (0, "held"));
}

// CASE-RUN-2
#[test]
fn usage_errors() {
    let e = Env::new();
    for args in [&["hold", "ios-sim", "--account", "a", "--account", "a"][..], &["hold"][..], &["hold", "pad"][..]] {
        let r = e.aq(args);
        assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("usage")), "{args:?}");
    }
    assert_eq!(e.q_i64("SELECT count(*) FROM holds"), 0);
}

// CASE-HOLD-4
#[test]
fn two_groups_same_phone_and_account_all_or_nothing() {
    let e = Env::new();
    let k1 = e.spawn(&["hold", "ios-device", "--account", "a", "--wait", "3"]);
    let k2 = e.spawn(&["hold", "ios-device", "--account", "a", "--wait", "3"]);
    let r1 = parse(k1.wait_with_output().unwrap());
    let r2 = parse(k2.wait_with_output().unwrap());
    let mut st = [r1.status().to_string(), r2.status().to_string()];
    st.sort();
    assert_eq!(st, ["held", "waiting"]);
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices"), 1, "等待的一组不占任何一项");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_accounts"), 1);
}

// CASE-HOLD-6
#[test]
fn unschedulable_and_memory_bound_sims() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("mem = \"28G\"", "mem = \"4G\""));
    let r = e.aq(&["hold", "ios-device", "ios-device"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (5, Some("unschedulable")));
    let r = e.aq(&["hold", "ios-sim", "ios-sim"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (5, Some("unschedulable")), "借的量超整机");
    let a = e.aq(&["hold", "ios-sim"]);
    assert_eq!(a.status(), "held");
    let b = e.aq(&["hold", "ios-sim", "--wait", "1"]);
    assert_eq!((b.code, b.status()), (2, "waiting"));
}

// CASE-HOLD-7
#[test]
fn create_failure_rolls_back_whole_group() {
    let e = Env::new();
    std::fs::write(e.path("sim/fail_create"), "").unwrap();
    let r = e.aq(&["hold", "ios-sim", "ios-device", "--account", "a"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (4, Some("device_error")), "{}", r.stdout);
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices") + e.q_i64("SELECT count(*) FROM hold_accounts"), 0);
    std::fs::remove_file(e.path("sim/fail_create")).unwrap();
    let again = e.aq(&["hold", "ios-device", "--account", "a"]);
    assert_eq!(again.status(), "held", "真机和账号立刻可被别人借");
    let no_xcode = Env::new();
    no_xcode.env("AQ_SIMCTL", "/usr/bin/false");
    let r = no_xcode.aq(&["hold", "ios-sim", "--account", "x"]);
    assert_eq!(r.one()["reason"], "device_error", "CASE-HOLD-7 没装 Xcode");
}

// CASE-HOLD-8
#[test]
fn wait_returns_waiting_then_held_with_memory_reserved() {
    let e = Env::new();
    e.env("FAKE_SIM_CREATE_SECS", "2");
    let r = e.aq(&["hold", "ios-sim", "--wait", "1"]);
    assert_eq!((r.code, r.status()), (2, "waiting"));
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices WHERE device IS NULL"), 1, "创建中模拟器行 device 为空，内存已预留");
    assert_eq!(e.aq(&["status"]).lines[0]["sims"], 1);
    let s = e.aq(&["status", &r.id(), "--wait", "10"]);
    assert_eq!((s.code, s.status()), (0, "held"));
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices WHERE device IS NOT NULL"), 1, "UDID 已回填");
}

// CASE-RECLAIM-1
#[test]
fn release_frees_now_deletes_sim_later() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim", "ios-device", "--account", "a"]);
    let id = r.id();
    let rel = e.aq(&["release", &id]);
    assert_eq!((rel.code, rel.status()), (0, "released"));
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_accounts"), 0, "账号立刻可借");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices WHERE type='ios-device'"), 0, "真机立刻可借");
    assert!(until(10, || e.sims().is_empty()), "模拟器随后被删");
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 0), "删完才删模拟器行");
    assert!(!e.state().join(format!("holds/{id}.used")).exists(), "CASE-RECLAIM-1");
    let again = e.aq(&["release", &id]);
    assert_eq!((again.code, again.one()), (0, rel.one()), "CASE-RECLAIM-1 输出不变");
}

// CASE-RECLAIM-2, CASE-RUN-2, CASE-CANCEL-3, CASE-STORE-1
#[test]
fn release_waiting_and_unknown() {
    let e = Env::new();
    let a = e.aq(&["hold", "ios-device"]);
    let b = e.aq(&["hold", "ios-device", "--wait", "0"]);
    assert_eq!(e.aq(&["release", &b.id()]).status(), "released");
    e.aq(&["release", &a.id()]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(e.aq(&["status", &b.id()]).status(), "released", "CASE-RECLAIM-2 之后不再分配");
    assert_eq!(e.q_i64(&format!("SELECT acquired_at IS NULL FROM holds WHERE id='{}'", b.id())), 1, "CASE-STORE-1");
    let r = e.aq(&["release", "hd_nope"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("usage")));
}

/// 冒充 Claude Code 的会话主进程：环境变量带 CLAUDECODE，进程名 fakeagent（AQ_T_AGENT_ARGV0 把已验证表里的 claude 换成它），执行 inner 后常驻。
fn fake_agent(e: &Env, inner: &str) -> std::process::Child {
    e.as_process("fakeagent", &[("CLAUDECODE", "1"), ("AQ_T_AGENT_ARGV0", "fakeagent")], &format!("{inner}; sleep 300"))
}

// CASE-RECLAIM-3
#[test]
fn owner_exit_reclaims_and_deletes_sim() {
    let e = Env::new();
    let mut agent = fake_agent(&e, &format!("{AQ} hold ios-sim --account a > {}", e.path("out").display()));
    assert!(until(10, || e.q_i64("SELECT count(*) FROM holds WHERE status='held'") == 1));
    let owner = e.q_i64("SELECT owner_pid FROM holds");
    assert_eq!(owner, agent.id() as i64, "借用者是 agent 进程");
    agent.kill().unwrap();
    let _ = agent.wait();
    e.aq(&["status"]);
    assert_eq!(e.q_str("SELECT status FROM holds").as_deref(), Some("reclaimed"));
    assert!(until(10, || e.sims().is_empty()), "模拟器被删");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_accounts"), 0);
}

// CASE-RECLAIM-3
#[test]
fn pid_reused_with_other_start_time_reclaims() {
    let e = Env::new();
    e.aq(&["status"]); // 先建好库，下面轮询时表已存在
    let mut agent = fake_agent(&e, &format!("{AQ} hold ios-device > {}", e.path("out").display()));
    assert!(until(10, || e.q_i64("SELECT count(*) FROM holds WHERE status='held'") == 1));
    let r = parse_file(&e.path("out"));
    e.db().execute("UPDATE holds SET owner_started_at = owner_started_at - 12345", []).unwrap();
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &r.id()]).status(), "reclaimed");
    let _ = agent.kill(); // agent 进程还活着：收回靠的是启动时间对不上
    let _ = agent.wait();
}

// CASE-RECLAIM-4
#[test]
fn idle_reclaim() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "2");
    let idle = e.aq(&["hold", "ios-device"]);
    let busy = e.aq(&["hold", "ios-sim"]);
    let udid = busy.one()["devices"][0]["id"].as_str().unwrap().to_string();
    let used = e.state().join(format!("holds/{}.used", busy.id()));
    let born = modified(&used);
    let mut user = e.using_device(&udid); // aq 之外的进程参数里带着 UDID：一直在用
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(800));
        e.aq(&["status"]);
    }
    assert_eq!(e.aq(&["status", &idle.id()]).status(), "reclaimed", "CASE-RECLAIM-4 闲置超限收回");
    assert_eq!(e.aq(&["status", &busy.id()]).status(), "held", "CASE-RECLAIM-4 一直在用不收回");
    assert!(modified(&used) > born, "CASE-RECLAIM-4 探测到在用就刷新 .used");
    let _ = user.kill();
    let _ = user.wait();
    std::thread::sleep(Duration::from_millis(3000));
    assert_eq!(e.aq(&["status", &busy.id()]).status(), "reclaimed", "CASE-RECLAIM-4 没人用了按闲置收回");
    assert!(e.aq_log().lines().any(|l| l.contains(&busy.id()) && l.contains("idle past the limit")), "收回原因为闲置");
    assert!(!used.exists(), "CASE-RECLAIM-4 收回后 .used 不存在");
    let quiet = Env::new();
    quiet.env("AQ_T_IDLE_RECLAIM_S", "100");
    let h = quiet.aq(&["hold", "ios-device"]);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(quiet.aq(&["status", &h.id()]).status(), "held", "闲置不足上限不收回");
    assert!(!e.state().join(format!("holds/{}.used", idle.id())).exists(), "CASE-RECLAIM-4 收回后 .used 不存在");
}

fn modified(p: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

// CASE-FAULT-5
#[test]
fn kill_hold_runner_mid_creation_leaves_no_residue() {
    let e = Env::new();
    e.env("FAKE_SIM_CREATE_SECS", "2");
    let r = e.aq(&["hold", "ios-sim", "--wait", "0"]);
    let id = r.id();
    assert!(until(5, || e.q_i64("SELECT count(*) FROM hold_devices") == 1));
    let kill = Command::new("pkill").args(["-9", "-f", &format!("__runner {id}")]).status().unwrap();
    assert!(kill.success());
    std::thread::sleep(Duration::from_millis(300));
    let s = e.aq(&["status", &id, "--wait", "20"]);
    assert_eq!(s.status(), "held", "{}", s.stdout);
    assert!(
        until(15, || {
            e.aq(&["status"]);
            e.sims().len() == 1
        }),
        "半成品被删，只剩一台：{:?}",
        e.sims()
    );
}

// CASE-RECLAIM-6
#[test]
fn sim_log_dir_removed_with_sim() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim"]);
    let udid = r.one()["devices"][0]["id"].as_str().unwrap().to_string();
    let logs = e.dir.path().join("Library/Logs/CoreSimulator");
    std::fs::create_dir_all(logs.join(&udid)).unwrap();
    std::fs::write(logs.join(&udid).join("system.log"), "x").unwrap();
    std::fs::create_dir_all(logs.join("OTHER-UDID")).unwrap();
    e.aq(&["release", &r.id()]);
    assert!(until(10, || !logs.join(&udid).exists()), "随模拟器一起删掉日志目录");
    assert!(logs.join("OTHER-UDID").exists(), "别的设备的日志不动");
}

/// 借用者未识别：经一个随即退出的 shell 调 aq hold。
fn hold_via_shell(e: &Env, envs: &[(&str, &str)], agent_name: &str) -> String {
    e.aq(&["status"]); // 先建好库，下面轮询时表已存在
    let out = e.path("out-shell");
    let inner = format!("bash -c '{AQ} hold ios-sim > {}'; sleep 300", out.display());
    let mut p = e.as_process(agent_name, envs, &inner);
    assert!(until(10, || e.q_i64("SELECT count(*) FROM holds WHERE status='held'") == 1));
    let id = parse_file(&out).id();
    let _ = p.kill();
    let _ = p.wait();
    id
}

// CASE-RECLAIM-5
#[test]
fn unidentified_owner_not_reclaimed_on_caller_exit_but_idle() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "3");
    let id = hold_via_shell(&e, &[], "someshell");
    assert_eq!(e.q_i64("SELECT owner_pid FROM holds"), 0, "借用者未识别记为 0");
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &id]).status(), "held", "调用方退出不收回");
    std::thread::sleep(Duration::from_millis(3500));
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &id]).status(), "reclaimed", "闲置超限收回");
}

// CASE-RECLAIM-5
#[test]
fn claude_env_without_claude_ancestor_is_unidentified() {
    let e = Env::new();
    // 测试本身跑在真 Claude Code 里，链上有真 claude；把要找的主进程名换成链上不存在的 fakeagent
    let id = hold_via_shell(&e, &[("CLAUDECODE", "1"), ("AQ_T_AGENT_ARGV0", "fakeagent")], "notclaude");
    assert_eq!(e.q_i64("SELECT owner_pid FROM holds"), 0, "环境变量说是 Claude Code，但链上没有 claude");
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &id]).status(), "held");
}
