//! 契约
//! 假件在 fakes.rs；真件只在 macOS 上跑；真 simctl 创建删除较慢，标 #[ignore]，发版前实机演练时跑。
// 测试代码：断言失败就该 panic 并打印上下文，unwrap/expect/panic 是期望行为
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fakes;

use std::path::Path;

use aq_core::model::ProcId;
use aq_core::ports::{AiCli, Clock, Git, Locks, Procs, Simulator, SysRes};

macro_rules! contract {
    ($name:ident, $check:path, real = $real:expr, fake = $fake:expr) => {
        mod $name {
            #[test]
            fn real() {
                $check(&$real);
            }
            #[test]
            fn fake() {
                $check(&$fake);
            }
        }
    };
}

pub fn clock_contract(c: &dyn Clock) {
    let (a, b) = (c.now_ms(), c.now_ms());
    assert!(a > 1_700_000_000_000 && b >= a, "now_ms 是 Unix 毫秒且不倒退");
    let (u1, u2) = (c.uptime_ms(), c.uptime_ms());
    assert!(u1 > 0 && u2 >= u1, "uptime 单调");
}

pub fn procs_contract(p: &dyn Procs) {
    let me = std::process::id() as i64;
    let start = p.start_time(me).expect("自己的启动时间");
    assert!(p.alive(ProcId { pid: me, started_at: start }));
    assert!(!p.alive(ProcId { pid: me, started_at: start - 1000 }), "启动时间不同 = 已不在（进程号复用）");
    assert_eq!(p.start_time(99_999_999), None);
    assert_eq!(p.ancestors(me).first().map(|a| a.0), Some(me), "父进程链从自己开始");
    assert!(!p.group_alive(ProcId { pid: 99_999_999, started_at: 1 }));
}

pub fn sysres_contract(s: &dyn SysRes) {
    let x = s.sample();
    assert!(x.mem_total_mb > 0 && (0..=x.mem_total_mb).contains(&x.mem_avail_mb));
    assert!(x.disk_total_mb > 0 && (0..=x.disk_total_mb).contains(&x.disk_free_mb));
    assert!((0..=100).contains(&x.cpu_pct));
}

