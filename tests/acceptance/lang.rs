//! 输出和交互只用英文（CASE-LANG-1～3）。每次 aq 调用的 stdout、stderr 和测试结束时的 aq.log 都由
//! support 里的 assert_english 检查；这里补上公共封装覆盖不到的入口：帮助、写法错误、配置错误、内部报错。

use std::process::Stdio;

use crate::support::*;

fn raw(e: &Env, args: &[&str]) -> (String, String) {
    let o = e.command(args).stdin(Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

// CASE-LANG-1
#[test]
fn help_usage_and_config_errors_are_english() {
    let e = Env::new();
    for args in [
        &["--help"][..],
        &["run", "--help"],
        &["status", "--help"],
        &["cancel", "--help"],
        &["hold", "--help"],
        &["release", "--help"],
        &["run"],
        &["run", "--", ""],
        &["hold"],
        &["hold", "ipad"],
        &["hold", "browser", "--account", "a", "--account", "a"],
        &["cancel", "ex_nope"],
        &["release", "hd_nope"],
        &["status", "nope"],
        &["frobnicate"],
    ] {
        let (out, err) = raw(&e, args);
        assert_english(&format!("aq {args:?} stdout"), &out, &[]);
        assert_english(&format!("aq {args:?} stderr"), &err, &[]);
    }
    for bad in ["mem = \"lots\"\n", "mem = \"3Q\"\n", "heavy_slots = -1\n", "nice = 99\n", "bogus = 1\n"] {
        e.write_config(bad);
        let (out, err) = raw(&e, &["status"]);
        assert!(out.contains("rejected"), "{out}");
        assert_english(&format!("config {bad:?}"), &(out + &err), &[]);
    }
}

// CASE-LANG-2
#[test]
fn log_of_a_full_lifecycle_is_english() {
    let e = Env::new();
    e.aq(&["run", "--", "fake-task", "--exit", "3"]);
    e.aq(&["run", "--", "no-such-program-here"]);
    let h = e.aq(&["hold", "ios-sim", "browser", "--wait", "30"]);
    e.aq(&["status", "--human"]);
    e.aq(&["release", &h.id()]);
    assert!(until(10, || e.sims().is_empty()));
    assert!(!e.aq_log().is_empty());
    // aq.log 由 Env 结束时检查
}

// CASE-LANG-3
#[test]
fn user_text_is_echoed_as_is() {
    let e = Env::new();
    let dir = e.work().join("中文 目录");
    std::fs::create_dir_all(&dir).unwrap();
    let r = e.aq_in(&dir, &["run", "--", "echo", "你好"]);
    assert_eq!(r.status(), "ok");
    let human = e.aq_in(&dir, &["status", &r.id(), "--human"]);
    assert!(human.stdout.contains("中文 目录"), "执行目录原样显示：{}", human.stdout);
}
