//! 浏览器与桌面：browser（借时开一个 agent-browser 会话、还时关掉，按内存放行）与 desktop（只发 agent-desktop 会话名）。
//! 两者都可以多个 agent 同时借。

use std::time::Duration;

use crate::support::*;

fn browser_calls(e: &Env) -> String {
    std::fs::read_to_string(e.path("browser/calls.log")).unwrap_or_default()
}

// CASE-HOLD-3
#[test]
fn browser_opens_named_session() {
    let e = Env::new();
    let r = e.aq(&["hold", "browser"]);
    assert_eq!((r.code, r.status()), (0, "held"), "{}", r.stdout);
    let name = e.dev_name(&r.id(), 1);
    assert_eq!(r.one()["devices"][0], serde_json::json!({"type": "browser", "id": name}));
    assert_eq!(e.browsers(), std::slice::from_ref(&name));
    assert!(browser_calls(&e).contains(&format!("--session {name} open about:blank")), "{}", browser_calls(&e));
}

// CASE-HOLD-3
#[test]
fn browsers_shared_by_many_agents() {
    let e = Env::new();
    let a = e.aq(&["hold", "browser"]);
    let b = e.aq(&["hold", "browser", "browser"]);
    assert_eq!((a.status(), b.status()), ("held", "held"), "{} {}", a.stdout, b.stdout);
    assert_eq!(e.browsers().len(), 3, "各自一个会话：{:?}", e.browsers());
}

// CASE-HOLD-3
#[test]
fn browsers_admitted_by_memory() {
    let e = Env::with_config("mem = \"2G\"\n[browser]\nmem = \"1G\"\n");
    let a = e.aq(&["hold", "browser", "browser"]);
    assert_eq!(a.status(), "held", "{}", a.stdout);
    let b = e.aq(&["hold", "browser", "--wait", "1"]);
    assert_eq!(b.status(), "waiting", "内存记满后排队：{}", b.stdout);
    e.aq(&["release", &a.id()]);
    assert_eq!(e.aq(&["status", &b.id(), "--wait", "10"]).status(), "held");
}

// CASE-RECLAIM-1
#[test]
fn release_closes_session_and_frees_memory() {
    let e = Env::new();
    let r = e.aq(&["hold", "browser"]);
    assert_eq!(e.aq(&["release", &r.id()]).status(), "released");
    assert!(until(10, || e.browsers().is_empty()), "会话被关掉");
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 0), "关完才释放内存");
}

// CASE-HOLD-7
#[test]
fn open_failure_rolls_back_group() {
    let e = Env::new();
    std::fs::write(e.path("browser/fail_open"), "").unwrap();
    let r = e.aq(&["hold", "browser", "ios-device", "--account", "a"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("device_error")), "{}", r.stdout);
    assert!(e.browsers().is_empty(), "起了一半的会话被关掉：{:?}", e.browsers());
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices") + e.q_i64("SELECT count(*) FROM hold_accounts"), 0);
}

// CASE-RECLAIM-4
#[test]
fn idle_reclaim_closes_session() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "1");
    let r = e.aq(&["hold", "browser"]);
    std::thread::sleep(Duration::from_millis(1200));
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &r.id()]).status(), "reclaimed");
    assert!(until(10, || e.browsers().is_empty()));
}

// CASE-FAULT-2
#[test]
fn reconcile_closes_only_aq_orphans_never_user_session() {
    let e = Env::new();
    std::fs::write(e.path("browser/sessions").join(e.dev_name("hd_gone", 1)), "").unwrap();
    std::fs::write(e.path("browser/sessions/mywork"), "").unwrap();
    e.aq(&["status"]);
    assert!(until(10, || e.browsers() == ["mywork"]), "残留被对账关掉，用户自己的会话不动：{:?}", e.browsers());
}

// CASE-HOLD-3
#[test]
fn desktop_gives_session_name_to_many_agents() {
    let e = Env::new();
    let a = e.aq(&["hold", "desktop"]);
    let b = e.aq(&["hold", "desktop"]);
    assert_eq!((a.status(), b.status()), ("held", "held"), "{} {}", a.stdout, b.stdout);
    assert_eq!(a.one()["devices"][0], serde_json::json!({"type": "desktop", "id": e.dev_name(&a.id(), 1)}));
    e.aq(&["release", &a.id()]);
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 1), "还了就删行，只剩 b 的");
    assert!(!browser_calls(&e).contains(" open "), "desktop 不开浏览器会话：{}", browser_calls(&e));
}

// CASE-HOLD-4
#[test]
fn mixed_group_with_sim_browser_desktop() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim", "browser", "desktop", "--account", "a"]);
    let types: Vec<&str> = r.one()["devices"].as_array().unwrap().iter().filter_map(|d| d["type"].as_str()).collect();
    assert_eq!(types, ["ios-sim", "browser", "desktop"], "{}", r.stdout);
    e.aq(&["release", &r.id()]);
    assert!(until(10, || e.sims().is_empty() && e.browsers().is_empty() && e.q_i64("SELECT count(*) FROM hold_devices") == 0));
}
