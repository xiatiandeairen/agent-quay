//! Android：android-emu（借时新起实例、还时 kill）与 android-device（固定清单、独占）。

use std::process::Command;
use std::time::Duration;

use crate::support::*;

fn emu_calls(e: &Env) -> String {
    std::fs::read_to_string(e.path("emu/calls.log")).unwrap_or_default()
}

// CASE-HOLD-2
#[test]
fn android_emu_new_readonly_instance_on_aq_port() {
    let e = Env::new();
    let r = e.aq(&["hold", "android-emu"]);
    assert_eq!((r.code, r.status()), (0, "held"), "{}", r.stdout);
    assert_eq!(r.one()["devices"][0], serde_json::json!({"type": "android-emu", "id": "emulator-5600"}));
    let calls = emu_calls(&e);
    assert!(
        calls.contains("-avd test_avd") && calls.contains("-read-only") && calls.contains("-no-window") && calls.contains("-port 5600"),
        "{calls}"
    );
    assert_eq!(std::fs::read_to_string(e.state().join("emu/5600.name")).unwrap(), format!("aq-{}-1", r.id()));
    assert!(e.state().join(format!("holds/{}.used", r.id())).exists());
}

// CASE-HOLD-2
#[test]
fn two_groups_get_distinct_ports() {
    let e = Env::new();
    e.env("FAKE_EMU_BOOT_SECS", "1");
    let k1 = e.spawn(&["hold", "android-emu", "--wait", "20"]);
    let k2 = e.spawn(&["hold", "android-emu", "--wait", "20"]);
    let a = parse(k1.wait_with_output().unwrap());
    let b = parse(k2.wait_with_output().unwrap());
    assert_eq!((a.status(), b.status()), ("held", "held"), "{} {}", a.stdout, b.stdout);
    assert_ne!(a.one()["devices"][0]["id"], b.one()["devices"][0]["id"]);
    assert_eq!(e.emus(), ["5600", "5602"]);
}

// CASE-RECLAIM-1
#[test]
fn release_kills_instance_and_frees_memory() {
    let e = Env::new();
    let r = e.aq(&["hold", "android-emu", "--account", "a"]);
    assert_eq!(e.aq(&["status"]).lines[0]["sims"], 1);
    assert_eq!(e.aq(&["release", &r.id()]).status(), "released");
    assert!(until(10, || e.emus().is_empty()), "实例被 kill");
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 0), "删完才释放内存");
    assert!(!e.state().join("emu/5600.name").exists());
}

// CASE-HOLD-7
#[test]
fn boot_failure_rolls_back_group() {
    let e = Env::new();
    std::fs::write(e.path("emu/fail_boot"), "").unwrap();
    let r = e.aq(&["hold", "android-emu", "android-device", "--account", "a"]);
    assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("device_error")), "{}", r.stdout);
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices") + e.q_i64("SELECT count(*) FROM hold_accounts"), 0);
    std::fs::remove_file(e.path("emu/fail_boot")).unwrap();
    assert_eq!(e.aq(&["hold", "android-device", "--account", "a"]).status(), "held", "真机和账号立刻可借");
}

// CASE-RECLAIM-4
#[test]
fn idle_reclaim_kills_instance() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "1");
    let r = e.aq(&["hold", "android-emu"]);
    std::thread::sleep(Duration::from_millis(1200));
    e.aq(&["status"]);
    assert_eq!(e.aq(&["status", &r.id()]).status(), "reclaimed");
    assert!(until(10, || e.emus().is_empty()));
}

// CASE-FAULT-5
#[test]
fn kill_runner_mid_boot_no_residue() {
    let e = Env::new();
    e.env("FAKE_EMU_BOOT_SECS", "2");
    let r = e.aq(&["hold", "android-emu", "--wait", "0"]);
    assert!(until(5, || !e.emus().is_empty()), "实例已在启动");
    Command::new("pkill").args(["-9", "-f", &format!("__runner {}", r.id())]).status().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let s = e.aq(&["status", &r.id(), "--wait", "20"]);
    assert_eq!(s.status(), "held", "{}", s.stdout);
    assert!(
        until(10, || {
            e.aq(&["status"]);
            e.emus().len() == 1
        }),
        "半成品被删，只剩一台：{:?}",
        e.emus()
    );
}

// CASE-FAULT-2
#[test]
fn reconcile_kills_only_aq_orphans_never_user_emulator() {
    let e = Env::new();
    // 一个 aq 端口上的残留实例（没有对应的活动借用）
    std::fs::write(e.path("emu/running/5610"), "999999").unwrap();
    e.aq(&["status"]);
    assert!(until(10, || e.emus().is_empty()), "残留被对账删掉");
    let adb = std::fs::read_to_string(e.path("emu/adb.log")).unwrap_or_default();
    assert!(!adb.contains("emulator-5554 emu kill"), "用户自己的模拟器不受影响");
}

// CASE-HOLD-5
#[test]
fn android_device_exclusive() {
    let e = Env::new();
    let a = e.aq(&["hold", "android-device"]);
    assert_eq!(a.one()["devices"][0], serde_json::json!({"type": "android-device", "id": "android-1"}));
    let b = e.aq(&["hold", "android-device", "--wait", "1"]);
    assert_eq!(b.status(), "waiting");
    let r = e.aq(&["hold", "android-device", "android-device"]);
    assert_eq!(r.one()["reason"], "unschedulable");
    e.aq(&["release", &a.id()]);
    assert_eq!(e.aq(&["status", &b.id(), "--wait", "10"]).status(), "held");
}

// CASE-HOLD-4
#[test]
fn mixed_ios_and_android_group() {
    let e = Env::new();
    let r = e.aq(&["hold", "ios-sim", "android-emu", "--account", "a"]);
    let d = r.one()["devices"].as_array().unwrap();
    assert_eq!((d[0]["type"].as_str(), d[1]["type"].as_str()), (Some("ios-sim"), Some("android-emu")));
    e.aq(&["release", &r.id()]);
    assert!(until(10, || e.emus().is_empty() && e.sims().is_empty()));
}
