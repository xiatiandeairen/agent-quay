//! 复用。环境 。
//! 用「同时提交、看是否挂到同一次执行」黑盒判断 hash 是否相同。

use std::path::Path;

use crate::support::*;

/// 在 a、b 两处各提交一条命令（先 a 再 b，a 还在跑时提交 b），返回 b 是否复用了 a。
fn merged(e: &Env, a: (&Path, &[&str]), b: (&Path, &[&str])) -> bool {
    let mut aa = vec!["run", "--wait", "0", "--"];
    aa.extend_from_slice(a.1);
    let ra = e.aq_in(a.0, &aa);
    let mut bb = vec!["run", "--wait", "0", "--"];
    bb.extend_from_slice(b.1);
    let rb = e.aq_in(b.0, &bb);
    let same = rb.id() == ra.id();
    assert_eq!(rb.one()["reused"].as_bool(), Some(same));
    e.aq(&["cancel", &ra.id()]);
    e.aq(&["cancel", &rb.id()]);
    same
}

const CMD: &[&str] = &["fake-task", "--secs", "20"];

fn worktree(repo: &Path, name: &str) -> std::path::PathBuf {
    let wt = repo.parent().unwrap().join(name);
    git(repo, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]);
    wt
}

// CASE-REUSE-1, CASE-REUSE-2
#[test]
fn same_commit_clean_merges_across_worktrees() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    assert!(merged(&e, (&repo, CMD), (&repo, CMD)), "CASE-REUSE-1");
    let wt = worktree(&repo, "wt2");
    assert!(merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-1");
    std::fs::write(wt.join("a.txt"), "changed\n").unwrap();
    assert!(!merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-2");
}

// CASE-REUSE-2
#[test]
fn identical_dirty_and_revert() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let wt = worktree(&repo, "wt2");
    std::fs::write(repo.join("a.txt"), "same edit\n").unwrap();
    std::fs::write(wt.join("a.txt"), "same edit\n").unwrap();
    assert!(merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-2");
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    std::fs::write(wt.join("a.txt"), "hello\n").unwrap();
    let r = e.aq_in(&repo, &["run", "--wait", "0", "--", "true"]);
    let fp = e.q_str(&format!("SELECT fingerprint FROM executions WHERE id='{}'", r.id())).unwrap();
    assert!(!fp.contains("dirty"), "CASE-REUSE-2 改回原样后指纹回到 SHA：{fp}");
}

// CASE-REUSE-2
#[test]
fn untracked_counts_ignored_does_not() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let wt = worktree(&repo, "wt2");
    std::fs::create_dir_all(wt.join("build")).unwrap();
    std::fs::write(wt.join("build/out.o"), "x").unwrap();
    assert!(merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-2");
    std::fs::write(wt.join("notes.md"), "n").unwrap();
    assert!(!merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-2");
}

// CASE-REUSE-2
#[test]
fn submodule_change_counts() {
    let e = Env::new();
    let sub = e.work().join("subsrc");
    git_repo(&sub);
    let repo = e.work().join("repo");
    git_repo(&repo);
    let out = std::process::Command::new("git")
        .args(["-c", "protocol.file.allow=always", "submodule", "add", "-q", sub.to_str().unwrap(), "External/Sub"])
        .current_dir(&repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(&repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qam", "sub"]);
    let before = e.aq_in(&repo, &["run", "--", "true"]);
    let fp0 = e.q_str(&format!("SELECT fingerprint FROM executions WHERE id='{}'", before.id())).unwrap();
    std::fs::write(repo.join("External/Sub/a.txt"), "sub change\n").unwrap();
    let after = e.aq_in(&repo, &["run", "--", "true"]);
    let fp1 = e.q_str(&format!("SELECT fingerprint FROM executions WHERE id='{}'", after.id())).unwrap();
    assert!(!fp0.contains("dirty"));
    assert!(fp1.contains("dirty"), "CASE-REUSE-2 子模块改动要进指纹：{fp1}");
}

// CASE-REUSE-3
#[test]
fn cwd_and_argv_text() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let sub = repo.join("Modules");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(!merged(&e, (&repo, CMD), (&sub, CMD)), "CASE-REUSE-3");
    assert!(
        !merged(&e, (&repo, &["fake-task", "--secs", "20", "--exit", "0"]), (&repo, &["fake-task", "--secs", "20", "--exit", "1"])),
        "CASE-REUSE-3"
    );
    assert!(
        !merged(&e, (&repo, &["fake-task", "--secs", "20", "--exit", "0"]), (&repo, &["fake-task", "--exit", "0", "--secs", "20"])),
        "CASE-REUSE-3"
    );
}

// CASE-REUSE-3
#[test]
fn session_path_differs_env_ignored() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let a = ["fake-task", "--secs", "20", "--touch", "/private/tmp/s1/x"];
    let b = ["fake-task", "--secs", "20", "--touch", "/private/tmp/s2/x"];
    assert!(!merged(&e, (&repo, &a), (&repo, &b)), "CASE-REUSE-3");
    let ra = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    e.env("DEVELOPER_DIR", "/elsewhere");
    let rb = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    assert_eq!(ra.id(), rb.id(), "CASE-REUSE-3 环境变量不进 hash");
    e.aq(&["cancel", &ra.id()]);
}

// CASE-REUSE-2
#[test]
fn rebased_same_content_different_sha() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let wt = worktree(&repo, "wt2");
    git(
        &wt,
        &[
            "-c",
            "user.email=x@x",
            "-c",
            "user.name=x",
            "commit",
            "-q",
            "--allow-empty",
            "--amend",
            "--no-edit",
            "--date=2001-01-01T00:00:00",
        ],
    );
    assert!(!merged(&e, (&repo, CMD), (&wt, CMD)), "CASE-REUSE-2");
}

// CASE-REUSE-4
#[test]
fn fresh_and_finished() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let first = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    let fresh = e.aq_in(&repo, &["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    assert_ne!(fresh.id(), first.id(), "CASE-REUSE-4/CASE-REUSE-4 --fresh 新开执行");
    assert_eq!(e.q_i64(&format!("SELECT hash IS NULL FROM executions WHERE id='{}'", fresh.id())), 1);
    assert_eq!(e.q_i64(&format!("SELECT fingerprint IS NOT NULL FROM executions WHERE id='{}'", fresh.id())), 1, "CASE-REUSE-4");
    let plain = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "20"]);
    assert_eq!(plain.id(), first.id(), "CASE-REUSE-4 不带 --fresh 的挂到最早那次");
    e.aq(&["cancel", &first.id()]);
    e.aq(&["cancel", &fresh.id()]);
    let done = e.aq_in(&repo, &["run", "--", "true"]);
    let again = e.aq_in(&repo, &["run", "--", "true"]);
    assert_ne!(done.id(), again.id(), "CASE-REUSE-4/CASE-REUSE-4 已结束的不复用");
    assert_eq!(again.one()["reused"], false);
}

