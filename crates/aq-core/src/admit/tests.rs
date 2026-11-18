use super::*;

fn cap() -> Capacity {
    Capacity { ios_devices: vec!["phone1".into()], ..Capacity::default() }
}
fn sys() -> SysSample {
    SysSample { mem_total_mb: 36_000, mem_avail_mb: 30_000, disk_total_mb: 1_000_000, disk_free_mb: 500_000 }
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
