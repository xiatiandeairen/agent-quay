//! 借模拟器时指定版本和机型：`<类型>[@<版本>][:<机型>]`，组合不了就在提交时拒绝；Android 每次新建自己的 AVD。
//! 假 simctl：26.2 只支持 iPhone 17 Pro，27.2 支持 iPhone 18 Pro 和 iPhone 17 Pro，另有只给 iPhone Duo 用的 27.2；配置机型 iPhone 17 Pro。
//! 假 SDK：arm64 的 35、37.0，只有 x86_64 的 36；配置 AVD test_avd 用 35、pixel_6、4096M。

use std::time::{SystemTime, UNIX_EPOCH};

use crate::support::*;

fn created(e: &Env) -> Vec<String> {
    std::fs::read_to_string(e.path("sim/created.log")).unwrap_or_default().lines().map(String::from).collect()
}

fn rt(v: &str) -> String {
    format!("com.apple.CoreSimulator.SimRuntime.iOS-{}", v.replace('.', "-"))
}

// CASE-HOLD-13
#[test]
fn ios_version_and_model_are_combined_with_defaults() {
    let e = Env::new();
    for (arg, want) in [
        ("ios-sim@26.2", format!("iPhone 17 Pro {}", rt("26.2"))),
        ("ios-sim@26.2:iPhone 17 Pro", format!("iPhone 17 Pro {}", rt("26.2"))),
        ("ios-sim:iPhone 18 Pro", format!("iPhone 18 Pro {}", rt("27.2"))),
        ("ios-sim@27.2", format!("iPhone 17 Pro {}", rt("27.2"))),
    ] {
        let r = e.aq(&["hold", arg]);
        assert_eq!((r.code, r.status()), (0, "held"), "{arg}: {}", r.stdout);
        assert_eq!(r.one().as_object().map(|o| o.len()), Some(3), "输出字段不变（id、status、devices）：{}", r.stdout);
        let line = created(&e).pop().unwrap_or_default();
        assert!(line.ends_with(&want), "{arg}: {line}");
        assert_eq!(e.aq(&["release", &r.id()]).status(), "released");
    }
    let r = e.aq(&["hold", "ios-sim@26.2", "ios-sim@27.2"]);
    assert_eq!(r.status(), "held", "{}", r.stdout);
    let c = created(&e);
    assert!(c[c.len() - 2].ends_with(&rt("26.2")) && c[c.len() - 1].ends_with(&rt("27.2")), "一组里各用各的版本：{c:?}");
}

// CASE-HOLD-19
#[test]
fn android_version_and_model_are_combined_with_the_configured_avd() {
    let e = Env::new();
    for (arg, pkg, dev) in [
        ("android-emu@35", "system-images;android-35;google_apis;arm64-v8a", "pixel_6"),
        ("android-emu:pixel_tablet", "system-images;android-35;google_apis;arm64-v8a", "pixel_tablet"),
        ("android-emu@35:pixel_7", "system-images;android-35;google_apis;arm64-v8a", "pixel_7"),
        ("android-emu@35:Nexus 5X", "system-images;android-35;google_apis;arm64-v8a", "Nexus 5X"),
        ("android-emu@37.0", "system-images;android-37.0;google_apis_playstore_ps16k;arm64-v8a", "pixel_6"),
    ] {
        let r = e.aq(&["hold", arg]);
        assert_eq!((r.code, r.status()), (0, "held"), "{arg}: {}", r.stdout);
        let name = e.dev_name(&r.id(), 1);
        let calls = std::fs::read_to_string(e.path("emu/avdmanager.log")).unwrap_or_default();
        assert!(calls.contains(&format!("create avd -n {name} -k {pkg} -d {dev}")), "{arg}: {calls}");
        assert!(e.aq_avd_config(&name).contains("hw.ramSize=4096M"), "内存照抄配置 AVD：{}", e.aq_avd_config(&name));
        assert_eq!(e.aq(&["release", &r.id()]).status(), "released");
    }
    let r = e.aq(&["hold", "ios-sim@26.2", "android-emu@35:pixel_7"]);
    assert_eq!(r.status(), "held", "混借：{}", r.stdout);
}

