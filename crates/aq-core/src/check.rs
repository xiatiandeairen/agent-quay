//! 所有命令共用的「检查」：plan(快照, 探测结果) → 动作。
//! 同样的输入永远得到同样的输出；IO 由 app 层按动作执行。

use std::collections::{HashMap, HashSet};

use crate::admit::{admit, AdmitResult, Demand, Item, Ledger, SysSample};
use crate::capacity::{Capacity, Limits};
use crate::model::{AccountRow, DeviceRow, ExecStatus, ExecView, HoldStatus, HoldView, ReclaimReason};

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub now_ms: i64,
    pub execs: Vec<ExecView>,
    pub holds: Vec<HoldView>,
    pub devices: Vec<DeviceRow>,
    pub accounts: Vec<AccountRow>,
    /// 这次检查同一事务里正要提交的 `aq run` 命令（没有时为空）：参数带着借到的设备 id 时提交那一刻就算在用（CASE-RECLAIM-10）。
    pub submitting: Vec<String>,
}

/// 探测结果（app 层在事务里采集）。缺省视为「还活着/还在用」，宁可不收回也不误收回。
#[derive(Debug, Clone, Default)]
pub struct Probes {
    /// runner 的锁拿得到 = runner 已不在。key 是执行或借用 id。
    pub runner_gone: HashMap<String, bool>,
    /// 执行的进程组是否还有进程（只对 runner 已不在的 running 执行有意义）。
    pub group_alive: HashMap<String, bool>,
    /// 运行中执行的进程组当前内存（MB）。
    pub group_mem_mb: HashMap<String, i64>,
    /// 借用者进程是否还在（进程号 + 启动时间都对得上）。
    pub owner_alive: HashMap<String, bool>,
    /// holds/<id>.used 的修改时间；没有文件时用 acquired_at。
    pub last_used_at: HashMap<String, i64>,
    /// 此刻探测到有 aq 之外的进程在用的借用 id（usage 模块判定，design.md 5.5）。
    pub in_use: HashSet<String>,
    pub sys: SysSample,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// runner 不在、命令进程也没了 → interrupted。
    Interrupt(String),
    /// 排队中的执行 runner 消失 → 重新拉起，保持排队位置。
    RespawnExec(String),
    /// 等待中的借用 runner 消失 → 先删半成品模拟器和已分配的行，再重新拉起。
    RespawnHold(String),
    Reclaim(String, ReclaimReason),
    /// 借用此刻在用：把 holds/<id>.used 改成现在（CASE-RECLAIM-4）。
    MarkUsed(String),
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    pub admit: AdmitResult,
}

fn gone(p: &Probes, id: &str) -> bool {
    p.runner_gone.get(id).copied().unwrap_or(false)
}

fn recover_execs(s: &Snapshot, p: &Probes, out: &mut Vec<Action>) {
    for e in &s.execs {
        if !gone(p, &e.id) {
            continue;
        }
        match e.status {
            ExecStatus::Running if !p.group_alive.get(&e.id).copied().unwrap_or(true) => {
                out.push(Action::Interrupt(e.id.clone()));
            }
            ExecStatus::Queued => out.push(Action::RespawnExec(e.id.clone())),
            _ => {}
        }
    }
}

/// 这组借用的设备 id 出现在排队、运行中或正要提交的 aq run 参数里（CASE-RECLAIM-10）。
fn named_by_run(s: &Snapshot, ids: &[&str]) -> bool {
    let live = s.execs.iter().filter(|e| !e.status.is_terminal()).map(|e| e.argv.as_slice());
    std::iter::once(s.submitting.as_slice()).chain(live).any(|argv| ids.iter().any(|id| crate::usage::argv_mentions(argv, id)))
}

/// 收回原因，按「借用者退出 → 超过借用上限 → 闲置」取第一个（CASE-RECLAIM-9）。
fn reclaim_reason(h: &HoldView, s: &Snapshot, p: &Probes, lim: &Limits, used: bool) -> Option<ReclaimReason> {
    if !p.owner_alive.get(&h.id).copied().unwrap_or(true) {
        return Some(ReclaimReason::OwnerExit);
    }
    let acquired = h.acquired_at.unwrap_or(s.now_ms);
    if s.now_ms - acquired >= lim.max_hold_ms {
        return Some(ReclaimReason::MaxHold);
    }
    // 只有浏览器、桌面会话的借用探测不到使用，不按闲置收回（design.md 5.5）
    let probed = s.devices.iter().any(|d| d.hold_id == h.id && d.dtype.usage_probed());
    let last = p.last_used_at.get(&h.id).copied().unwrap_or(acquired);
    (probed && !used && s.now_ms - last >= lim.idle_reclaim_ms).then_some(ReclaimReason::Idle)
}

