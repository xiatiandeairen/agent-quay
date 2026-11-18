//! 电脑保护：aq 启动的进程降低 CPU 优先级。

use crate::support::*;

fn log_text(e: &Env, r: &Res) -> String {
    let p = r.one()["log"].as_str().unwrap_or_else(|| panic!("没有日志：{}", r.stdout)).to_string();
    let _ = e;
    std::fs::read_to_string(p).unwrap_or_default()
}

const SHOW_NICE: &str = "ps -o nice= -p $$; sh -c 'ps -o nice= -p $$'";

// CASE-PROT-1
#[test]
fn command_and_children_run_at_nice_10() {
    let e = Env::new();
    let r = e.aq(&["run", "--fresh", "--", "sh", "-c", SHOW_NICE]);
    assert_eq!(r.status(), "ok", "{}", r.stdout);
    let v: Vec<String> = log_text(&e, &r).split_whitespace().map(String::from).collect();
    assert_eq!(v, vec!["10", "10"], "命令和它的子进程都是 nice 10");
}

// CASE-PROT-2
#[test]
fn nice_config_zero_and_out_of_range() {
    let e = Env::with_config(&format!("nice = 0\n{DEFAULT_CONFIG}"));
    let r = e.aq(&["run", "--fresh", "--", "sh", "-c", SHOW_NICE]);
    let v: Vec<String> = log_text(&e, &r).split_whitespace().map(String::from).collect();
    assert_eq!(v, vec!["0", "0"], "nice = 0 不降");
    e.write_config(&format!("nice = 25\n{DEFAULT_CONFIG}"));
    let bad = e.aq(&["status"]);
    assert_eq!((bad.code, bad.one()["reason"].as_str()), (3, Some("usage")), "{}", bad.stdout);
    assert!(bad.one()["message"].as_str().unwrap_or("").contains("nice"), "{}", bad.stdout);
}

// CASE-PROT-3
#[test]
fn emulator_and_browser_run_at_configured_nice() {
    let e = Env::new();
    let h = e.aq(&["hold", "android-emu", "browser", "--wait", "30"]);
    assert_eq!(h.status(), "held", "{}", h.stdout);
    let read = |p: &str| std::fs::read_to_string(e.path(p)).unwrap_or_default().trim().to_string();
    assert_eq!(read("emu/nice.last"), "10", "Android 模拟器");
    assert_eq!(read("browser/nice.last"), "10", "浏览器会话");
    e.aq(&["release", &h.id()]);
}
