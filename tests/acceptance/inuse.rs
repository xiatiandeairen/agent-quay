//! 借出的设备是否在用（design.md 3.3）：aq 之外的进程、aq run 的参数；借出上限。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::support::*;

/// 每 0.7 秒做一次检查，持续 `secs` 秒，每次都要求借用仍为 held。
fn stays_held(e: &Env, id: &str, secs: f64, why: &str) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs_f64(secs) {
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(e.aq(&["status", id]).status(), "held", "{why}（第 {:.1} 秒）", t0.elapsed().as_secs_f64());
    }
}

fn reason_logged(e: &Env, id: &str, reason: &str) -> bool {
    e.aq_log().lines().any(|l| l.contains(id) && l.contains("\"to\":\"reclaimed\"") && l.contains(reason))
}

// CASE-RECLAIM-9
#[test]
fn max_hold_reclaims_device_in_use() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "2").env("AQ_T_MAX_HOLD_S", "6");
    let h = e.aq(&["hold", "ios-sim"]);
    let udid = h.one()["devices"][0]["id"].as_str().unwrap().to_string();
    let mut user = e.using_device(&udid);
    stays_held(&e, &h.id(), 4.5, "在用、没到上限");
    assert!(until(5, || e.aq(&["status", &h.id()]).status() == "reclaimed"), "过了 6 秒不管是否在用都收回");
    assert!(reason_logged(&e, &h.id(), "held past the max hold time"), "{}", e.aq_log());
    let _ = user.kill();
    let _ = user.wait();
    assert!(until(10, || e.sims().is_empty()));
}

// CASE-RECLAIM-10
#[test]
fn run_naming_device_keeps_hold() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "2");
    let h = e.aq(&["hold", "ios-sim"]);
    let udid = h.one()["devices"][0]["id"].as_str().unwrap().to_string();
    let dest = format!("platform=iOS Simulator,id={udid}");
    let run = e.spawn(&["run", "--wait", "30", "--", "sh", "-c", "sleep 4", &dest]);
    stays_held(&e, &h.id(), 4.2, "aq run 的参数带着 UDID，提交和运行期间都算在用");
    let r = parse(run.wait_with_output().unwrap());
    assert_eq!(r.status(), "ok", "{}", r.stdout);
    assert!(until(6, || e.aq(&["status", &h.id()]).status() == "reclaimed"), "命令结束后没人用，按闲置收回");
    assert!(reason_logged(&e, &h.id(), "idle past the limit"));
}

// CASE-RECLAIM-12
#[test]
fn android_serial_in_process_args_counts_as_use() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "2");
    let h = e.aq(&["hold", "android-emu"]);
    assert_eq!(h.status(), "held", "{}", h.stdout);
    let serial = h.one()["devices"][0]["id"].as_str().unwrap().to_string();
    let mut user = e.using_device(&serial);
    stays_held(&e, &h.id(), 4.5, "进程参数整段带着 serial");
    let _ = user.kill();
    let _ = user.wait();
    let t0 = Instant::now();
    assert!(until(8, || e.aq(&["status", &h.id()]).status() == "reclaimed"), "没人用了按闲置收回");
    assert!(t0.elapsed() >= Duration::from_secs(1), "停用之后才开始计闲置");
    assert!(reason_logged(&e, &h.id(), "idle past the limit"));
    assert!(until(10, || e.emus().is_empty()));
}

// 表外：P1 一次进程探测（列全部进程、标 aq 后代、判定一台设备）的耗时，只记录不断言；`-- --nocapture` 看输出
#[test]
fn probe_cost_is_recorded() {
    let mut best = Duration::MAX;
    let mut n = 0;
    for _ in 0..5 {
        let t = Instant::now();
        let mut procs = aq_sys::procs::list_all();
        aq_core::usage::mark_aq_descendants(&mut procs, &HashSet::new());
        let _ = aq_core::usage::devices_in_use(&procs, ["00000000-0000-0000-0000-000000000000"]);
        best = best.min(t.elapsed());
        n = procs.len();
    }
    eprintln!("P1: {n} processes, best of 5 = {} ms", best.as_millis());
    assert!(n > 10, "列得到本机进程");
}
