use super::*;

fn s(mem_gb: i64, dur: i64) -> Sample {
    Sample { peak_mem_mb: mem_gb * 1024, disk_delta_mb: 0, duration_s: dur, cpu_avg_cores: 3.0, usable_for_duration: true }
}

// CASE-EST-5
#[test]
fn single_sample_times_1_2() {
    let e = from_history(&[s(5, 100)], None, &Limits::default()).unwrap();
    assert_eq!(e.res.mem_mb, (5.0f64 * 1024.0 * 1.2).ceil() as i64);
    assert_eq!(e.source, Source::Measured);
}

// CASE-EST-5
#[test]
fn weighted_p90_times_1_1() {
    // 时间顺序 4、4、5、6、9G，最新的是 9G。newest_first = 9,6,5,4,4
    let samples: Vec<Sample> = [9, 6, 5, 4, 4].iter().map(|g| s(*g, 100)).collect();
    let e = from_history(&samples, None, &Limits::default()).unwrap();
    // 权重 1, .8, .64, .512, .4096；总和 3.3616；p90 目标 3.025；
    // 升序累计：4G .9216 → 5G 1.5616 → 6G 2.3616 → 9G 3.3616 ≥ 3.025 → 9G
    assert_eq!(e.res.mem_mb, (9.0f64 * 1024.0 * 1.1).ceil() as i64);
}

// CASE-EST-5
#[test]
fn recent_growth_dominates() {
    let mut v: Vec<Sample> = (0..3).map(|_| s(8, 100)).collect();
    v.extend((0..10).map(|_| s(4, 100)));
    let e = from_history(&v, None, &Limits::default()).unwrap();
    assert!(e.res.mem_mb as f64 >= 8.8 * 1024.0 - 1.0);
}

// CASE-EST-5
#[test]
fn fast_failure_excluded_from_duration_not_memory() {
    let mut v = vec![Sample { duration_s: 2, peak_mem_mb: 12 * 1024, ..s(1, 2) }];
    v.extend((0..4).map(|_| s(4, 300)));
    let e = from_history(&v, None, &Limits::default()).unwrap();
    assert_eq!(e.est_s, 300);
    assert!(e.res.mem_mb >= 12 * 1024);
}

// CASE-EST-5
#[test]
fn interrupted_not_counted_for_duration() {
    let v = vec![Sample { usable_for_duration: false, duration_s: 5000, ..s(4, 5000) }, s(4, 100)];
    assert_eq!(from_history(&v, None, &Limits::default()).unwrap().est_s, 100);
}

// CASE-EST-11
#[test]
fn heavy_by_weighted_majority_of_cpu() {
    let light = Sample { cpu_avg_cores: 1.7, ..s(1, 45) };
    let cold = Sample { cpu_avg_cores: 3.5, ..s(3, 415) };
    let lim = Limits::default();
    // 真实 App：最近两次增量（1.7、1.8 核）+ 更早一次全量（3.5 核）→ 轻，增量编译不去排重任务名额
    assert!(!from_history(&[light, Sample { cpu_avg_cores: 1.8, ..light }, cold], None, &lim).unwrap().res.heavy);
    // 最新一次重、之前 3 次轻：1 / (1+0.8+0.64+0.512) 不过半 → 仍轻（CASE-EST-11 第一段）
    assert!(!from_history(&[cold, light, light, light], None, &lim).unwrap().res.heavy);
    // 最近两次都重：1.8 > 1.152 → 重（CASE-EST-11 第二段）
    assert!(from_history(&[cold, cold, light, light, light], None, &lim).unwrap().res.heavy);
    // 一直都是轻的 → 轻
    assert!(!from_history(&[light; 5], None, &lim).unwrap().res.heavy);
}

// CASE-EST-4
#[test]
fn default_without_history() {
    let cap = Capacity::default();
    let e = conservative(&MachineHistory::default(), &cap, &Limits::default());
    assert_eq!(e.res.mem_mb, cap.mem_mb / 4);
    assert!(e.res.heavy && e.res.uses_net);
    assert_eq!(e.timeout_s, 7200);
}

// CASE-EST-4
#[test]
fn default_with_history_uses_p90() {
    let h = MachineHistory { peaks_mb: (1..=10).map(|g| g * 1024).collect(), disk_deltas_mb: vec![10], durations_s: vec![100; 10] };
    let e = conservative(&h, &Capacity::default(), &Limits::default());
    assert_eq!(e.res.mem_mb, 9 * 1024);
    assert_eq!(e.timeout_s, 1800, "全机时长 p90 很短时超时也不低于 30 分钟");
}

// CASE-EST-4
#[test]
fn timeout_bounds() {
    let l = Limits::default();
    assert_eq!(timeout_for(10, &l), 1800);
    assert_eq!(timeout_for(1000, &l), 3000);
    assert_eq!(timeout_for(10_000, &l), 7200);
}

// CASE-EST-3
#[test]
fn ai_output_validation() {
    let c = Capacity::default();
    let l = Limits::default();
    let ok = r#"{"heavy": true, "mem_gb": 10, "disk_gb": 4, "net": false, "est_minutes": 5, "reason": "x"}"#;
    let e = parse_ai(ok, &c, &l).unwrap();
    assert_eq!((e.res.mem_mb, e.est_s, e.source), (10 * 1024, 300, Source::Ai));
    assert!(parse_ai(&format!("```json\n{ok}\n```"), &c, &l).is_some());
    assert!(parse_ai("garbage", &c, &l).is_none());
    assert!(parse_ai(&ok.replace("10,", "100,"), &c, &l).is_none());
    assert!(parse_ai(&ok.replace("\"est_minutes\": 5", "\"est_minutes\": 500"), &c, &l).is_none());
    assert!(parse_ai(&ok.replace("true", "\"yes\""), &c, &l).is_none());
    assert!(parse_ai(&format!("先分析一下：大概 {{10GB}}。\n{ok}"), &c, &l).is_some(), "JSON 前有说明文字可以");
    assert!(parse_ai(&format!("{ok}\n以上是估计"), &c, &l).is_none(), "JSON 后还有文字不行");
    assert!(parse_ai(&ok.replace("\"reason\"", "\"extra\": 1, \"reason\""), &c, &l).is_none(), "多余字段不行");
    assert!(parse_ai(&ok.replace("10,", "-1,"), &c, &l).is_none(), "负数不合格");
    let tiny = parse_ai(&ok.replace("\"mem_gb\": 10", "\"mem_gb\": 0.01").replace("\"est_minutes\": 5", "\"est_minutes\": 0.001"), &c, &l)
        .unwrap();
    assert_eq!((tiny.res.mem_mb, tiny.est_s), (256, 1), "低于下限抬到下限");
}