fn rejected(e: &Env, args: &[&str]) -> String {
    let r = e.aq(args);
    assert_eq!((r.code, r.status(), r.one()["reason"].as_str()), (5, "rejected", Some("unschedulable")), "{args:?}: {}", r.stdout);
    assert!(r.one().get("id").is_none(), "不进账本：{}", r.stdout);
    r.one()["message"].as_str().unwrap_or_default().to_string()
}

// CASE-HOLD-14
#[test]
fn unfit_ios_combinations_are_rejected_at_submit() {
    let e = Env::new();
    let m = rejected(&e, &["hold", "ios-sim@18.6"]);
    assert!(
        m.contains("no iOS 18.6 simulator runtime")
            && m.contains("installed: 26.2, 27.2")
            && m.contains("xcodebuild -downloadPlatform iOS -buildVersion 18.6"),
        "{m}"
    );
    let m = rejected(&e, &["hold", "ios-sim@26"]);
    assert!(m.contains("Write the full version, e.g. ios-sim@26.2") && !m.contains("xcodebuild"), "{m}");
    let m = rejected(&e, &["hold", "ios-sim@26.2:iPhone 18 Pro"]);
    assert!(m.contains("iOS 26.2 does not support iPhone 18 Pro; it supports: iPhone 17 Pro. E.g. ios-sim@26.2:iPhone 17 Pro"), "{m}");
    let m = rejected(&e, &["hold", "ios-sim@26.2", "android-emu@34"]);
    assert!(m.contains("no arm64 Android API 34"), "一组里有一项不行就整组拒绝：{m}");
    assert!(created(&e).is_empty() && e.aq_avds().is_empty(), "不建任何设备");
    assert_eq!(e.q_i64("SELECT count(*) FROM holds"), 0);
    e.write_config(&DEFAULT_CONFIG.replace("[ios-sim]\n", "[ios-sim]\nmodel = \"iPhone 18 Pro\"\n"));
    let m = rejected(&e, &["hold", "ios-sim@26.2"]);
    assert!(m.contains("(the default model, [ios-sim] model in config.toml)"), "{m}");
}

// CASE-HOLD-14
#[test]
fn unfit_android_combinations_are_rejected_at_submit() {
    let e = Env::new();
    let m = rejected(&e, &["hold", "android-emu@34"]);
    assert!(m.contains("installed: 35, 37.0") && m.contains("sdkmanager \"system-images;android-34;google_apis;arm64-v8a\""), "{m}");
    let m = rejected(&e, &["hold", "android-emu@36"]);
    assert!(m.contains("installed only as x86_64"), "{m}");
    let m = rejected(&e, &["hold", "android-emu@35:nope"]);
    assert!(m.contains("no Android device profile named nope"), "{m}");
    let m = rejected(&e, &["hold", "android-emu@35:wearos_rect"]);
    assert!(m.contains("wearos_rect is a Wear OS profile"), "{m}");
    assert!(e.aq_avds().is_empty() && e.emus().is_empty());
}

// CASE-HOLD-15
#[test]
fn spec_on_other_devices_and_bad_forms_are_usage_errors() {
    let e = Env::new();
    for (arg, want) in [
        ("ios-device@27", "ios-device does not take @version or :model"),
        ("browser:x", "browser does not take"),
        ("desktop@1", "desktop does not take"),
        ("ios-sim@", "empty version"),
        ("ios-sim@26.2:", "empty model"),
        ("ios-sim@26.2@27.2", "more than one @"),
    ] {
        let r = e.aq(&["hold", arg]);
        assert_eq!((r.code, r.one()["reason"].as_str()), (3, Some("usage")), "{arg}: {}", r.stdout);
        assert!(r.one()["message"].as_str().unwrap_or_default().contains(want), "{arg}: {}", r.stdout);
    }
}

