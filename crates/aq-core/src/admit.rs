//! 放行：资源记账 + 先来先服务 + 排头保留。

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::capacity::Capacity;
use crate::model::{DeviceType, HoldRequest, Resources};

/// 放行时可能缺的一类资源。排头缺哪类，后来者就不能用哪类。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Heavy,
    Mem,
    Disk,
    Net,
    Device(DeviceType),
    Account(String),
}

impl Kind {
    /// 给人看的「在等什么」。
    pub fn label(&self) -> String {
        match self {
            Kind::Heavy => "重任务名额".into(),
            Kind::Mem => "内存".into(),
            Kind::Disk => "磁盘".into(),
            Kind::Net => "下载名额".into(),
            Kind::Device(t) => format!("设备 {t}"),
            Kind::Account(a) => format!("账号 {a}"),
        }
    }
}

/// 系统实测值。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SysSample {
    pub mem_total_mb: i64,
    pub mem_avail_mb: i64,
    pub disk_total_mb: i64,
    pub disk_free_mb: i64,
    pub cpu_pct: i64,
}

/// 一个排队项要的东西。
#[derive(Debug, Clone, PartialEq)]
pub enum Demand {
    Exec(Resources),
    Hold(HoldRequest),
}

/// 当前占用（已放行的执行、已分配的借用）。
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub heavy: i64,
    pub mem_acct_mb: i64,
    /// Σ max(0, 记账 − 实际)：已记账但还没用上的内存，实际余量要先扣掉。
    pub mem_unused_mb: i64,
    pub disk_acct_mb: i64,
    pub net: i64,
    pub devices: HashSet<String>,
    pub accounts: HashSet<String>,
}

impl Ledger {
    pub fn add_exec(&mut self, r: &Resources, used_mem_mb: i64) {
        self.heavy += i64::from(r.heavy);
        self.net += i64::from(r.uses_net);
        self.mem_acct_mb += r.mem_mb;
        self.mem_unused_mb += (r.mem_mb - used_mem_mb).max(0);
        self.disk_acct_mb += r.disk_mb;
    }
    /// 模拟器按配置内存记账；已建好的视为已用满，创建中的视为还没用。
    pub fn add_sim(&mut self, mem_mb: i64, created: bool) {
        self.mem_acct_mb += mem_mb;
        if !created {
            self.mem_unused_mb += mem_mb;
        }
    }
}

/// 一次放行的结果：真机分到哪几台（按请求顺序，模拟器为 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct Admission {
    pub id: String,
    pub devices: Vec<(DeviceType, Option<String>)>,
}

pub struct Item {
    pub id: String,
    pub demand: Demand,
}

struct Need {
    heavy: bool,
    mem: i64,
    disk: i64,
    net: bool,
    devices: Vec<DeviceType>,
    accounts: Vec<String>,
    exclude: Vec<String>,
}

fn need_of(d: &Demand, cap: &Capacity) -> Need {
    match d {
        Demand::Exec(r) => {
            Need { heavy: r.heavy, mem: r.mem_mb, disk: r.disk_mb, net: r.uses_net, devices: vec![], accounts: vec![], exclude: vec![] }
        }
        Demand::Hold(h) => Need {
            heavy: false,
            mem: h.devices.iter().map(|t| cap.sim_mem_mb(*t)).sum(),
            disk: 0,
            net: false,
            devices: h.devices.clone(),
            accounts: h.accounts.clone(),
            exclude: h.exclude.clone(),
        },
    }
}

/// 可放行内存 = min(账面余量, 实际余量 − 已记账未用上的部分)。
fn mem_room(l: &Ledger, cap: &Capacity, sys: &SysSample) -> i64 {
    let book = cap.mem_mb - l.mem_acct_mb;
    let actual = sys.mem_avail_mb - l.mem_unused_mb;
    book.min(actual)
}

fn disk_room(l: &Ledger, cap: &Capacity, sys: &SysSample) -> i64 {
    sys.disk_free_mb - cap.disk_floor_mb - l.disk_acct_mb
}

