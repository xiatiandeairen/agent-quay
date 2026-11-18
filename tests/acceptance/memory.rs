//! 内存放行：系统压力闸门、模拟器冷启动串行、设备按实测记账、容量默认值。系统读数用 AQ_SYS_OVERRIDE 造。

use serde_json::json;

use crate::support::*;

fn waiting_for(e: &Env, id: &str) -> Vec<String> {
    let s = e.aq(&["status", id]);
    s.one()["waiting_for"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

// CASE-QUEUE-5
#[test]
fn pressure_normal_and_room_admits() {
    let e = Env::new();
    e.set_sys_with(json!({"pressure": "normal", "swapping": false}));
    let r = e.aq(&["run", "--wait", "10", "--", "fake-task", "--secs", "0"]);
    assert_eq!(r.status(), "ok", "{}", r.stdout);
    let h = e.aq(&["hold", "ios-sim"]);
    assert_eq!(h.status(), "held", "{}", h.stdout);
}

// CASE-QUEUE-5
#[test]
fn pressure_or_swapping_blocks_with_reason_until_it_clears() {
    for extra in [json!({"pressure": "warn"}), json!({"pressure": "critical"}), json!({"swapping": true})] {
        let e = Env::new();
        e.set_sys_with(extra.clone());
        let r = e.aq(&["run", "--wait", "1", "--", "fake-task", "--secs", "0"]);
        assert_eq!(r.status(), "queued", "{extra}: 账面够也不放行：{}", r.stdout);
        let h = e.aq(&["hold", "ios-sim", "--wait", "1"]);
        assert_eq!(h.status(), "waiting", "{extra}: {}", h.stdout);
        assert_eq!(waiting_for(&e, &r.id()), ["mem_pressure"], "{extra}");
        assert_eq!(waiting_for(&e, &h.id()), ["mem_pressure"], "{extra}");
        assert!(e.aq(&["status", &h.id()]).one()["retry_after_s"].is_number());
        let d = e.aq(&["hold", "ios-device", "--wait", "1"]);
        assert_eq!(d.status(), "held", "{extra}: 不要内存的（真机）不受影响：{}", d.stdout);
        e.set_sys_with(json!({"pressure": "normal", "swapping": false}));
        assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok", "{extra}: 压力回落后放行");
        assert_eq!(e.aq(&["status", &h.id(), "--wait", "20"]).status(), "held");
    }
}

// CASE-QUEUE-6
#[test]
fn three_simulators_cold_start_one_at_a_time() {
    let e = Env::new();
    e.env("FAKE_SIM_CREATE_SECS", "2");
    let hs: Vec<Res> = (0..3).map(|_| e.aq(&["hold", "ios-sim", "--wait", "0"])).collect();
    let ids: Vec<String> = hs.iter().map(Res::id).collect();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let waiting: Vec<&String> = ids.iter().filter(|id| e.aq(&["status", id]).status() == "waiting").collect();
    assert_eq!(waiting.len(), 3, "第一台在建（waiting），另两台排队");
    assert_eq!(waiting_for(&e, &ids[1]), ["cold_start"]);
    assert!(e.aq(&["status", &ids[2]]).one()["retry_after_s"].is_number());
    let mut most = 0;
    assert!(
        until(30, || {
            e.aq(&["status"]);
            most = most.max(e.q_i64("SELECT count(*) FROM hold_devices WHERE type = 'ios-sim' AND device IS NULL"));
            ids.iter().all(|id| e.aq(&["status", id]).status() == "held")
        }),
        "三台最终都借到"
    );
    assert_eq!(most, 1, "任何时刻最多一台在冷启动");
}

// CASE-HOLD-12
#[test]
fn simulator_count_limit_queues_then_admits_after_release() {
    let e = Env::with_config(&format!("sim_slots = 2\n{DEFAULT_CONFIG}"));
    let a = e.aq(&["hold", "ios-sim"]);
    let b = e.aq(&["hold", "android-emu"]);
    assert_eq!((a.status(), b.status()), ("held", "held"), "{} {}", a.stdout, b.stdout);
    let w = e.aq(&["hold", "browser"]);
    assert_eq!(w.status(), "held", "浏览器会话不算台数：{}", w.stdout);
    let c = e.aq(&["hold", "ios-sim", "--wait", "2"]);
    assert_eq!(c.status(), "waiting", "{}", c.stdout);
    assert_eq!(waiting_for(&e, &c.id()), ["sim_slots"]);
    assert!(e.aq(&["status", &c.id()]).one()["retry_after_s"].is_number());
    e.aq(&["release", &a.id()]);
    assert!(until(20, || e.aq(&["status", &c.id()]).status() == "held"), "归还一台、删完后放行");
}

// CASE-QUEUE-7
#[test]
fn default_simulator_limit_from_physical_memory() {
    // 不配置 sim_slots：物理内存 16G（覆盖值）→ 2 台
    let e = Env::new();
    e.set_sys_with(json!({"mem_total_mb": 16_384}));
    let a = e.aq(&["hold", "ios-sim"]);
    let b = e.aq(&["hold", "ios-sim"]);
    let c = e.aq(&["hold", "ios-sim", "--wait", "1"]);
    assert_eq!((a.status(), b.status(), c.status()), ("held", "held", "waiting"), "{}", c.stdout);
    assert_eq!(waiting_for(&e, &c.id()), ["sim_slots"]);
}

// CASE-QUEUE-5
#[test]
fn long_pressure_keeps_queueing_and_explains() {
    let e = Env::new();
    e.env("AQ_T_PRESSURE_WARN_S", "2");
    e.set_sys_with(json!({"pressure": "warn"}));
    let r = e.aq(&["run", "--wait", "1", "--", "fake-task", "--secs", "0"]);
    let early = e.aq(&["status", &r.id()]);
    assert_eq!(early.status(), "queued");
    assert!(early.one()["mem_pressure_s"].is_number(), "报出已持续多久：{}", early.stdout);
    assert!(early.one().get("message").is_none(), "没到上限不加说明");
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let late = e.aq(&["status", &r.id()]);
    assert_eq!((late.code, late.status()), (2, "queued"), "超过上限仍排队，不失败");
    assert!(late.one()["mem_pressure_s"].as_i64().unwrap() >= 2);
    assert!(late.one()["message"].as_str().unwrap().contains("系统内存压力"), "{}", late.stdout);
    e.set_sys_with(json!({"pressure": "normal"}));
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "ok");
    assert!(!e.state().join("mem-pressure-since").exists(), "压力回落后清掉开始时刻");
}

// CASE-QUEUE-7
#[test]
fn default_capacity_from_physical_memory() {
    // 不配置 mem：物理内存 36G（覆盖值）→ 容量 27G；要 30G 的模拟器永远放不下
    let cfg = DEFAULT_CONFIG.replace("mem = \"28G\"\n", "").replace("[ios-sim]\nmem = \"3G\"", "[ios-sim]\nmem = \"30G\"");
    let e = Env::with_config(&cfg);
    let r = e.aq(&["hold", "ios-sim"]);
    assert_eq!((r.code, r.status()), (3, "rejected"), "{}", r.stdout);
    assert!(r.one()["message"].as_str().unwrap().contains("27648MB"), "{}", r.stdout);
}
