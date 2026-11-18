//! 同一台机器上的多个账本（不同 XDG_STATE_HOME）共用模拟器、浏览器会话：设备名带账本标识，各自只清理自己的。
//! 两个 Env 共用一个假 simctl 目录，模拟同一台机器的 CoreSimulator。

use crate::support::*;

/// 和 a 共用假 simctl、假 agent-browser 目录的第二个账本。
fn second_ledger(a: &Env) -> Env {
    let b = Env::new();
    b.env("FAKE_SIM_DIR", &a.path("sim").to_string_lossy());
    b.env("FAKE_BROWSER_DIR", &a.path("browser").to_string_lossy());
    b
}

/// 触发一次对账（AQ_T_SIMGC_S=1：距上次超过 1 秒才会再跑）。
fn reconcile(e: &Env) {
    std::thread::sleep(std::time::Duration::from_millis(1100));
    e.aq(&["status"]);
}

// CASE-HOLD-9
#[test]
fn two_ledgers_never_delete_each_others_devices() {
    let a = Env::new();
    let b = second_ledger(&a);
    assert_ne!(a.ledger(), b.ledger());
    let ha = a.aq(&["hold", "ios-sim", "browser"]);
    let hb = b.aq(&["hold", "ios-sim", "browser"]);
    assert_eq!((ha.status(), hb.status()), ("held", "held"), "{} {}", ha.stdout, hb.stdout);
    // A 的半成品：建好了但没记进账本（创建后崩溃）
    std::fs::write(a.path("sim/devices/HALF-MADE-A"), a.dev_name("hd_crashed", 1)).unwrap();
    for _ in 0..2 {
        reconcile(&a);
        reconcile(&b);
    }
    let sims = a.sims();
    for n in [a.dev_name(&ha.id(), 1), b.dev_name(&hb.id(), 1)] {
        assert!(sims.contains(&n), "两边借出的模拟器都还在：{n} 不在 {sims:?}");
    }
    assert!(!sims.contains(&a.dev_name("hd_crashed", 1)), "A 自己的半成品被 A 删掉：{sims:?}");
    assert_eq!(a.browsers().len(), 2, "两边借出的浏览器会话都还在：{:?}", a.browsers());
    a.aq(&["release", &ha.id()]);
    assert!(until(10, || {
        a.aq(&["status"]);
        a.sims() == vec![b.dev_name(&hb.id(), 1)]
    }));
    b.aq(&["release", &hb.id()]);
    assert!(until(10, || {
        b.aq(&["status"]);
        a.sims().is_empty() && a.browsers().is_empty()
    }));
}

// CASE-HOLD-10
#[test]
fn legacy_names_reclaimed_only_by_default_ledger() {
    let other = Env::new();
    let default = second_ledger(&other);
    // 默认账本：状态目录在 $HOME/.local/state/aq
    default.env("XDG_STATE_HOME", &default.path(".local/state").to_string_lossy());
    std::fs::create_dir_all(other.path("sim/devices")).unwrap();
    std::fs::write(other.path("sim/devices/LEGACY-1"), "aq-hd_old-1").unwrap();
    reconcile(&other);
    reconcile(&other);
    assert_eq!(other.sims(), vec!["aq-hd_old-1".to_string()], "非默认账本不动旧格式名字");
    reconcile(&default);
    assert!(until(10, || other.sims().is_empty()), "默认账本删掉旧格式的残留：{:?}", other.sims());
}

// CASE-HOLD-11
#[test]
fn held_device_deleted_outside_aq_is_reported_missing() {
    let e = Env::new();
    let h = e.aq(&["hold", "ios-sim"]);
    assert_eq!(h.status(), "held", "{}", h.stdout);
    let udid = h.one()["devices"][0]["id"].as_str().unwrap().to_string();
    assert!(e.aq(&["status", &h.id()]).one().get("missing").is_none(), "设备在时没有 missing");
    std::fs::remove_file(e.path("sim/devices").join(&udid)).unwrap();
    let s = e.aq(&["status", &h.id()]);
    assert_eq!(s.status(), "held", "状态不变，由调用方决定归还重借");
    assert_eq!(s.one()["missing"], serde_json::json!([udid]), "{}", s.stdout);
    assert!(e.aq_log().contains("借出的设备已不在"));
}