fn reclaim_holds(s: &Snapshot, p: &Probes, lim: &Limits, out: &mut Vec<Action>) {
    for h in &s.holds {
        match h.status {
            HoldStatus::Held => {
                let ids: Vec<&str> = s.devices.iter().filter(|d| d.hold_id == h.id).filter_map(|d| d.device.as_deref()).collect();
                let used = p.in_use.contains(&h.id) || named_by_run(s, &ids);
                match reclaim_reason(h, s, p, lim, used) {
                    Some(why) => out.push(Action::Reclaim(h.id.clone(), why)),
                    None if used => out.push(Action::MarkUsed(h.id.clone())),
                    None => {}
                }
            }
            HoldStatus::Waiting if gone(p, &h.id) => out.push(Action::RespawnHold(h.id.clone())),
            _ => {}
        }
    }
}

/// 当前占用：运行中的执行 + 已分配借用的设备和账号（本轮被收回的借用只释放账号和真机，模拟器删完前仍计内存）。
fn ledger(s: &Snapshot, p: &Probes, actions: &[Action], cap: &Capacity) -> Ledger {
    let reclaimed: Vec<&str> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Reclaim(id, _) | Action::RespawnHold(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let interrupted: Vec<&str> =
        actions.iter().filter_map(|a| if let Action::Interrupt(id) = a { Some(id.as_str()) } else { None }).collect();
    let mut l = Ledger::default();
    for e in s.execs.iter().filter(|e| e.status == ExecStatus::Running && !interrupted.contains(&e.id.as_str())) {
        if let Some(r) = &e.res {
            l.add_exec(r, p.group_mem_mb.get(&e.id).copied().unwrap_or(0));
        }
    }
    for d in &s.devices {
        let freed = reclaimed.contains(&d.hold_id.as_str());
        if d.dtype.is_virtual() {
            if !(freed && d.device.is_none()) {
                l.add_sim(d.dtype, cap.sim_mem_mb(d.dtype), d.device.is_some());
            }
        } else if let (Some(dev), false) = (&d.device, freed) {
            l.devices.insert(dev.clone());
        }
    }
    for a in s.accounts.iter().filter(|a| !reclaimed.contains(&a.hold_id.as_str())) {
        l.accounts.insert(a.account.clone());
    }
    l
}

/// 排队项：已评估的 queued 执行 + 还没分配的 waiting 借用，按到达时间（同时到达按 id）。
fn queue(s: &Snapshot, actions: &[Action]) -> Vec<Item> {
    let mut v: Vec<(i64, String, Demand)> = vec![];
    for e in s.execs.iter().filter(|e| e.status == ExecStatus::Queued) {
        if let Some(r) = e.res {
            v.push((e.submitted_at, e.id.clone(), Demand::Exec(r)));
        }
    }
    let allocated: Vec<&str> = s.devices.iter().map(|d| d.hold_id.as_str()).collect();
    for h in s.holds.iter().filter(|h| h.status == HoldStatus::Waiting) {
        let respawn = actions.contains(&Action::RespawnHold(h.id.clone()));
        if respawn || !allocated.contains(&h.id.as_str()) {
            v.push((h.requested_at, h.id.clone(), Demand::Hold(h.request.clone())));
        }
    }
    v.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    v.into_iter().map(|(_, id, demand)| Item { id, demand }).collect()
}

pub fn plan(s: &Snapshot, p: &Probes, cap: &Capacity, lim: &Limits) -> Plan {
    let mut actions = vec![];
    recover_execs(s, p, &mut actions);
    reclaim_holds(s, p, lim, &mut actions);
    let l = ledger(s, p, &actions, cap);
    let admit = admit(&queue(s, &actions), l, cap, &p.sys);
    Plan { actions, admit }
}

#[cfg(test)]
mod tests;
