use super::*;
use crate::model::{DeviceType, HoldRequest, ProcId, Resources};

fn ex(id: &str, status: ExecStatus, at: i64) -> ExecView {
    ExecView {
        id: id.into(),
        status,
        submitted_at: at,
        started_at: None,
        res: Some(Resources { heavy: true, mem_mb: 1000, disk_mb: 0, uses_net: false }),
        timeout_s: Some(1800),
        pgid: None,
        argv: vec![],
    }
}
fn hold(id: &str, status: HoldStatus, devices: Vec<DeviceType>) -> HoldView {
    HoldView {
        id: id.into(),
        status,
        requested_at: 1,
        acquired_at: Some(1),
        request: HoldRequest { devices, accounts: vec!["a".into()], exclude: vec![] },
        owner: ProcId { pid: 1, started_at: 1 },
    }
}
fn probes() -> Probes {
    Probes {
        sys: SysSample {
            mem_total_mb: 36_000,
            mem_avail_mb: 30_000,
            disk_total_mb: 1_000_000,
            disk_free_mb: 500_000,
            ..SysSample::default()
        },
        ..Probes::default()
    }
}

// CASE-FAULT-1
#[test]
fn runner_gone_command_alive_keeps_running() {
    let s = Snapshot { execs: vec![ex("e", ExecStatus::Running, 1)], ..Snapshot::default() };
    let mut p = probes();
    p.runner_gone.insert("e".into(), true);
    p.group_alive.insert("e".into(), true);
    assert!(plan(&s, &p, &Capacity::default(), &Limits::default()).actions.is_empty());
    p.group_alive.insert("e".into(), false);
    let pl = plan(&s, &p, &Capacity::default(), &Limits::default());
    assert_eq!(pl.actions, vec![Action::Interrupt("e".into())]);
}

// CASE-FAULT-1
#[test]
fn runner_alive_never_interrupted() {
    let s = Snapshot { execs: vec![ex("e", ExecStatus::Running, 1)], ..Snapshot::default() };
    let mut p = probes();
    p.group_alive.insert("e".into(), false);
    assert!(plan(&s, &p, &Capacity::default(), &Limits::default()).actions.is_empty());
}

// CASE-FAULT-3
#[test]
fn queued_runner_gone_respawns_and_keeps_position() {
    let s = Snapshot { execs: vec![ex("a", ExecStatus::Queued, 1), ex("b", ExecStatus::Queued, 2)], ..Snapshot::default() };
    let mut p = probes();
    p.runner_gone.insert("a".into(), true);
    let pl = plan(&s, &p, &Capacity::default(), &Limits::default());
    assert_eq!(pl.actions, vec![Action::RespawnExec("a".into())]);
    assert_eq!(pl.admit.admitted[0].id, "a");
}

const MIN: i64 = 60_000;

/// 一组 held 的借用：一台设备，acquired_at = 0。
fn held_with(id: &str, dtype: DeviceType) -> (HoldView, DeviceRow) {
    let mut h = hold(id, HoldStatus::Held, vec![dtype]);
    h.acquired_at = Some(0);
    (h, DeviceRow { hold_id: id.into(), seq: 0, dtype, device: Some(format!("DEV-{id}")) })
}

fn one(now_ms: i64, h: (HoldView, DeviceRow)) -> Snapshot {
    Snapshot { now_ms, holds: vec![h.0], devices: vec![h.1], ..Snapshot::default() }
}

fn actions(s: &Snapshot, p: &Probes) -> Vec<Action> {
    plan(s, p, &Capacity::default(), &Limits::default()).actions
}

fn in_use(id: &str) -> Probes {
    let mut p = probes();
    p.in_use.insert(id.into());
    p
}

// CASE-RECLAIM-3
#[test]
fn u1_owner_exit_reclaims_even_in_use() {
    let mut p = in_use("h");
    p.owner_alive.insert("h".into(), false);
    assert_eq!(actions(&one(MIN, held_with("h", DeviceType::IosSim)), &p), [Action::Reclaim("h".into(), ReclaimReason::OwnerExit)]);
}

// CASE-RECLAIM-4
#[test]
fn u2_never_used_idle_at_threshold() {
    let h = || held_with("h", DeviceType::IosSim);
    assert!(actions(&one(29 * MIN, h()), &probes()).is_empty(), "29 分不收");
    assert_eq!(actions(&one(30 * MIN, h()), &probes()), [Action::Reclaim("h".into(), ReclaimReason::Idle)], "30 分收（闲置）");
}

// CASE-RECLAIM-4
#[test]
fn u3_idle_counts_from_last_use() {
    let mut p = probes();
    p.last_used_at.insert("h".into(), 50 * MIN);
    assert!(actions(&one(60 * MIN, held_with("h", DeviceType::IosSim)), &p).is_empty(), "10 分钟前有人用");
    assert_eq!(actions(&one(81 * MIN, held_with("h", DeviceType::IosSim)), &p), [Action::Reclaim("h".into(), ReclaimReason::Idle)]);
}

// CASE-RECLAIM-4
#[test]
fn in_use_marks_used_and_is_not_idle() {
    let mut p = in_use("h");
    p.last_used_at.insert("h".into(), 0);
    assert_eq!(actions(&one(90 * MIN, held_with("h", DeviceType::AndroidEmu)), &p), [Action::MarkUsed("h".into())]);
}

