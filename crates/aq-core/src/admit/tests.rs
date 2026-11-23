use super::*;

fn cap() -> Capacity {
    Capacity { ios_devices: vec!["phone1".into()], ..Capacity::default() }
}
fn sys() -> SysSample {
    SysSample { mem_total_mb: 36_000, mem_avail_mb: 30_000, disk_total_mb: 1_000_000, disk_free_mb: 500_000, ..SysSample::default() }
}
fn ex(id: &str, heavy: bool, mem: i64) -> Item {
    Item { id: id.into(), demand: Demand::Exec(Resources { heavy, mem_mb: mem, disk_mb: 0, uses_net: false }) }
}
fn hold(id: &str, devices: Vec<DeviceType>, accounts: &[&str]) -> Item {
    let accounts = accounts.iter().map(|s| s.to_string()).collect();
    Item { id: id.into(), demand: Demand::Hold(HoldRequest { devices, accounts, exclude: vec![], specs: vec![] }) }
}
fn ids(r: &AdmitResult) -> Vec<&str> {
    r.admitted.iter().map(|a| a.id.as_str()).collect()
}

// CASE-QUEUE-1
#[test]
fn at_most_two_heavy() {
    let items: Vec<Item> = (0..4).map(|i| ex(&format!("e{i}"), true, 1000)).collect();
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(ids(&r), vec!["e0", "e1"]);
    assert_eq!(r.waiting_for["e2"], vec![Kind::Heavy]);
}

// CASE-QUEUE-2
#[test]
fn book_memory_limits() {
    let items = vec![ex("a", false, 20_000), ex("b", false, 10_000)];
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(ids(&r), vec!["a"]);
}

// CASE-QUEUE-2
#[test]
fn actual_memory_limits() {
    let s = SysSample { mem_avail_mb: 5_000, ..sys() };
    let r = admit(&[ex("a", false, 8_000)], Ledger::default(), &cap(), &s);
    assert!(r.admitted.is_empty());
    assert_eq!(r.waiting_for["a"], vec![Kind::Mem]);
}

// CASE-QUEUE-2
#[test]
fn actual_check_subtracts_booked_but_unused() {
    let mut l = Ledger::default();
    l.add_exec(&Resources { heavy: false, mem_mb: 10_000, disk_mb: 0, uses_net: false }, 1_000);
    let s = SysSample { mem_avail_mb: 15_000, ..sys() };
    // 实际余量 15000 − 未用上的 9000 = 6000
    let r = admit(&[ex("a", false, 7_000)], l, &cap(), &s);
    assert!(r.admitted.is_empty());
}

// CASE-QUEUE-2
#[test]
fn disk_floor_and_net_slots() {
    let s = SysSample { disk_free_mb: 50 * 1024, ..sys() };
    let item = Item { id: "d".into(), demand: Demand::Exec(Resources { heavy: false, mem_mb: 1, disk_mb: 1, uses_net: false }) };
    assert!(admit(&[item], Ledger::default(), &cap(), &s).admitted.is_empty());
    let l = Ledger { net: 3, ..Ledger::default() };
    let item = Item { id: "n".into(), demand: Demand::Exec(Resources { heavy: false, mem_mb: 1, disk_mb: 0, uses_net: true }) };
    assert!(admit(&[item], l, &cap(), &sys()).admitted.is_empty());
}

// CASE-QUEUE-3
#[test]
fn head_reserves_only_what_it_lacks() {
    let items = vec![ex("head", false, 29_000), ex("nomem", false, 0), ex("mem", false, 100)];
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(ids(&r), vec!["nomem"]);
}

// CASE-QUEUE-3
#[test]
fn hold_waiting_for_phone_blocks_later_phone_hold_not_run() {
    let l = Ledger { devices: ["phone1".to_string()].into(), ..Ledger::default() };
    let items = vec![hold("h1", vec![DeviceType::IosDevice], &[]), ex("run", true, 1000), hold("h2", vec![DeviceType::IosDevice], &[])];
    let r = admit(&items, l, &cap(), &sys());
    assert_eq!(ids(&r), vec!["run"]);
    assert_eq!(r.waiting_for["h2"], vec![Kind::Device(DeviceType::IosDevice)]);
}

