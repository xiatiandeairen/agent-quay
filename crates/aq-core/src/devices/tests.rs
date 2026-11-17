use super::*;

/// 本机实测的 devicectl 输出（Xcode 26），去掉无关字段并把名字、UDID 换成假的：
/// 一台模拟器、一台有线真机、一台同网无线且调试通道没开的真机。
const SAMPLE: &str = include_str!("devicectl-sample.json");

fn seen(kind: DeviceType, id: &str, problem: Option<&str>) -> Seen {
    Seen { kind, id: id.into(), name: id.into(), problem: problem.map(String::from) }
}

// CASE-DEV-1
#[test]
fn devicectl_real_output_wired_and_wireless_usable_sims_skipped() {
    let s = parse_devicectl(SAMPLE).unwrap();
    let got: Vec<(&str, &str, Option<&str>)> = s.iter().map(|d| (d.id.as_str(), d.name.as_str(), d.problem.as_deref())).collect();
    assert_eq!(got, vec![("00008150-000000000001AAAA", "Phone 1", None), ("00008150-000000000002AAAA", "Phone 2", None)]);
}

// CASE-DEV-2
#[test]
fn devicectl_unpaired_devmode_off_no_transport() {
    let mut v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
    let devs = v["result"]["devices"].as_array_mut().unwrap();
    devs[1]["connectionProperties"]["pairingState"] = "unpaired".into();
    devs[2]["deviceProperties"]["developerModeStatus"] = "disabled".into();
    let mut third = devs[2].clone();
    third["hardwareProperties"]["udid"] = "x3".into();
    third["deviceProperties"]["developerModeStatus"] = "enabled".into();
    third["connectionProperties"].as_object_mut().unwrap().remove("transportType");
    devs.push(third);
    let p: Vec<String> = parse_devicectl(&v.to_string()).unwrap().into_iter().map(|d| d.problem.unwrap_or_default()).collect();
    assert!(p[0].contains("未配对") && p[1].contains("开发者模式") && p[2] == NOT_FOUND, "{p:?}");
    assert!(parse_devicectl("not json").is_err());
}

// CASE-DEV-2, CASE-DEV-3
#[test]
fn adb_states_and_emulators() {
    let out = "* daemon started *\nList of devices attached\nemulator-5554\tdevice\nR58M\tdevice usb:1-1 model:Pixel_9 device:x\n\
               U1\tunauthorized usb:1-2\nO1\toffline\n192.168.1.5:5555\tdevice model:Pixel_8\n\n";
    let got: Vec<(String, String, bool)> = parse_adb(out).into_iter().map(|d| (d.id, d.name, d.problem.is_none())).collect();
    assert_eq!(
        got,
        vec![
            ("R58M".into(), "Pixel 9".into(), true),
            ("U1".into(), "U1".into(), false),
            ("O1".into(), "O1".into(), false),
            ("192.168.1.5:5555".into(), "Pixel 8".into(), true),
        ]
    );
}

fn req(devices: Vec<DeviceType>) -> HoldRequest {
    HoldRequest { devices, accounts: vec![], exclude: vec![] }
}

fn cap(ios: &[&str]) -> Capacity {
    Capacity { ios_devices: ios.iter().map(|s| s.to_string()).collect(), ..Capacity::default() }
}

// CASE-DEV-1, CASE-DEV-3
#[test]
fn judge_counts_only_declared_and_usable() {
    use DeviceType::IosDevice as I;
    let scan = vec![seen(I, "p1", None), seen(I, "p2", Some("开发者模式未开启")), seen(I, "own", None)];
    let v = judge(&req(vec![I]), &cap(&["p1", "p2", "p3"]), &[(I, Ok(scan.clone()))]);
    assert_eq!((v.reject.is_none(), v.exclude.clone()), (true, vec!["p2".to_string(), "p3".to_string()]), "p1 可用就放行，排除 p2、p3");
    assert!(v.undeclared.is_empty() && v.snippet.is_none(), "不拒绝时不带未声明设备");

    let v = judge(&req(vec![I, I]), &cap(&["p1", "p2", "p3"]), &[(I, Ok(scan.clone()))]);
    let (reason, msg) = v.reject.unwrap();
    assert_eq!(reason, "device_unavailable");
    assert!(msg.contains("p2：开发者模式未开启") && msg.contains(&format!("p3：{NOT_FOUND}")), "{msg}");
    assert_eq!(v.snippet.as_deref(), Some("[ios-device]\nids = [\"p1\", \"p2\", \"p3\", \"own\"]"));

    let v = judge(&req(vec![I, I, I, I]), &cap(&["p1", "p2", "p3"]), &[(I, Ok(scan))]);
    assert_eq!(v.reject.map(|r| r.0), Some("unschedulable"), "比清单还多是永远满足不了");
}

// CASE-DEV-2
#[test]
fn judge_scan_error_makes_all_declared_unavailable() {
    use DeviceType::AndroidDevice as A;
    let c = Capacity { android_devices: vec!["a1".into()], ..Capacity::default() };
    let v = judge(&req(vec![A]), &c, &[(A, Err("找不到 adb".into()))]);
    assert_eq!(v.reject.map(|r| r.0), Some("device_unavailable"));
    assert_eq!(v.unavailable[0].why, "找不到 adb");
}

#[test]
fn real_types_dedup_in_order() {
    use DeviceType::*;
    assert_eq!(real_types(&req(vec![IosSim, AndroidDevice, IosDevice, AndroidDevice])), vec![AndroidDevice, IosDevice]);
}
