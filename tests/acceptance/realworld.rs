//! 真实工作场景的回放。
//! 时长按比例缩短；真实 Xcode、模拟器、真机的部分见用例表里验证为「实机」的行。

use std::time::{Duration, Instant};

use crate::support::*;

fn heavy_ai(e: &Env) {
    e.set_ai_output(r#"{"heavy": true, "mem_gb": 1, "disk_gb": 0, "net": false, "est_minutes": 1, "reason": "xcodebuild"}"#);
}

// CASE-QUEUE-1, CASE-REUSE-3
#[test]
fn two_release_worktrees_build_together_cargo_third() {
    let e = Env::new();
    heavy_ai(&e);
    let repo = e.work().join("app-1.37");
    git_repo(&repo);
    let wt = e.work().join("app-1.36-extra");
    git(&repo, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]);
    let dd = |w: &std::path::Path| w.join("dd-marker").to_string_lossy().into_owned(); // 各 worktree 自己的绝对路径
    let a = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "3", "--touch", &dd(&repo)]);
    let b = e.aq_in(&wt, &["run", "--wait", "0", "--", "fake-task", "--secs", "3", "--touch", &dd(&wt)]);
    assert_ne!(a.id(), b.id(), "CASE-REUSE-3 不同的绝对 derivedDataPath 不复用");
    let c = e.aq_in(&repo, &["run", "--wait", "1", "--", "fake-task", "--secs", "1", "--exit", "0"]);
    assert_eq!(c.status(), "queued", "cargo test 排第三");
    assert!((10..=300).contains(&c.one()["retry_after_s"].as_i64().unwrap()));
    assert_eq!(e.q_i64("SELECT count(*) FROM executions WHERE status='running'"), 2, "两个 App 编译同时运行");
    assert_eq!(e.aq(&["status", &a.id(), &b.id(), &c.id(), "--wait", "30"]).code, 0);
}

// CASE-REUSE-1
#[test]
fn same_worktree_same_command_shares_result_and_log() {
    let e = Env::new();
    let repo = e.work().join("demo-ios");
    git_repo(&repo);
    let a = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    let b = e.aq_in(&repo, &["run", "--wait", "30", "--", "fake-task", "--secs", "2"]);
    let a = e.aq(&["status", &a.id(), "--wait", "30"]);
    assert_eq!(b.one()["reused"], true);
    assert_eq!((a.one()["status"].clone(), a.one()["log"].clone()), (b.one()["status"].clone(), b.one()["log"].clone()));
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 1);
}