// CASE-HOLD-4
#[test]
fn whole_group_or_nothing() {
    let items = vec![hold("h1", vec![DeviceType::IosDevice], &["a"]), hold("h2", vec![DeviceType::IosDevice], &["a"])];
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(ids(&r), vec!["h1"]);
    assert_eq!(r.admitted[0].devices, vec![(DeviceType::IosDevice, Some("phone1".into()))]);
}

// CASE-HOLD-5
#[test]
fn account_taken_waits() {
    let l = Ledger { accounts: ["test-a".to_string()].into(), ..Ledger::default() };
    let r = admit(&[hold("h", vec![DeviceType::IosSim], &["test-a"])], l, &cap(), &sys());
    assert_eq!(r.waiting_for["h"], vec![Kind::Account("test-a".into())]);
}

// CASE-HOLD-6
#[test]
fn sims_by_memory() {
    let c = Capacity { mem_mb: 4_000, ..cap() };
    let items = vec![hold("h1", vec![DeviceType::IosSim], &[]), hold("h2", vec![DeviceType::IosSim], &[])];
    let r = admit(&items, Ledger::default(), &c, &sys());
    assert_eq!(ids(&r), vec!["h1"]);
    let big = Demand::Hold(HoldRequest { devices: vec![DeviceType::IosSim; 2], accounts: vec![], exclude: vec![], specs: vec![] });
    assert!(unschedulable(&big, &c).is_some());
}

// CASE-HOLD-6
#[test]
fn more_phones_than_list() {
    let d = Demand::Hold(HoldRequest { devices: vec![DeviceType::IosDevice; 2], accounts: vec![], exclude: vec![], specs: vec![] });
    assert!(unschedulable(&d, &cap()).is_some());
}

// CASE-EST-9
#[test]
fn exec_over_capacity() {
    let d = Demand::Exec(Resources { heavy: true, mem_mb: 40_000, disk_mb: 0, uses_net: false });
    assert!(unschedulable(&d, &cap()).is_some());
}

// CASE-QUEUE-1
#[test]
fn admission_never_exceeds_slots_with_existing_load() {
    let l = Ledger { heavy: 2, ..Ledger::default() };
    let r = admit(&[ex("a", true, 1)], l, &cap(), &sys());
    assert!(r.admitted.is_empty());
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 任意队列：放行后重任务 ≤ 名额、内存记账 ≤ 容量、同一真机或账号不重复。
        // CASE-QUEUE-1, CASE-QUEUE-2
        #[test]
        fn invariants_hold(specs in proptest::collection::vec((any::<bool>(), 0i64..20_000, 0u8..4, any::<bool>()), 0..30)) {
            let items: Vec<Item> = specs.iter().enumerate().map(|(i, (h, m, kind, acct))| {
                if *kind == 0 {
                    let accts: &[&str] = if *acct { &["x"] } else { &[] };
                    hold(&format!("h{i}"), vec![DeviceType::IosDevice, DeviceType::IosSim], accts)
                } else { ex(&format!("e{i}"), *h, *m) }
            }).collect();
            let c = cap();
            let r = admit(&items, Ledger::default(), &c, &sys());
            let mut heavy = 0; let mut mem = 0; let mut phones = 0; let mut accts = 0;
            for a in &r.admitted {
                let it = items.iter().find(|i| i.id == a.id).unwrap();
                match &it.demand {
                    Demand::Exec(res) => { heavy += i64::from(res.heavy); mem += res.mem_mb; }
                    Demand::Hold(h) => { phones += 1; mem += c.ios_sim_mem_mb; accts += h.accounts.len(); }
                }
            }
            prop_assert!(heavy <= c.heavy_slots);
            prop_assert!(mem <= c.mem_mb);
            prop_assert!(phones <= 1);
            prop_assert!(accts <= 1);
        }
    }
}

