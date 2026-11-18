//! 真机可用性。只借「已声明 ∩ 此刻可用」的真机；不可用立即拒绝并说明原因。
//! 拒绝时列出已连接但未声明的设备和可粘贴的配置片段。真机由假 devicectl / 假 adb 模拟。

use serde_json::{json, Value};

use crate::support::*;

/// 写假 devicectl 的输出：每项 (udid, 名字, transportType)；transportType 为空表示不带这个字段。
fn ios(e: &Env, devs: &[(&str, &str, &str)]) {
    let list: Vec<Value> = devs
        .iter()
        .map(|(id, name, tr)| {
            let mut conn = json!({"pairingState": "paired", "tunnelState": "disconnected"});
            if !tr.is_empty() {
                conn["transportType"] = json!(tr);
            }
            json!({"identifier": format!("ID-{id}"), "connectionProperties": conn,
                   "deviceProperties": {"name": name, "developerModeStatus": "enabled"},
                   "hardwareProperties": {"reality": "physical", "udid": id}})
        })
        .collect();
    std::fs::write(e.path("dev/ios.json"), json!({"result": {"devices": list}}).to_string()).unwrap();
}

fn two_phones() -> Env {
    Env::with_config(&DEFAULT_CONFIG.replace("ids = [\"phone-1\"]", "ids = [\"phone-1\", \"phone-2\"]"))
}

// CASE-DEV-1
#[test]
fn wired_and_wireless_both_held() {
    let e = two_phones();
    ios(&e, &[("phone-1", "Phone 1", "wired"), ("phone-2", "Phone 2", "localNetwork")]);
    let r = e.aq(&["hold", "ios-device", "ios-device"]);
    assert_eq!((r.code, r.status()), (0, "held"), "{}", r.stdout);
    let ids: Vec<&str> = r.one()["devices"].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["phone-1", "phone-2"], "无线、调试通道没开也算可用");
}

// CASE-DEV-2
#[test]
fn only_declared_unavailable_rejected_without_queueing() {
    let e = Env::new();
    ios(&e, &[]);
    let r = e.aq(&["hold", "ios-device"]);
    let o = r.one();
    assert_eq!((r.code, o["status"].as_str(), o["reason"].as_str()), (3, Some("rejected"), Some("device_unavailable")), "{}", r.stdout);
    assert_eq!(o["unavailable"][0]["id"], "phone-1");
    assert!(o["unavailable"][0]["why"].as_str().unwrap().contains("cannot find"), "{o}");
    assert!(o.get("undeclared").is_none() && o.get("config_snippet").is_none(), "没有未声明的设备就不带");
    assert_eq!(e.q_i64("SELECT count(*) FROM holds"), 0, "不排队、不留借用");
}

// CASE-DEV-3
#[test]
fn undeclared_connected_listed_never_lent() {
    let e = Env::new();
    ios(&e, &[("own-phone", "Own Phone", "wired")]);
    let r = e.aq(&["hold", "ios-device"]);
    let o = r.one();
    assert_eq!((r.code, o["reason"].as_str()), (3, Some("device_unavailable")), "{}", r.stdout);
    assert_eq!(o["undeclared"], json!([{"type": "ios-device", "id": "own-phone", "name": "Own Phone"}]));
    assert_eq!(o["config_snippet"], "[ios-device]\nids = [\"phone-1\", \"own-phone\"]");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices"), 0, "未声明的绝不借出");
}

// CASE-DEV-3
#[test]
fn empty_list_unschedulable_with_snippet() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("[ios-device]\nids = [\"phone-1\"]\n", ""));
    ios(&e, &[("new-phone", "New Phone", "localNetwork")]);
    let r = e.aq(&["hold", "ios-device"]);
    let o = r.one();
    assert_eq!((r.code, o["reason"].as_str()), (3, Some("unschedulable")), "{}", r.stdout);
    assert_eq!(o["undeclared"][0]["id"], "new-phone");
    assert_eq!(o["config_snippet"], "[ios-device]\nids = [\"new-phone\"]");
    ios(&e, &[]);
    let r = e.aq(&["hold", "ios-device"]);
    let o = r.one();
    assert_eq!(o["reason"], "unschedulable");
    assert!(o.get("undeclared").is_none() && o.get("config_snippet").is_none(), "什么都没连就不带");
}

// CASE-DEV-1
#[test]
fn skips_unavailable_takes_available() {
    let e = two_phones();
    ios(&e, &[("phone-2", "Phone 2", "wired")]);
    let r = e.aq(&["hold", "ios-device"]);
    assert_eq!((r.status(), r.one()["devices"][0]["id"].as_str()), ("held", Some("phone-2")), "{}", r.stdout);
}

// CASE-DEV-4
#[test]
fn busy_phone_waits_unavailable_never_given() {
    let e = two_phones();
    ios(&e, &[("phone-1", "Phone 1", "wired")]);
    let h0 = e.aq(&["hold", "ios-device"]);
    assert_eq!(h0.one()["devices"][0]["id"], "phone-1");
    let h1 = e.aq(&["hold", "ios-device", "--wait", "0"]);
    assert_eq!((h1.code, h1.status()), (2, "waiting"), "phone-1 只是忙，排队而不是拒绝：{}", h1.stdout);
    ios(&e, &[("phone-1", "Phone 1", "wired"), ("phone-2", "Phone 2", "wired")]);
    let w = e.aq(&["status", &h1.id(), "--wait", "1"]);
    assert_eq!(w.status(), "waiting", "提交时不可用的 phone-2 不给这次借用：{}", w.stdout);
    e.aq(&["release", &h0.id()]);
    let w = e.aq(&["status", &h1.id(), "--wait", "10"]);
    assert_eq!((w.status(), w.one()["devices"][0]["id"].as_str()), ("held", Some("phone-1")), "{}", w.stdout);
}

// CASE-DEV-2
#[test]
fn android_unauthorized_rejected_emulator_not_listed() {
    let e = Env::new();
    std::fs::write(e.path("emu/real-devices"), "android-1\tunauthorized usb:1-1 model:Pixel_9\n").unwrap();
    let r = e.aq(&["hold", "android-device"]);
    let o = r.one();
    assert_eq!((r.code, o["reason"].as_str()), (3, Some("device_unavailable")), "{}", r.stdout);
    assert!(o["unavailable"][0]["why"].as_str().unwrap().contains("not authorized"), "{o}");
    assert!(o.get("undeclared").is_none(), "emulator-5554 是模拟器，不算未声明的真机：{o}");
}

// CASE-DEV-2
#[test]
fn missing_devicectl_rejected_and_sims_never_probe() {
    let e = Env::new();
    e.env("AQ_DEVICECTL", "/nonexistent/devicectl");
    let r = e.aq(&["hold", "ios-device"]);
    let o = r.one();
    assert_eq!((r.code, o["reason"].as_str()), (3, Some("device_unavailable")), "{}", r.stdout);
    assert!(o["unavailable"][0]["why"].as_str().unwrap().contains("devicectl"), "{o}");
    let e = Env::new();
    assert_eq!(e.aq(&["hold", "ios-sim"]).status(), "held");
    assert!(!e.path("dev/devicectl.log").exists(), "不借真机就不探测");
}
