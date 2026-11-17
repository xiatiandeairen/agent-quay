//! 数据存储。
//! 迁移夹具 tests/storage/fixtures/vN/ 写下后冻结（红区）。
// 测试代码：断言失败就该 panic 并打印上下文，unwrap/expect/panic 是期望行为
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use aq_store::{Conn, Store};

fn fresh() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("aq.db")).unwrap();
    (d, s)
}

fn ok(c: &Conn, sql: &str) {
    c.execute_batch(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn rejected(c: &Conn, sql: &str) {
    assert!(c.execute_batch(sql).is_err(), "应被拒绝：{sql}");
}

const EXEC: &str = "INSERT INTO executions (id, hash, status, cwd, argv, profile_key, submitted_at";

// 表外：文档与迁移文件一致性检查，不守需求行为
#[test]
fn schema_matches_docs_schema_sql() {
    let (_d, s) = fresh();
    let mem = Conn::open_in_memory().unwrap();
    mem.execute_batch(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.sql")).unwrap()).unwrap();
    let a = aq_store::schema_dump(s.conn()).unwrap();
    let b = aq_store::schema_dump(&mem).unwrap();
    assert_eq!(a, b, "迁移后的结构必须与 docs/schema.sql 一致");
}

// CASE-STORE-6
#[test]
fn new_db_is_wal_version_and_fk() {
    let (_d, s) = fresh();
    let c = s.conn();
    let mode: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let fk: i64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
    let bt: i64 = c.query_row("PRAGMA busy_timeout", [], |r| r.get(0)).unwrap();
    assert_eq!((mode.as_str(), v, fk, bt), ("wal", aq_store::SCHEMA_VERSION, 1, 5000));
}

// CASE-STORE-6
#[test]
fn old_fixture_upgrades_keeping_data() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("aq.db");
    {
        let c = Conn::open(&p).unwrap();
        c.execute_batch(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/storage/fixtures/v1/aq.sql")).unwrap())
            .unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
    }
    let s = Store::open(&p).unwrap();
    let n: i64 = s.conn().query_row("SELECT count(*) FROM executions", [], |r| r.get(0)).unwrap();
    let v: i64 = s.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!((n, v), (2, aq_store::SCHEMA_VERSION));
    assert!(aq_store::active_holds(s.conn()).unwrap().iter().any(|h| h.request.devices.len() == 1));
}

// CASE-STORE-2
#[test]
fn one_inflight_per_hash() {
    let (_d, s) = fresh();
    let c = s.conn();
    ok(c, &format!("{EXEC}) VALUES ('a','h','queued','/','[]','k',1)"));
    rejected(c, &format!("{EXEC}) VALUES ('b','h','queued','/','[]','k',1)"));
    ok(c, &format!("{EXEC}) VALUES ('c',NULL,'queued','/','[]','k',1)"));
    ok(c, &format!("{EXEC}) VALUES ('d',NULL,'queued','/','[]','k',1)"));
}

// CASE-STORE-2
#[test]
fn device_account_and_fk() {
    let (_d, s) = fresh();
    let c = s.conn();
    for id in ["h1", "h2"] {
        ok(
            c,
            &format!(
                "INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at) VALUES ('{id}','waiting','{{}}',1,1,1)"
            ),
        );
    }
    ok(c, "INSERT INTO hold_devices VALUES ('h1',1,'ios-device','p1')");
    rejected(c, "INSERT INTO hold_devices VALUES ('h2',1,'ios-device','p1')");
    ok(c, "INSERT INTO hold_devices VALUES ('h1',2,'ios-sim',NULL); INSERT INTO hold_devices VALUES ('h2',2,'ios-sim',NULL)");
    ok(c, "INSERT INTO hold_accounts VALUES ('a','h1')");
    rejected(c, "INSERT INTO hold_accounts VALUES ('a','h2')");
    rejected(c, "INSERT INTO hold_devices VALUES ('nope',1,'ios-device','p9')");
    rejected(c, "INSERT INTO hold_accounts VALUES ('b','nope')");
}

// CASE-STORE-2
#[test]
fn bad_values_rejected() {
    let (_d, s) = fresh();
    let c = s.conn();
    rejected(c, &format!("{EXEC}) VALUES ('a',NULL,'weird','/','[]','k',1)"));
    rejected(c, "INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at) VALUES ('h','waiting','{}',1,1,1); INSERT INTO hold_devices VALUES ('h',1,'pad',NULL)");
    rejected(c, &format!("{EXEC}) VALUES ('a',NULL,'queued','/','[]','k','soon')"));
    rejected(c, &format!("{EXEC}) VALUES ('a',NULL,'queued','/','not json','k',1)"));
    rejected(c, &format!("{EXEC}, cancel_requested) VALUES ('a',NULL,'queued','/','[]','k',1,2)"));
    rejected(c, &format!("{EXEC}) VALUES ('a',NULL,'running','/','[]','k',1)"));
    rejected(c, &format!("{EXEC}, finished_at) VALUES ('a',NULL,'ok','/','[]','k',1,2)"));
    rejected(c, &format!("{EXEC}, finished_at, exit_code) VALUES ('a',NULL,'failed','/','[]','k',1,2,0)"));
    rejected(c, &format!("{EXEC}, finished_at, exit_code) VALUES ('a',NULL,'timeout','/','[]','k',1,2,1)"));
    rejected(c, &format!("{EXEC}) VALUES ('a',NULL,'cancelled','/','[]','k',1)"));
    rejected(c, "INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at) VALUES ('x','held','{}',1,1,1)");
    rejected(c, "INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at) VALUES ('y','released','{}',1,1,1)");
}

// CASE-STORE-3
#[test]
fn terminal_status_immutable() {
    let (_d, s) = fresh();
    let c = s.conn();
    ok(c, &format!("{EXEC}, finished_at) VALUES ('a',NULL,'cancelled','/','[]','k',1,2)"));
    rejected(c, "UPDATE executions SET status='queued' WHERE id='a'");
    ok(c, "INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at,ended_at) VALUES ('h','released','{}',1,1,1,2)");
    rejected(c, "UPDATE holds SET status='held', acquired_at=3 WHERE id='h'");
}

// CASE-FAULT-6
#[test]
fn kill_mid_write_leaves_nothing() {
    let (d, s) = fresh();
    drop(s);
    let p = d.path().join("aq.db");
    let script = format!(
        "import sqlite3,os,time\nc=sqlite3.connect('{}',isolation_level=None)\nc.execute('BEGIN IMMEDIATE')\nc.execute(\"INSERT INTO executions (id,status,cwd,argv,profile_key,submitted_at) VALUES ('half','queued','/','[]','k',1)\")\nprint('ready',flush=True)\ntime.sleep(30)\n",
        p.display()
    );
    let mut child = std::process::Command::new("python3").arg("-c").arg(script).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line).unwrap();
    assert_eq!(line.trim(), "ready");
    child.kill().unwrap();
    let _ = child.wait();
    let s = Store::open(&p).unwrap();
    let n: i64 = s.conn().query_row("SELECT count(*) FROM executions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

fn plan(c: &Conn, sql: &str) -> String {
    let mut q = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows: Vec<String> = q.query_map([], |r| r.get::<_, String>(3)).unwrap().map(Result::unwrap).collect();
    rows.join(" | ")
}

// CASE-STORE-4
#[test]
fn hot_queries_use_indexes() {
    let (_d, s) = fresh();
    let c = s.conn();
    ok(c, "BEGIN");
    for i in 0..10_000 {
        let st = if i % 1000 == 0 {
            "running"
        } else if i % 997 == 0 {
            "queued"
        } else {
            "ok"
        };
        let extra = match st {
            "running" => ", started_at, pgid, pgid_started_at, heavy) VALUES (?,?,?,'/','[]',?,?,1,1,1,1)",
            "ok" => ", started_at, finished_at, exit_code, peak_mem_mb) VALUES (?,?,?,'/','[]',?,?,1,2,0,10)",
            _ => ") VALUES (?,?,?,'/','[]',?,?)",
        };
        let ps = rusqlite_params(i, st);
        let refs: Vec<&dyn aq_store::ToSql> = ps.iter().map(|b| b.as_ref()).collect();
        c.execute(&format!("{EXEC}{extra}"), refs.as_slice()).unwrap();
    }
    ok(c, "COMMIT");
    for (sql, idx) in [
        // 复用查找：SQLite 不会把 status='running' 匹配到 one_inflight 的部分索引条件上，走 exec_queue 的 status 前缀，进行中的行很少
        ("SELECT id FROM executions WHERE hash = 'h5' AND status = 'running'", "USING INDEX"),
        ("SELECT id FROM executions WHERE status = 'queued' ORDER BY submitted_at", "exec_queue"),
        ("SELECT id FROM executions WHERE status = 'running' ORDER BY submitted_at", "exec_queue"),
        (
            "SELECT peak_mem_mb FROM executions WHERE profile_key = 'k3' AND finished_at IS NOT NULL ORDER BY finished_at DESC LIMIT 20",
            "exec_profile",
        ),
        ("SELECT id FROM holds WHERE status = 'waiting' ORDER BY requested_at", "hold_queue"),
    ] {
        let p = plan(c, sql);
        assert!(p.contains(idx) && !p.contains("SCAN executions") && !p.contains("SCAN holds"), "{sql} → {p}");
    }
    let t = std::time::Instant::now();
    for _ in 0..100 {
        aq_store::active_execs(c).unwrap();
    }
    assert!(t.elapsed().as_millis() < 100, "列出进行中的执行 < 1 毫秒/次：{:?}", t.elapsed() / 100);
}

fn rusqlite_params(i: i64, st: &str) -> [Box<dyn aq_store::ToSql>; 5] {
    [Box::new(format!("e{i:05}")), Box::new(format!("h{i}")), Box::new(st.to_string()), Box::new(format!("k{}", i % 66)), Box::new(i)]
}

// CASE-STORE-5
#[test]
fn retention_keeps_newest_terminal_only() {
    let (_d, s) = fresh();
    let c = s.conn();
    for i in 0..12 {
        ok(c, &format!("{EXEC}, started_at, finished_at, exit_code) VALUES ('e{i:02}',NULL,'ok','/','[]','k',{i},{i},{i},0)"));
        ok(c, &format!("INSERT INTO holds (id,status,request,owner_pid,owner_started_at,requested_at,acquired_at,ended_at) VALUES ('h{i:02}','released','{{}}',1,1,{i},{i},{i})"));
        ok(c, &format!("INSERT INTO hold_devices VALUES ('h{i:02}',1,'ios-sim','u{i}')"));
    }
    ok(c, &format!("{EXEC}) VALUES ('q',NULL,'queued','/','[]','k',0)"));
    let removed = aq_store::prune(c, 5).unwrap();
    assert_eq!(removed.len(), 7);
    let n: i64 = c.query_row("SELECT count(*) FROM executions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 6, "保留最近 5 条已结束 + queued 不删");
    let d: i64 = c.query_row("SELECT count(*) FROM hold_devices", [], |r| r.get(0)).unwrap();
    assert_eq!(d, 5, "设备行级联删除");
}

/// CASE-LOAD-2：导入一个月 761 次真实编译测试（时长来自真实记录，内存按时长合成），每类命令的估算与独立实现（scripts 外的 Python 手算，结果存在 db42-real-month.json）一致。
// CASE-LOAD-2
#[test]
fn real_month_estimates_match_hand_calc() {
    #[derive(serde::Deserialize)]
    struct Row {
        id: String,
        kind: String,
        started: i64,
        finished: i64,
        peak: i64,
        cpu: f64,
    }
    #[derive(serde::Deserialize)]
    struct Exp {
        mem_mb: i64,
        est_s: i64,
        heavy: bool,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        rows: Vec<Row>,
        expected: std::collections::BTreeMap<String, Exp>,
    }
    let d: Data =
        serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/db42-real-month.json")).unwrap())
            .unwrap();
    let (_dir, s) = fresh();
    let c = s.conn();
    ok(c, "BEGIN");
    for r in &d.rows {
        c.execute(
            "INSERT INTO executions (id, status, cwd, argv, profile_key, submitted_at, started_at, finished_at, exit_code, peak_mem_mb, cpu_avg_cores, disk_delta_mb, heavy, mem_mb, disk_mb, uses_net, timeout_s, content_changed) \
             VALUES (?1, 'ok', '/', '[]', ?2, ?3, ?3, ?4, 0, ?5, ?6, 0, 1, 1, 0, 0, 1, 0)",
            aq_store_params(r.id.clone(), r.kind.clone(), r.started, r.finished, r.peak, r.cpu).iter().map(|b| b.as_ref()).collect::<Vec<_>>().as_slice(),
        )
        .unwrap();
    }
    ok(c, "COMMIT");
    assert_eq!(d.expected.len(), 4);
    for (kind, e) in &d.expected {
        let hist = aq_store::history(c, kind).unwrap();
        let est = aq_core::estimate::from_history(&hist, None, &Default::default()).unwrap();
        assert_eq!((est.res.mem_mb, est.est_s, est.res.heavy), (e.mem_mb, e.est_s, e.heavy), "{kind}");
    }
}

fn aq_store_params(id: String, kind: String, start: i64, fin: i64, peak: i64, cpu: f64) -> Vec<Box<dyn aq_store::ToSql>> {
    vec![Box::new(id), Box::new(kind), Box::new(start), Box::new(fin), Box::new(peak), Box::new(cpu)]
}