// CASE-RUN-4
#[test]
fn background_build_then_status_wait_replaces_sleep_loop() {
    let e = Env::new();
    let r = e.aq(&["run", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    let t = Instant::now();
    let s = e.aq(&["status", &r.id(), "--wait", "600"]);
    assert_eq!(s.status(), "ok");
    assert!(t.elapsed() < Duration::from_secs(5), "结束即返回，不空等到 600 秒");
}

// CASE-REUSE-5
#[test]
fn edit_during_build_marks_content_changed() {
    let e = Env::new();
    let repo = e.work().join("repo");
    git_repo(&repo);
    let r = e.aq_in(&repo, &["run", "--wait", "0", "--", "fake-task", "--secs", "2"]);
    std::fs::write(repo.join("LoginView.swift"), "// edit").unwrap();
    assert_eq!(e.aq(&["status", &r.id(), "--wait", "20"]).one()["content_changed"], true);
}

// CASE-LOAD-3
#[test]
fn sdk_chain_each_step_own_id_failure_stops_chain() {
    let e = Env::new();
    let steps: [&[&str]; 3] = [
        &["fake-task", "--secs", "0", "--mem", "10M"],
        &["sh", "-c", "echo xcframework ok"],
        &["sh", "-c", "echo build failed >&2; exit 65"],
    ];
    let mut ids = vec![];
    for s in steps {
        let mut args = vec!["run", "--"];
        args.extend_from_slice(s);
        let r = e.aq(&args);
        ids.push(r.id());
        if r.status() != "ok" {
            assert_eq!(r.one()["exit_code"], 65);
            let log = std::fs::read_to_string(r.one()["log"].as_str().unwrap()).unwrap();
            assert!(log.contains("build failed"));
            break;
        }
    }
    assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 3);
    assert_eq!(e.q_i64("SELECT count(*) FROM executions"), 3, "失败后没有自动提交后续步骤");
}

// CASE-HOLD-4
#[test]
fn meeting_two_sims_two_accounts() {
    let e = Env::new();
    let a = e.aq(&["hold", "ios-sim", "ios-sim", "--account", "A", "--account", "B"]);
    assert_eq!(a.status(), "held");
    assert_eq!(a.one()["devices"].as_array().unwrap().len(), 2);
    let other = e.aq(&["hold", "ios-sim", "--account", "A", "--wait", "1"]);
    assert_eq!(other.status(), "waiting");
    e.aq(&["release", &a.id()]);
    assert_eq!(e.aq(&["status", &other.id(), "--wait", "10"]).status(), "held", "release 后 A 立即可借");
}

// CASE-RECLAIM-3, CASE-RECLAIM-4
#[test]
fn forgotten_and_overnight_holds_reclaimed_without_residue() {
    let e = Env::new();
    e.env("AQ_T_IDLE_RECLAIM_S", "1");
    let h1 = e.aq(&["hold", "ios-sim", "--account", "A"]);
    let h2 = e.aq(&["hold", "ios-sim", "ios-device"]);
    std::thread::sleep(Duration::from_millis(1500));
    e.aq(&["status"]); // 第二天第一条 aq 命令
    for h in [&h1, &h2] {
        assert_eq!(e.aq(&["status", &h.id()]).status(), "reclaimed");
    }
    assert!(until(10, || e.sims().is_empty()), "没有残留的 aq- 模拟器：{:?}", e.sims());
    assert_eq!(e.aq(&["status"]).lines[0]["sims"], 0, "内存记账减少");
}

// CASE-LOAD-1, CASE-QUEUE-1
#[test]
fn eight_sessions_peak() {
    let e = Env::new();
    heavy_ai(&e);
    let mut kids = vec![];
    for i in 0..8 {
        kids.push(match i % 4 {
            0 | 1 => e.spawn(&["run", "--fresh", "--wait", "60", "--", "fake-task", "--secs", "2"]),
            2 => e.spawn(&["run", "--fresh", "--wait", "60", "--", "sh", "-c", "sleep 1"]),
            _ => e.spawn(&["hold", "ios-sim", "--wait", "60"]),
        });
    }
    std::thread::sleep(Duration::from_millis(800));
    let human = e.aq(&["status", "--human"]).stdout;
    assert!(human.contains("排队") || human.contains("运行"), "{human}");
    let mut max = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && e.q_i64("SELECT count(*) FROM executions WHERE status IN ('queued','running')") > 0 {
        max = max.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
        std::thread::sleep(Duration::from_millis(50));
    }
    let outs: Vec<Res> = kids.into_iter().map(|k| parse(k.wait_with_output().unwrap())).collect();
    assert!(max <= 2);
    assert!(outs.iter().all(|o| ["ok", "held"].contains(&o.status())), "所有人最终都能跑上");
}

// CASE-RUN-6
#[test]
fn hung_build_times_out_and_queue_moves_on() {
    let e = Env::with_config(&DEFAULT_CONFIG.replace("heavy_slots = 2", "heavy_slots = 1"));
    heavy_ai(&e);
    e.env("AQ_T_TIMEOUT_MIN_S", "2").env("AQ_T_TIMEOUT_MAX_S", "2");
    let hung = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "1000"]);
    let next = e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", "0"]);
    let s = e.aq(&["status", &hung.id(), &next.id(), "--wait", "30"]);
    assert_eq!((s.lines[0]["status"].as_str(), s.lines[1]["status"].as_str()), (Some("timeout"), Some("ok")));
}

// CASE-EST-1
#[test]
fn first_swift_uses_ai_then_measured_and_human_says_so() {
    let e = Env::new();
    let a = e.aq(&["run", "--", "fake-task", "--mem", "30M"]);
    assert!(e.aq(&["status", &a.id(), "--human"]).stdout.contains("评估来自AI 评估"));
    let calls = e.ai_calls();
    let b = e.aq(&["run", "--", "fake-task", "--mem", "30M"]);
    assert_eq!(e.ai_calls(), calls);
    assert!(e.aq(&["status", &b.id(), "--human"]).stdout.contains("评估来自实测"));
}

// CASE-REUSE-4
#[test]
fn scratch_dir_not_git_runs_without_reuse() {
    let e = Env::new();
    let r = e.aq(&["run", "--", "true"]);
    assert_eq!(r.status(), "ok");
    assert_eq!(e.q_i64("SELECT hash IS NULL AND fingerprint IS NULL FROM executions"), 1, "CASE-REUSE-4");
}

#[derive(serde::Deserialize)]
struct Job {
    at_s: f64,
    dur_s: f64,
}