// CASE-REUSE-1
#[test]
fn many_worktrees_same_content_run_once() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let count = e.path("count");
    let mut dirs = vec![repo.clone()];
    for i in 0..5 {
        dirs.push(worktree(&repo, &format!("wt{i}")));
    }
    let cmd = ["run", "--wait", "30", "--", "fake-task", "--secs", "2", "--count", count.to_str().unwrap()];
    let kids: Vec<_> = dirs
        .iter()
        .map(|d| {
            let mut c = e.command(&cmd);
            c.current_dir(d).stdout(std::process::Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let outs: Vec<Res> = kids.into_iter().map(|k| parse(k.wait_with_output().unwrap())).collect();
    let ids: std::collections::HashSet<String> = outs.iter().map(|o| o.id()).collect();
    assert_eq!(ids.len(), 1, "只执行一次");
    assert!(outs.iter().all(|o| o.status() == "ok"));
    assert_eq!(outs.iter().filter(|o| o.one()["reused"] == false).count(), 1);
    assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1, "计数文件 = 1");
}

// CASE-REUSE-5
#[test]
fn content_changed_while_running() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let r = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    std::fs::write(repo.join("a.txt"), "edited during build\n").unwrap();
    let s = e.aq(&["status", &r.id(), "--wait", "20"]);
    assert_eq!(s.status(), "ok");
    assert_eq!(s.one()["content_changed"], true);
}

// CASE-REUSE-6
#[test]
fn fingerprint_never_touches_index_or_lock() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
    let st0 = git(&repo, &["status", "--porcelain"]);
    let idx = repo.join(".git/index");
    let before = std::fs::metadata(&idx).unwrap().modified().unwrap();
    std::fs::write(repo.join(".git/index.lock"), "").unwrap(); // CASE-REUSE-6：别人持有 index.lock
    let r = e.aq_in(&repo, &["run", "--", "true"]);
    assert_eq!(r.status(), "ok");
    assert!(e.q_str(&format!("SELECT fingerprint FROM executions WHERE id='{}'", r.id())).unwrap().contains("dirty"));
    assert!(repo.join(".git/index.lock").exists(), "别人的 index.lock 不被删除");
    assert_eq!(std::fs::metadata(&idx).unwrap().modified().unwrap(), before, ".git/index 不被改写");
    std::fs::remove_file(repo.join(".git/index.lock")).unwrap();
    assert_eq!(git(&repo, &["status", "--porcelain"]), st0);
}

// CASE-RUN-3
#[test]
fn odd_cwd_and_argv_run_verbatim_and_hash_stable() {
    let e = Env::new();
    let dir = e.work().join("目录 with space");
    git_repo(&dir);
    let arg = "it's \"quoted\"\nline2";
    let r = e.aq_in(&dir, &["run", "--", "printf", "%s", arg]);
    assert_eq!(r.status(), "ok");
    assert_eq!(std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap(), arg);
    assert!(merged(&e, (&dir, &["fake-task", "--secs", "20", "--touch", arg]), (&dir, &["fake-task", "--secs", "20", "--touch", arg])));
}

// CASE-REUSE-1
#[test]
fn reuse_is_logged_on_the_shared_execution() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let a = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    let b = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    assert_eq!(b.id(), a.id());
    let log = e.aq_log();
    assert_eq!(log.lines().filter(|l| l.contains(&a.id()) && l.contains("reused by a new submission")).count(), 1, "{log}");
    e.aq(&["status", &a.id(), "--wait", "20"]);
}
