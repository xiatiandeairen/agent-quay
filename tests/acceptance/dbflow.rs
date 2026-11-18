//! 数据存储的流程用例。每一步之后直接看账本里哪些字段有值。

use std::time::Duration;

use crate::support::*;

fn row(e: &Env, sql: &str) -> Vec<Option<String>> {
    e.db()
        .query_row(sql, [], |r| {
            let n = r.as_ref().column_count();
            (0..n).map(|i| r.get::<_, rusqlite_value::V>(i).map(|v| v.0)).collect()
        })
        .unwrap_or_else(|err| panic!("{sql}: {err}"))
}

/// 把任意列读成「有没有值」的字符串，便于断言。
mod rusqlite_value {
    pub struct V(pub Option<String>);
    impl aq_store::FromSql for V {
        fn column_result(v: aq_store::ValueRef<'_>) -> aq_store::FromSqlResult<Self> {
            Ok(V(match v {
                aq_store::ValueRef::Null => None,
                aq_store::ValueRef::Integer(i) => Some(i.to_string()),
                aq_store::ValueRef::Real(f) => Some(f.to_string()),
                aq_store::ValueRef::Text(t) | aq_store::ValueRef::Blob(t) => Some(String::from_utf8_lossy(t).into_owned()),
            }))
        }
    }
}

fn set(v: &[Option<String>]) -> Vec<bool> {
    v.iter().map(Option::is_some).collect()
}

// CASE-STORE-1, CASE-EST-3
#[test]
fn exec_fields_per_step() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    e.set_ai("fail");
    let repo = e.work().join("repo");
    git_repo(&repo);
    let blocker = e.aq_in(&repo, &["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "3"]);
    let r = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "1"]);
    let q = format!(
        "SELECT hash, fingerprint, cwd, argv, profile_key, submitted_at, started_at, pgid, exit_code FROM executions WHERE id='{}'",
        r.id()
    );
    let v = set(&row(&e, &q));
    assert_eq!(v, [true, true, true, true, true, true, false, false, false], "CASE-STORE-1 提交后");
    assert!(
        until(10, || set(&row(&e, &format!("SELECT heavy, mem_mb, disk_mb, uses_net, timeout_s FROM executions WHERE id='{}'", r.id())))
            .iter()
            .all(|x| *x)),
        "CASE-STORE-1 资源评估已写入"
    );
    assert_eq!(e.q_i64("SELECT count(*) FROM profiles"), 0, "CASE-EST-3 AI 失败不写 profiles");
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    let v = set(&row(&e, &format!("SELECT started_at, pgid, pgid_started_at FROM executions WHERE id='{}'", r.id())));
    assert_eq!(v, [true, true, true], "CASE-STORE-1 放行");
    e.aq(&["status", &r.id(), &blocker.id(), "--wait", "20"]);
    let v = set(&row(
        &e,
        &format!(
            "SELECT finished_at, exit_code, peak_mem_mb, cpu_avg_cores, disk_delta_mb, content_changed FROM executions WHERE id='{}'",
            r.id()
        ),
    ));
    assert_eq!(v, [true; 6], "CASE-STORE-1 结束");
}

// CASE-REUSE-1
#[test]
fn same_hash_no_new_row() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let kids: Vec<_> = (0..6)
        .map(|_| {
            let mut c = e.command(&["run", "--wait", "0", "--", "fake-task", "--secs", "5"]);
            c.current_dir(&repo).stdout(std::process::Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let outs: Vec<Res> = kids.into_iter().map(|k| parse(k.wait_with_output().unwrap())).collect();
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 1);
    assert_eq!(outs.iter().filter(|o| o.one()["reused"] == true).count(), 5);
    e.aq(&["cancel", &outs[0].id()]);
}

// CASE-EST-1, CASE-EST-5
#[test]
fn ai_success_writes_profile_then_measured_wins() {
    let e = Env::new();
    e.aq(&["run", "--", "fake-task", "--mem", "50M"]);
    assert_eq!(e.q_i64("SELECT count(*) FROM profiles"), 1, "CASE-EST-1 AI 成功写 profiles");
    let r = e.aq(&["run", "--", "fake-task", "--mem", "50M"]);
    let ai_mem = e.q_i64("SELECT mem_mb FROM profiles");
    assert_ne!(e.q_i64(&format!("SELECT mem_mb FROM executions WHERE id='{}'", r.id())), ai_mem, "CASE-EST-5 有实测就不用 profiles");
}

// CASE-STORE-1
#[test]
fn timeout_and_cancel_fields() {
    let e = Env::new();
    e.env("AQ_T_TIMEOUT_MIN_S", "1").env("AQ_T_TIMEOUT_MAX_S", "1");
    let t = e.aq(&["run", "--wait", "20", "--", "fake-task", "--secs", "30"]);
    assert_eq!(t.status(), "timeout");
    assert_eq!(
        set(&row(&e, &format!("SELECT finished_at, exit_code FROM executions WHERE id='{}'", t.id()))),
        [true, false],
        "CASE-STORE-1"
    );
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "sh", "-c", "trap '' TERM; sleep 30"]);
    assert!(until(10, || e.aq(&["status", &r.id()]).status() == "running"));
    e.aq(&["cancel", &r.id()]);
    let (st, flag) = (
        e.q_str(&format!("SELECT status FROM executions WHERE id='{}'", r.id())).unwrap(),
        e.q_i64("SELECT cancel_requested FROM executions"),
    );
    assert_eq!((st.as_str(), flag), ("running", 1), "CASE-STORE-1 先置 cancel_requested，状态仍为 running");
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).status(), "cancelled", "进程组结束后改为 cancelled");
}