// CASE-HOLD-16
#[test]
fn plain_android_emu_builds_a_new_avd_from_the_configured_one() {
    let e = Env::new();
    let before = std::fs::read_to_string(e.path(".android/avd/test_avd.avd/config.ini")).unwrap();
    let r = e.aq(&["hold", "android-emu"]);
    assert_eq!(r.status(), "held", "{}", r.stdout);
    let name = e.dev_name(&r.id(), 1);
    let cfg = e.aq_avd_config(&name);
    assert!(
        cfg.contains("system-images/android-35/google_apis/arm64-v8a/") && cfg.contains("pixel_6") && cfg.contains("hw.ramSize=4096M"),
        "{cfg}"
    );
    assert_eq!(std::fs::read_to_string(e.path("emu/avd.last")).unwrap().trim(), name, "启动的是新 AVD");
    assert!(!std::fs::read_to_string(e.path("emu/calls.log")).unwrap().contains("test_avd"), "配置 AVD 从不被启动");
    assert_eq!(std::fs::read_to_string(e.path(".android/avd/test_avd.avd/config.ini")).unwrap(), before, "配置 AVD 内容不变");
}

// CASE-HOLD-17
#[test]
fn hold_written_before_upgrade_still_works() {
    let e = Env::new();
    e.aq(&["status"]); // 建好账本
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let sql = format!(
        r#"INSERT INTO holds (id, status, request, owner_pid, owner_started_at, requested_at) VALUES ('hd_old', 'waiting', '{{"devices":["ios-sim"],"accounts":[]}}', 0, 0, {now})"#
    );
    e.db().execute(&sql, []).unwrap();
    let r = e.aq(&["status", "hd_old", "--wait", "20"]);
    assert_eq!(r.status(), "held", "{}", r.stdout);
    assert!(created(&e).pop().unwrap_or_default().ends_with(&format!("iPhone 17 Pro {}", rt("27.2"))), "按默认值新建");
}

// CASE-HOLD-18
#[test]
fn help_shows_device_types_spec_form_and_examples() {
    let e = Env::new();
    let out = |args: &[&str]| String::from_utf8_lossy(&e.command(args).output().unwrap().stdout).into_owned();
    let h = out(&["hold", "--help"]);
    for want in [
        "ios-sim",
        "android-emu",
        "ios-device",
        "android-device",
        "browser",
        "desktop",
        "[@<version>][:<model>]",
        "Examples:",
        "aq hold ios-sim@26.2",
    ] {
        assert!(h.contains(want), "缺 {want}：{h}");
    }
    assert!(h.contains("Test account to borrow") && h.contains("Wait at most this many seconds"), "{h}");
    assert!(out(&["--help"]).contains("aq hold <device>[@<version>][:<model>]"));
}

// CASE-RECLAIM-13
#[test]
fn android_avd_goes_with_the_instance_and_leftovers_are_swept() {
    let e = Env::new();
    let r = e.aq(&["hold", "android-emu@35:pixel_7"]);
    let name = e.dev_name(&r.id(), 1);
    assert_eq!(e.aq_avds(), std::slice::from_ref(&name));
    assert_eq!(e.aq(&["release", &r.id()]).status(), "released");
    assert!(until(10, || e.aq_avds().is_empty()), "实例结束后 AVD 一并删除：{:?}", e.aq_avds());
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 0), "删完才不计内存");
    // 建好 AVD 还没启动就被杀留下的半成品；别的账本的 AVD 不动
    let mine = e.dev_name("hd_gone", 1);
    let theirs = aq_core::devname::name("otherledger", "hd_x", 1);
    for n in [&mine, &theirs] {
        std::fs::create_dir_all(e.state().join(format!("emu/avd/{n}.avd"))).unwrap();
    }
    assert!(
        until(15, || {
            e.aq(&["status"]);
            e.aq_avds() == [theirs.clone()]
        }),
        "{:?}",
        e.aq_avds()
    );
}
