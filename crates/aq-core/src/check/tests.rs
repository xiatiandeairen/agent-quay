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
        sys: SysSample { mem_total_mb: 36_000, mem_avail_mb: 30_000, disk_total_mb: 1_000_000, disk_free_mb: 500_000, cpu_pct: 1 },
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

// CASE-RECLAIM-3, CASE-RECLAIM-4
#[test]
fn reclaim_owner_exit_and_idle() {
    let s = Snapshot {
        now_ms: 10_000_000,
        holds: vec![hold("gone", HoldStatus::Held, vec![]), hold("idle", HoldStatus::Held, vec![]), hold("busy", HoldStatus::Held, vec![])],
        ..Snapshot::default()
    };
    let mut p = probes();
    p.owner_alive.insert("gone".into(), false);
    p.last_used_at.insert("busy".into(), 9_999_000);
    let lim = Limits { idle_reclaim_ms: 1_000_000, ..Limits::default() };
    let pl = plan(&s, &p, &Capacity::default(), &lim);
    assert_eq!(
        pl.actions,
        vec![Action::Reclaim("gone".into(), ReclaimReason::OwnerExit), Action::Reclaim("idle".into(), ReclaimReason::Idle)]
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