// CASE-QUEUE-5
#[test]
fn memory_pressure_blocks_memory_users_only() {
    let items = vec![ex("e", false, 100), hold("s", vec![DeviceType::IosSim], &[]), hold("p", vec![DeviceType::IosDevice], &[])];
    for s in [
        SysSample { pressure: Pressure::Warn, ..sys() },
        SysSample { pressure: Pressure::Critical, ..sys() },
        SysSample { swapping: true, ..sys() },
    ] {
        let r = admit(&items, Ledger::default(), &cap(), &s);
        assert_eq!(ids(&r), vec!["p"], "只放不要内存的真机");
        assert_eq!(r.waiting_for["e"], vec![Kind::MemPressure]);
        assert_eq!(r.waiting_for["s"], vec![Kind::MemPressure]);
    }
    assert_eq!(ids(&admit(&items, Ledger::default(), &cap(), &sys())), vec!["e", "s", "p"], "压力正常、余量够：都放行");
}

// CASE-QUEUE-6
#[test]
fn one_cold_boot_at_a_time() {
    let items: Vec<Item> = (0..3).map(|i| hold(&format!("s{i}"), vec![DeviceType::IosSim], &[])).collect();
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(ids(&r), vec!["s0"], "同一轮只放一台冷启动");
    assert_eq!(r.waiting_for["s1"], vec![Kind::ColdStart]);
    let mut l = Ledger::default();
    l.add_sim(DeviceType::IosSim, 3072, true);
    assert_eq!(ids(&admit(&items, l, &cap(), &sys())), vec!["s0"], "已建好的不挡");
    let mut l = Ledger::default();
    l.add_sim(DeviceType::AndroidEmu, 4096, false);
    let r = admit(&[hold("b", vec![DeviceType::Browser], &[]), hold("s0", vec![DeviceType::IosSim], &[])], l, &cap(), &sys());
    assert_eq!(ids(&r), vec!["b"], "Android 实例在建时浏览器会话照借，iOS 模拟器排队");
}

// CASE-HOLD-12
#[test]
fn simulator_count_limit() {
    let c = Capacity { sim_slots: 2, ..cap() };
    let mut l = Ledger::default();
    l.add_sim(DeviceType::IosSim, 3072, true);
    l.add_sim(DeviceType::AndroidEmu, 4096, true);
    l.add_sim(DeviceType::Browser, 1024, true);
    let items = vec![hold("s", vec![DeviceType::IosSim], &[]), hold("b", vec![DeviceType::Browser], &[])];
    let r = admit(&items, l.clone(), &c, &sys());
    assert_eq!(ids(&r), vec!["b"], "浏览器会话不算台数");
    assert_eq!(r.waiting_for["s"], vec![Kind::SimSlots]);
    let mut one = Ledger::default();
    one.add_sim(DeviceType::IosSim, 3072, true);
    assert_eq!(ids(&admit(&items, one, &c, &sys())), vec!["s", "b"], "还差一台：放行");
    assert!(unschedulable(&hold("x", vec![DeviceType::IosSim; 3], &[]).demand, &c).is_some(), "一次要的超过上限：拒绝");
}

fn exd(id: &str, heavy: bool, mem: i64, disk: i64) -> Item {
    Item { id: id.into(), demand: Demand::Exec(Resources { heavy, mem_mb: mem, disk_mb: disk, uses_net: false }) }
}
const GB: i64 = 1024;

// CASE-QUEUE-3
#[test]
fn blocked_item_names_its_head() {
    let items = vec![ex("head", false, 29_000), ex("mem", false, 100)];
    let r = admit(&items, Ledger::default(), &cap(), &sys());
    assert_eq!(r.blocked_by.get("mem").map(String::as_str), Some("head"));
    assert_eq!(r.waiting_for["mem"], vec![Kind::Mem]);
    assert!(!r.blocked_by.contains_key("head"), "排头自己缺资源，不算被挡");
}