pub fn locks_contract<L: Locks>(l: &L) {
    let a = l.try_lock("x").unwrap();
    assert!(a.is_some(), "没人持有时拿得到");
    assert!(l.try_lock("x").unwrap().is_none(), "持有期间别人拿不到");
    assert!(l.try_lock("y").unwrap().is_some(), "不同名字互不影响");
    drop(a);
    // 同进程里并行的测试在 fork 时会短暂继承这把锁的 fd（到 exec 时才关），所以允许 1 秒内拿到
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut got = false;
    while !got && std::time::Instant::now() < deadline {
        got = l.try_lock("x").unwrap().is_some();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(got, "持有者释放后拿得到");
}

pub fn sim_contract(s: &dyn Simulator) {
    let udid = s.create("aq-hd_contract-1").unwrap();
    assert!(s.list_aq().unwrap().contains(&("aq-hd_contract-1".to_string(), udid.clone())));
    s.delete(&udid).unwrap();
    assert!(!s.list_aq().unwrap().iter().any(|(_, u)| *u == udid));
    assert!(s.delete(&udid).is_err(), "删不存在的报错");
}

pub fn ai_contract(a: &dyn AiCli) {
    let out = a.ask("评估这条命令").unwrap();
    assert!(aq_core::estimate::parse_ai(&out, &Default::default(), &Default::default()).is_some(), "{out}");
}

/// git：仓库外为 None；干净 = HEAD SHA；有改动 = SHA+dirty.；子目录给出相对路径。
pub trait GitWorld {
    fn git(&self) -> &dyn Git;
    fn root(&self) -> &Path;
    fn dirty(&self);
}

pub fn git_contract(w: &dyn GitWorld) {
    let outside = tempfile::tempdir().unwrap();
    assert_eq!(w.git().locate(outside.path().to_str().unwrap()).unwrap(), None);
    let r = w.git().locate(w.root().to_str().unwrap()).unwrap().expect("在仓库里");
    let fp = r.fingerprint.expect("干净时有指纹");
    assert!(fp.len() == 40 && !fp.contains('+'), "{fp}");
    assert_eq!(r.rel_cwd, "");
    let sub = w.root().join("Modules");
    let r = w.git().locate(sub.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(r.rel_cwd, "Modules");
    w.dirty();
    let d = w.git().locate(w.root().to_str().unwrap()).unwrap().unwrap().fingerprint.unwrap();
    assert!(d.starts_with(&fp) && d.contains("+dirty."), "{d}");
}

contract!(clock, super::clock_contract, real = aq_sys::clock::RealClock, fake = super::fakes::FakeClock::new());
contract!(procs, super::procs_contract, real = aq_sys::procs::RealProcs, fake = super::fakes::FakeProcs::with_self());
contract!(sysres, super::sysres_contract, real = aq_sys::sysres::RealSys { disk_path: std::env::temp_dir() }, fake = super::fakes::FakeSys);
contract!(locks, super::locks_contract, real = super::fakes::real_locks(), fake = super::fakes::FakeLocks::default());
contract!(simulator, super::sim_contract, real = super::fakes::simctl_adapter(), fake = super::fakes::FakeSim::default());
contract!(ai_cli, super::ai_contract, real = super::fakes::ai_command(), fake = super::fakes::FakeAi);
contract!(git, super::git_contract, real = super::fakes::RealGitWorld::new(), fake = super::fakes::FakeGitWorld::new());

/// 真 simctl：建一台真模拟器再删掉（约十几秒，需要 Xcode）。
#[test]
#[ignore]
fn simulator_real_xcrun() {
    sim_contract(&aq_sys::sim::SimCtl { cmd: vec!["xcrun".into(), "simctl".into()], model: "iPhone 17 Pro".into() });
}

/// 真 Android 模拟器：按 AQ_TEST_AVD（默认 aq_lowend26）起一个 read-only 实例再关掉（需要 Android SDK）。
#[test]
#[ignore]
fn simulator_real_android_emulator() {
    let avd = std::env::var("AQ_TEST_AVD").unwrap_or_else(|_| "aq_lowend26".into());
    let dir = tempfile::tempdir().unwrap();
    sim_contract(&aq_sys::emu::Emulator::from_env(&avd, dir.path()));
}

/// 真 agent-browser：开一个无窗口 Chrome 会话再关掉（约 2 秒，需要装 agent-browser）。
#[test]
#[ignore]
fn simulator_real_agent_browser() {
    sim_contract(&aq_sys::browser::AgentBrowser { cmd: vec!["agent-browser".into()] });
}

/// 真件：进程组内 2 个忙循环跑 1.5 秒，按累计 CPU 时间算出约 2 核（验证 mach 时钟单位换算，CASE-EST-10 的前提）。
// CASE-EST-10
#[test]
fn procs_real_group_cpu_measures_cores() {
    use std::os::unix::process::CommandExt;
    let mut leader = std::process::Command::new("sh")
        .args(["-c", "python3 -c 'while True: pass' & python3 -c 'while True: pass' & wait"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = i64::from(leader.id());
    let p = aq_sys::procs::RealProcs;
    let mut w = aq_core::cpu_watch::CpuWatch::new(1500);
    let t0 = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(300));
    w.observe(0, &p.group_cpu_ns(pgid));
    while t0.elapsed() < std::time::Duration::from_millis(1900) {
        std::thread::sleep(std::time::Duration::from_millis(250));
        w.observe(t0.elapsed().as_millis() as i64 - 300, &p.group_cpu_ns(pgid));
    }
    aq_sys::spawn::killpg(pgid, aq_sys::spawn::SIGKILL);
    let _ = leader.wait();
    let cores = w.window_cores().unwrap();
    assert!((1.5..=2.5).contains(&cores), "测到 {cores} 核");
}