/// 返回缺的资源类；为空表示放得下。真机按配置顺序挑空闲的。
fn lacking(n: &Need, l: &Ledger, cap: &Capacity, sys: &SysSample) -> (BTreeSet<Kind>, Vec<Option<String>>) {
    let mut miss = BTreeSet::new();
    if n.heavy && l.heavy >= cap.heavy_slots {
        miss.insert(Kind::Heavy);
    }
    if n.mem > 0 && n.mem > mem_room(l, cap, sys) {
        miss.insert(Kind::Mem);
    }
    if n.disk > 0 && n.disk > disk_room(l, cap, sys) {
        miss.insert(Kind::Disk);
    }
    if n.net && l.net >= cap.net_slots {
        miss.insert(Kind::Net);
    }
    let mut picked: Vec<Option<String>> = vec![];
    let mut taken: HashSet<String> = HashSet::new();
    for t in &n.devices {
        if t.is_virtual() {
            picked.push(None);
            continue;
        }
        let free = cap.real_devices(*t).iter().find(|d| !l.devices.contains(*d) && !taken.contains(*d) && !n.exclude.contains(*d));
        match free {
            Some(d) => {
                taken.insert(d.clone());
                picked.push(Some(d.clone()));
            }
            None => {
                miss.insert(Kind::Device(*t));
            }
        }
    }
    for a in &n.accounts {
        if l.accounts.contains(a) {
            miss.insert(Kind::Account(a.clone()));
        }
    }
    (miss, picked)
}

fn touches(n: &Need, reserved: &BTreeSet<Kind>) -> bool {
    reserved.iter().any(|k| match k {
        Kind::Heavy => n.heavy,
        Kind::Mem => n.mem > 0,
        Kind::Disk => n.disk > 0,
        Kind::Net => n.net,
        Kind::Device(t) => n.devices.contains(t),
        Kind::Account(a) => n.accounts.contains(a),
    })
}

fn commit(n: &Need, picked: &[Option<String>], l: &mut Ledger, cap: &Capacity) {
    l.heavy += i64::from(n.heavy);
    l.net += i64::from(n.net);
    l.disk_acct_mb += n.disk;
    for (t, d) in n.devices.iter().zip(picked) {
        if let Some(d) = d {
            l.devices.insert(d.clone());
        } else if t.is_virtual() {
            l.add_sim(cap.sim_mem_mb(*t), false);
        }
    }
    if n.devices.is_empty() {
        l.mem_acct_mb += n.mem;
        l.mem_unused_mb += n.mem;
    }
    for a in &n.accounts {
        l.accounts.insert(a.clone());
    }
}

/// 放行结果：谁能放行，以及每个没放行的项在等什么。
#[derive(Debug, Clone, Default)]
pub struct AdmitResult {
    pub admitted: Vec<Admission>,
    pub waiting_for: HashMap<String, Vec<Kind>>,
}

/// 按到达顺序过一遍队列。`items` 必须已按到达时间排好。
pub fn admit(items: &[Item], mut l: Ledger, cap: &Capacity, sys: &SysSample) -> AdmitResult {
    let mut out = AdmitResult::default();
    let mut reserved: Option<BTreeSet<Kind>> = None;
    for it in items {
        let n = need_of(&it.demand, cap);
        let (miss, picked) = lacking(&n, &l, cap, sys);
        let blocked_by_head = reserved.as_ref().is_some_and(|r| touches(&n, r));
        if miss.is_empty() && !blocked_by_head {
            commit(&n, &picked, &mut l, cap);
            let devices = n.devices.iter().copied().zip(picked).collect();
            out.admitted.push(Admission { id: it.id.clone(), devices });
            continue;
        }
        let mut why: Vec<Kind> = miss.iter().cloned().collect();
        if why.is_empty() {
            if let Some(r) = &reserved {
                why = r.iter().cloned().collect();
            }
        }
        if reserved.is_none() {
            reserved = Some(miss);
        }
        out.waiting_for.insert(it.id.clone(), why);
    }
    out
}

/// 永远满足不了的需求（超过整机容量、真机比清单多）→ rejected unschedulable。
pub fn unschedulable(d: &Demand, cap: &Capacity) -> Option<String> {
    let n = need_of(d, cap);
    if n.mem > cap.mem_mb {
        return Some(format!("需要内存 {}MB，超过整机容量 {}MB", n.mem, cap.mem_mb));
    }
    if n.heavy && cap.heavy_slots < 1 {
        return Some("重任务名额为 0".into());
    }
    for t in DeviceType::ALL.iter().filter(|t| !t.is_virtual()) {
        let want = n.devices.iter().filter(|x| *x == t).count();
        let have = cap.real_devices(*t).len();
        if want > have {
            return Some(format!("要 {want} 台 {t}，清单里只有 {have} 台"));
        }
    }
    None
}

#[cfg(test)]
mod tests;