// CASE-QUEUE-8
#[test]
fn unreachable_disk_does_not_hold_back_later_tasks() {
    // 本次真实场景：剩余 57G、底线 50G、排头要 12G、没有运行中任务
    let s = SysSample { disk_free_mb: 57 * GB, ..sys() };
    let items = vec![exd("head", false, 1000, 12 * GB), exd("small", false, 500, 186), exd("big", false, 500, 3 * GB)];
    let r = admit(&items, Ledger::default(), &cap(), &s);
    assert_eq!(ids(&r), vec!["small", "big"]);
    assert_eq!(r.waiting_for["head"], vec![Kind::DiskFull]);
    assert!(r.blocked_by.is_empty());
    let g = r.disk_full["head"];
    assert_eq!(g.short_mb(), 12 * GB - 7 * GB, "缺口 = 需要 − (剩余 − 底线 − 运行中记账)");
}

// CASE-QUEUE-8
#[test]
fn reachable_disk_stays_reserved() {
    // 余量 60 − 50 − 8 = 2G，排头要 6G、缺 4G；运行中任务记账 8G，结束后就够：排头仍预留磁盘，后来要磁盘的不能先走
    let s = SysSample { disk_free_mb: 60 * GB, ..sys() };
    let l = Ledger { disk_acct_mb: 8 * GB, ..Ledger::default() };
    let items = vec![exd("head", false, 1000, 6 * GB), exd("small", false, 500, 186), exd("nodisk", false, 500, 0)];
    let r = admit(&items, l, &cap(), &s);
    assert_eq!(ids(&r), vec!["nodisk"]);
    assert_eq!(r.waiting_for["head"], vec![Kind::Disk]);
    assert_eq!(r.blocked_by.get("small").map(String::as_str), Some("head"));
}

// CASE-QUEUE-9
#[test]
fn unreachable_disk_still_reserves_other_kinds() {
    // 排头同时缺内存和等不到的磁盘：内存照样预留给它，磁盘不预留
    let s = SysSample { disk_free_mb: 57 * GB, mem_avail_mb: 3_000, ..sys() };
    let items = vec![exd("head", true, 8_000, 12 * GB), exd("mem", false, 1_000, 0), exd("disk", false, 0, 100)];
    let r = admit(&items, Ledger::default(), &cap(), &s);
    assert_eq!(ids(&r), vec!["disk"]);
    assert_eq!(r.waiting_for["head"], vec![Kind::Mem, Kind::DiskFull]);
    assert_eq!(r.blocked_by.get("mem").map(String::as_str), Some("head"));
}

// CASE-QUEUE-10
#[test]
fn next_blocked_takes_over_as_head() {
    let s = SysSample { disk_free_mb: 57 * GB, mem_avail_mb: 2_000, ..sys() };
    let items = vec![exd("disk", false, 100, 12 * GB), exd("mem", false, 5_000, 0), exd("later", false, 1_000, 0)];
    let r = admit(&items, Ledger::default(), &cap(), &s);
    assert!(ids(&r).is_empty(), "later 自己放得下 1000，但内存预留给新排头 mem");
    assert_eq!(r.waiting_for["mem"], vec![Kind::Mem]);
    assert_eq!(r.blocked_by.get("later").map(String::as_str), Some("mem"));
}

// CASE-QUEUE-11
#[test]
fn below_floor_only_zero_disk_runs() {
    let s = SysSample { disk_free_mb: 40 * GB, ..sys() };
    let items = vec![exd("a", false, 100, 100), exd("zero", false, 100, 0)];
    let r = admit(&items, Ledger::default(), &cap(), &s);
    assert_eq!(ids(&r), vec!["zero"]);
    assert_eq!(r.waiting_for["a"], vec![Kind::DiskFull]);
}