// CASE-STORE-1
#[test]
fn hold_fields_per_step() {
    let e = Env::new();
    e.env("FAKE_SIM_CREATE_SECS", "2");
    let r = e.aq(&["hold", "ios-sim", "ios-device", "--account", "a", "--wait", "0"]);
    let v = set(&row(
        &e,
        &format!("SELECT request, owner_pid, owner_started_at, requested_at, acquired_at, ended_at FROM holds WHERE id='{}'", r.id()),
    ));
    assert_eq!(v, [true, true, true, true, false, false], "CASE-STORE-1");
    assert!(until(5, || e.q_i64("SELECT count(*) FROM hold_devices") == 2));
    assert_eq!(e.q_str("SELECT device FROM hold_devices WHERE type='ios-device'").as_deref(), Some("phone-1"), "CASE-STORE-1 真机行有值");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices WHERE type='ios-sim' AND device IS NULL"), 1, "CASE-STORE-1 模拟器行为空");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_accounts"), 1);
    assert_eq!(e.aq(&["status", &r.id()]).status(), "waiting", "CASE-STORE-1 仍为 waiting");
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "10"]).status(), "held");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_devices WHERE device IS NULL"), 0, "CASE-STORE-1 回填");
    assert_eq!(e.q_i64("SELECT acquired_at IS NOT NULL FROM holds"), 1);
}

// CASE-STORE-1, CASE-RECLAIM-1
#[test]
fn end_fields() {
    let e = Env::new();
    std::fs::write(e.path("sim/fail_create"), "").unwrap();
    let rej = e.aq(&["hold", "ios-sim", "--account", "a"]);
    assert_eq!(set(&row(&e, &format!("SELECT ended_at FROM holds WHERE id='{}'", rej.id()))), [true], "CASE-STORE-1");
    std::fs::remove_file(e.path("sim/fail_create")).unwrap();
    let h = e.aq(&["hold", "ios-sim", "ios-device", "--account", "a"]);
    e.aq(&["release", &h.id()]);
    assert_eq!(set(&row(&e, &format!("SELECT ended_at FROM holds WHERE id='{}'", h.id()))), [true], "CASE-RECLAIM-1");
    assert!(until(10, || e.q_i64("SELECT count(*) FROM hold_devices") == 0), "CASE-RECLAIM-1 删完模拟器后删行");
    e.env("AQ_T_IDLE_RECLAIM_S", "1");
    let g = e.aq(&["hold", "ios-device", "--account", "b"]);
    std::thread::sleep(Duration::from_millis(1200));
    e.aq(&["status"]);
    assert_eq!(e.q_str(&format!("SELECT status FROM holds WHERE id='{}'", g.id())).as_deref(), Some("reclaimed"), "CASE-STORE-1");
    assert_eq!(e.q_i64("SELECT count(*) FROM hold_accounts"), 0);
}

// CASE-RECLAIM-7
#[test]
fn release_racing_reclaim_single_end() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "1");
    let h = e.aq(&["hold", "ios-sim"]);
    std::thread::sleep(Duration::from_millis(1100));
    let a = e.spawn(&["release", &h.id()]);
    let b = e.spawn(&["status"]);
    let _ = (a.wait_with_output(), b.wait_with_output());
    let st = e.q_str(&format!("SELECT status FROM holds WHERE id='{}'", h.id())).unwrap();
    assert!(["released", "reclaimed"].contains(&st.as_str()));
    assert!(until(10, || e.sims().is_empty()));
    let deletes = std::fs::read_to_string(e.path("sim/calls.log")).unwrap().lines().filter(|l| l.starts_with("delete")).count();
    assert_eq!(deletes, 1, "模拟器只删一次");
    let ends = e.aq_log().lines().filter(|l| l.contains(&h.id()) && l.contains("\"from\":\"held\"")).count();
    assert_eq!(ends, 1, "终态唯一");
}

// CASE-STORE-5
#[test]
fn log_total_over_cap_deletes_oldest_finished() {
    let e = Env::new();
    e.env("AQ_T_LOG_TOTAL_MB", "1");
    let ids: Vec<String> = (0..4).map(|_| e.aq(&["run", "--fresh", "--", "fake-task", "--out", "400000"]).id()).collect();
    let exists: Vec<bool> = ids.iter().map(|id| e.state().join(format!("logs/{id}.log")).exists()).collect();
    assert!(!exists[0] && exists[3], "最旧的已结束日志被删：{exists:?}");
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 4, "执行记录保留");
}
