use super::*;

const UDID: &str = "CD41CCB9-3F02-471C-80E3-0A81A11718E4";
const OTHER: &str = "0A1B2C3D-0000-4000-8000-000000000001";

fn p(pid: i64, ppid: i64, exe: &str, argv: &[&str]) -> ProcInfo {
    ProcInfo { pid, ppid, exe: exe.into(), argv: argv.iter().map(|s| s.to_string()).collect(), aq_descendant: false }
}

const XCB: &str = "/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild";

// CASE-RECLAIM-11
#[test]
fn c1_xcodebuild_destination_counts() {
    let x = p(10, 1, XCB, &["xcodebuild", "test", "-destination", &format!("platform=iOS Simulator,id={UDID}")]);
    assert!(uses_device(&x, UDID));
    let lower = p(10, 1, XCB, &["xcodebuild", "-destination", &format!("id={}", UDID.to_lowercase())]);
    assert!(uses_device(&lower, UDID), "不分大小写");
}

// CASE-RECLAIM-11
#[test]
fn c2_agent_device_runner_counts() {
    // 本机 2026-11-19 实测的 agent-device iOS runner 参数（节选）
    let xctestrun = format!(
        "/Users/u/.agent-device/apple-runner/derived/ios-simulator/cache-ef907f/Build/Products/AgentDeviceRunner.env.session-{UDID}-owner-18377-245f.xctestrun"
    );
    let dest = format!("platform=iOS Simulator,id={UDID}");
    let r = p(
        22597,
        18377,
        XCB,
        &[
            XCB,
            "test-without-building",
            "-only-testing",
            "AgentDeviceRunnerUITests/RunnerTests/testCommand",
            "-xctestrun",
            &xctestrun,
            "-destination",
            &dest,
        ],
    );
    assert!(uses_device(&r, UDID));
}

// CASE-RECLAIM-11
#[test]
fn c3_process_inside_simulator_does_not_count() {
    let app =
        format!("/Users/u/Library/Developer/CoreSimulator/Devices/{UDID}/data/Containers/Bundle/Application/X/MultiTodo.app/MultiTodo");
    assert!(!uses_device(&p(30, 22001, &app, &[&app]), UDID), "装进模拟器的 App");
    let launchd = "/Library/Developer/CoreSimulator/Volumes/iOS_23C54/Library/Developer/CoreSimulator/Profiles/Runtimes/iOS 26.2.simruntime/Contents/Resources/RuntimeRoot/sbin/launchd_sim";
    let plist = format!("/Users/u/Library/Developer/CoreSimulator/Devices/{UDID}/data/var/run/launchd_bootstrap.plist");
    assert!(!uses_device(&p(22001, 1, launchd, &["launchd_sim", &plist]), UDID), "launchd_sim：UDID 只在 Devices 路径里");
}

// CASE-RECLAIM-11
#[test]
fn c4_aq_and_its_descendants_do_not_count() {
    let known: HashSet<String> = ["hd_mine".to_string()].into();
    let dest = format!("id={UDID}");
    let mut procs = vec![
        p(100, 1, "/Users/u/.local/bin/aq", &["aq", "__runner", "hd_mine"]),
        p(101, 100, "/usr/bin/xcrun", &["xcrun", "simctl", "boot", UDID]),
        p(102, 101, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", &["chrome", &dest]),
        p(110, 1, "/Users/u/.local/bin/aq", &["aq", "run", "--", "xcodebuild", "-destination", &dest]),
        p(200, 1, "/Users/u/.local/bin/aq", &["aq", "__runner", "ex_other_ledger"]),
        p(201, 200, "/bin/sh", &["sh", "-c", &format!("use {UDID}")]),
        p(300, 1, "/tmp/target/debug/aq", &["aq", "__runner", "hd_other_ledger"]),
        p(301, 300, "/usr/bin/xcrun", &["xcrun", "simctl", "boot", UDID]),
    ];
    mark_aq_descendants(&mut procs, &known);
    let flags: Vec<bool> = procs.iter().map(|p| p.aq_descendant).collect();
    assert_eq!(flags, [true, true, true, true, false, false, true, true], "别的账本执行命令的 runner 不算 aq 的，借用的 runner 算");
    assert_eq!(devices_in_use(&procs, [UDID]), [UDID.to_string()].into(), "只有 201 算");
    procs.truncate(4);
    assert!(devices_in_use(&procs, [UDID]).is_empty(), "aq 自己、本账本 runner 起的 simctl 和 Chrome 都不算");
}

// CASE-RECLAIM-12
#[test]
fn c5_adb_with_serial_counts() {
    let adb =
        p(40, 1, "/Users/u/Library/Android/sdk/platform-tools/adb", &["adb", "-s", "emulator-5600", "shell", "input", "tap", "1", "2"]);
    assert!(uses_device(&adb, "emulator-5600"));
}

// CASE-RECLAIM-12
#[test]
fn c6_emulator_body_does_not_count() {
    let qemu = p(
        50,
        1,
        "/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64",
        &["qemu-system-aarch64", "-avd", "aq", "-port", "5600", "emulator-5600"],
    );
    let emu = p(51, 1, "/sdk/emulator/emulator", &["emulator", "-avd", "aq", "-read-only", "-port", "5600", "emulator-5600"]);
    assert!(!uses_device(&qemu, "emulator-5600") && !uses_device(&emu, "emulator-5600"));
}

// CASE-RECLAIM-12
#[test]
fn c7_serial_must_match_whole_token() {
    for arg in ["emulator-56001", "emulator-560", "xemulator-5600", "emulator-5600_2"] {
        assert!(!uses_device(&p(60, 1, "/bin/adb", &["adb", "-s", arg]), "emulator-5600"), "{arg}");
    }
    for arg in ["emulator-5600", "serial=emulator-5600", "emulator-5600:5555", "/tmp/emulator-5600/log"] {
        assert!(uses_device(&p(60, 1, "/bin/adb", &["adb", "-s", arg]), "emulator-5600"), "{arg}");
    }
}

// CASE-RECLAIM-11
#[test]
fn c8_other_udid_does_not_count() {
    let x = p(10, 1, XCB, &["xcodebuild", "-destination", &format!("id={OTHER}")]);
    assert!(!uses_device(&x, UDID));
    assert!(devices_in_use(&[x], [UDID, OTHER]) == [OTHER.to_string()].into());
}