// CASE-RECLAIM-9
#[test]
fn u4_max_hold_reclaims_in_use() {
    assert_eq!(
        actions(&one(240 * MIN, held_with("h", DeviceType::IosSim)), &in_use("h")),
        [Action::Reclaim("h".into(), ReclaimReason::MaxHold)]
    );
}

// CASE-RECLAIM-9
#[test]
fn u5_just_under_max_hold_kept() {
    assert_eq!(actions(&one(239 * MIN, held_with("h", DeviceType::IosSim)), &in_use("h")), [Action::MarkUsed("h".into())]);
}

// CASE-RECLAIM-5
#[test]
fn u6_unknown_owner_skips_owner_layer_only() {
    let unknown = || {
        let (mut h, d) = held_with("h", DeviceType::IosSim);
        h.owner = ProcId::UNKNOWN;
        (h, d)
    };
    // 未识别的借用者不做退出核对：app 层不写 owner_alive
    assert!(actions(&one(MIN, unknown()), &probes()).is_empty());
    assert_eq!(actions(&one(30 * MIN, unknown()), &probes()), [Action::Reclaim("h".into(), ReclaimReason::Idle)]);
    assert_eq!(actions(&one(240 * MIN, unknown()), &in_use("h")), [Action::Reclaim("h".into(), ReclaimReason::MaxHold)]);
}

// CASE-RECLAIM-9
#[test]
fn u7_owner_exit_and_max_hold_single_action() {
    let mut p = in_use("h");
    p.owner_alive.insert("h".into(), false);
    assert_eq!(actions(&one(300 * MIN, held_with("h", DeviceType::IosSim)), &p), [Action::Reclaim("h".into(), ReclaimReason::OwnerExit)]);
}

// CASE-RECLAIM-10
#[test]
fn run_args_naming_device_mark_used() {
    for st in [ExecStatus::Queued, ExecStatus::Running] {
        let mut s = one(60 * MIN, held_with("h", DeviceType::IosSim));
        let mut e = ex("e", st, 1);
        e.argv = vec!["xcodebuild".into(), "-destination".into(), "platform=iOS Simulator,id=DEV-h".into()];
        s.execs = vec![e];
        assert_eq!(actions(&s, &probes()), [Action::MarkUsed("h".into())], "{st:?}");
    }
    let mut s = one(60 * MIN, held_with("h", DeviceType::IosSim));
    s.submitting = vec!["xcodebuild".into(), "id=DEV-h".into()];
    assert_eq!(actions(&s, &probes()), [Action::MarkUsed("h".into())], "正要提交的也算");
    s.submitting = vec!["xcodebuild".into(), "id=DEV-hx".into()];
    assert_eq!(actions(&s, &probes()), [Action::Reclaim("h".into(), ReclaimReason::Idle)], "不整段匹配不算");
}

// CASE-RECLAIM-4, CASE-RECLAIM-9
#[test]
fn browser_only_hold_not_idle_but_max_hold() {
    assert!(actions(&one(200 * MIN, held_with("h", DeviceType::Browser)), &probes()).is_empty());
    assert!(actions(&one(200 * MIN, held_with("h", DeviceType::Desktop)), &probes()).is_empty());
    assert_eq!(
        actions(&one(240 * MIN, held_with("h", DeviceType::Browser)), &probes()),
        [Action::Reclaim("h".into(), ReclaimReason::MaxHold)]
    );
}

// CASE-RECLAIM-3
#[test]
fn reclaimed_account_and_phone_free_immediately() {
    let cap = Capacity { ios_devices: vec!["p1".into()], ..Capacity::default() };
    let mut old = hold("old", HoldStatus::Held, vec![DeviceType::IosDevice]);
    old.owner = ProcId { pid: 9, started_at: 9 };
    let mut new = hold("new", HoldStatus::Waiting, vec![DeviceType::IosDevice]);
    new.requested_at = 5;
    let s = Snapshot {
        holds: vec![old, new],
        devices: vec![DeviceRow { hold_id: "old".into(), seq: 0, dtype: DeviceType::IosDevice, device: Some("p1".into()) }],
        accounts: vec![AccountRow { account: "a".into(), hold_id: "old".into() }],
        ..Snapshot::default()
    };
    let mut p = probes();
    p.owner_alive.insert("old".into(), false);
    let pl = plan(&s, &p, &cap, &Limits::default());
    assert_eq!(pl.admit.admitted.len(), 1);
    assert_eq!(pl.admit.admitted[0].id, "new");
}

#[test]
fn unevaluated_queued_exec_not_admitted() {
    let mut e = ex("e", ExecStatus::Queued, 1);
    e.res = None;
    let s = Snapshot { execs: vec![e], ..Snapshot::default() };
    assert!(plan(&s, &probes(), &Capacity::default(), &Limits::default()).admit.admitted.is_empty());
}

// CASE-HOLD-8
#[test]
fn allocated_waiting_hold_counts_sim_memory_not_requeued() {
    let cap = Capacity { mem_mb: 4000, ..Capacity::default() };
    let creating = hold("c", HoldStatus::Waiting, vec![DeviceType::IosSim]);
    let mut next = hold("n", HoldStatus::Waiting, vec![DeviceType::IosSim]);
    next.request.accounts = vec![];
    next.requested_at = 2;
    let s = Snapshot {
        holds: vec![creating, next],
        devices: vec![DeviceRow { hold_id: "c".into(), seq: 0, dtype: DeviceType::IosSim, device: None }],
        ..Snapshot::default()
    };
    let pl = plan(&s, &probes(), &cap, &Limits::default());
    assert!(pl.admit.admitted.is_empty());
}
