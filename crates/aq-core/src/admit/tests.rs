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
    Item { id: id.into(), demand: Demand::Hold(HoldRequest { devices, accounts, exclude: vec![] }) }
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
    let big = Demand::Hold(HoldRequest { devices: vec![DeviceType::IosSim; 2], accounts: vec![], exclude: vec![] });
    assert!(unschedulable(&big, &c).is_some());
}

// CASE-HOLD-6
#[test]
fn more_phones_than_list() {
    let d = Demand::Hold(HoldRequest { devices: vec![DeviceType::IosDevice; 2], accounts: vec![], exclude: vec![] });
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