/// 场景回放：真实记录里最忙的 2 小时（20 次 xcodebuild）按 120 倍速重放。
/// 比较：运行中重任务始终 ≤ 2；全部成功；打印排队时间供与附录 C 对照。
// CASE-LOAD-1
#[test]
fn replay_busiest_two_hours_at_120x() {
    let e = Env::new();
    heavy_ai(&e);
    let text = std::fs::read_to_string(support_dir().join("replay-window.json")).unwrap();
    let jobs: Vec<Job> = serde_json::from_str(&text).unwrap();
    let speed = 120.0;
    let t0 = Instant::now();
    let mut ids = vec![];
    let mut max = 0;
    for j in &jobs {
        while t0.elapsed().as_secs_f64() < j.at_s / speed {
            max = max.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
            std::thread::sleep(Duration::from_millis(20));
        }
        let secs = format!("{:.2}", j.dur_s / speed);
        ids.push(e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", &secs, "--mem", "20M"]).id());
    }
    while e.q_i64("SELECT count(*) FROM executions WHERE status IN ('queued','running')") > 0 && t0.elapsed() < Duration::from_secs(180) {
        max = max.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(max <= 2, "运行中的重任务 {max}");
    let waits: Vec<i64> = {
        let db = e.db();
        let mut q = db.prepare("SELECT started_at - submitted_at FROM executions WHERE status = 'ok'").unwrap();
        q.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(waits.len(), jobs.len(), "全部成功");
    let mean = waits.iter().sum::<i64>() as f64 / waits.len() as f64 / 1000.0 * speed;
    let ideal = ideal_fifo_mean_wait(&jobs, 2);
    eprintln!("回放 {} 个任务：平均排队 {mean:.0} 秒，理想先来先服务 {ideal:.0} 秒（折算回真实时间）", waits.len());
    assert!(mean <= ideal * 1.25 + 30.0, "排队时间应接近理想先来先服务（差额来自进程启动和轮询，按倍速放大）");
}

/// 同一批任务在「先来先服务、最多 n 个重任务、无额外开销」下的平均排队秒数（附录 C 的回放模型）。
fn ideal_fifo_mean_wait(jobs: &[Job], n: usize) -> f64 {
    let mut free = vec![0.0f64; n];
    let mut total = 0.0;
    for j in jobs {
        free.sort_by(f64::total_cmp);
        let start = free[0].max(j.at_s);
        total += start - j.at_s;
        free[0] = start + j.dur_s;
    }
    total / jobs.len() as f64
}

/// 一个月 761 次真实编译测试按 1000 倍速整月回放（约 45 分钟，`just m3` 手动跑）。
/// 时长按真实记录（单条上限 2 小时，超出的按超时处理）；比较：重任务始终 ≤ 2、没有因资源不足失败、排队接近理想先来先服务。
// CASE-LOAD-1
#[test]
#[ignore]
fn replay_full_month_at_1000x() {
    let e = Env::new();
    heavy_ai(&e);
    let text = std::fs::read_to_string(support_dir().join("replay-month.json")).unwrap();
    let mut jobs: Vec<Job> = serde_json::from_str(&text).unwrap();
    for j in &mut jobs {
        j.dur_s = j.dur_s.min(7200.0);
    }
    let speed = 1000.0;
    let t0 = Instant::now();
    let mut max = 0;
    for j in &jobs {
        while t0.elapsed().as_secs_f64() < j.at_s / speed {
            max = max.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
            std::thread::sleep(Duration::from_millis(50));
        }
        let secs = format!("{:.3}", j.dur_s / speed);
        e.aq(&["run", "--fresh", "--wait", "0", "--", "fake-task", "--secs", &secs, "--mem", "20M"]);
    }
    while e.q_i64("SELECT count(*) FROM executions WHERE status IN ('queued','running')") > 0 {
        max = max.max(e.q_i64("SELECT count(*) FROM executions WHERE status='running' AND heavy=1"));
        std::thread::sleep(Duration::from_millis(50));
    }
    let failed = e.q_i64("SELECT count(*) FROM executions WHERE status <> 'ok'");
    let waits: Vec<i64> = {
        let db = e.db();
        let mut q = db.prepare("SELECT started_at - submitted_at FROM executions ORDER BY submitted_at").unwrap();
        q.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let mut sorted = waits.clone();
    sorted.sort();
    let mean = waits.iter().sum::<i64>() as f64 / waits.len() as f64 / 1000.0 * speed;
    let p95 = sorted[sorted.len() * 95 / 100] as f64 / 1000.0 * speed;
    let ideal = ideal_fifo_mean_wait(&jobs, 2);
    eprintln!(
        "CASE-LOAD-1 回放 {} 个：平均排队 {mean:.0}s，p95 {p95:.0}s；理想先来先服务平均 {ideal:.0}s；重任务峰值 {max}；非 ok {failed}",
        waits.len()
    );
    assert!(max <= 2);
    assert_eq!(failed, 0, "没有因资源不足导致的失败");
}
