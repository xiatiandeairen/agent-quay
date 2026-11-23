//! 7 个 port 的假件，以及真件的构造辅助。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use aq_core::admit::SysSample;
use aq_core::model::ProcId;
use aq_core::ports::{AiCli, Clock, Git, Locks, Procs, RepoInfo, Simulator, SysRes};

use super::GitWorld;

pub struct FakeClock {
    now: RefCell<i64>,
}
impl FakeClock {
    pub fn new() -> Self {
        FakeClock { now: RefCell::new(1_800_000_000_000) }
    }
}
impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        *self.now.borrow_mut() += 1;
        *self.now.borrow()
    }
    fn uptime_ms(&self) -> i64 {
        *self.now.borrow() - 1_799_999_000_000
    }
}

pub struct FakeProcs {
    table: HashMap<i64, (i64, i64)>, // pid → (ppid, start)
}
impl FakeProcs {
    pub fn with_self() -> Self {
        let me = std::process::id() as i64;
        FakeProcs { table: [(me, (1, 42))].into() }
    }
}
impl Procs for FakeProcs {
    fn start_time(&self, pid: i64) -> Option<i64> {
        self.table.get(&pid).map(|v| v.1)
    }
    fn group_alive(&self, leader: ProcId) -> bool {
        self.alive(leader)
    }
    fn group_mem_mb(&self, _pgid: i64) -> i64 {
        0
    }
    fn ancestors(&self, pid: i64) -> Vec<(i64, String, i64)> {
        let mut out = vec![];
        let mut cur = pid;
        while let Some((pp, st)) = self.table.get(&cur) {
            out.push((cur, "fake".into(), *st));
            cur = *pp;
        }
        out
    }
}

#[derive(Default)]
pub struct FakeSys;
impl SysRes for FakeSys {
    fn sample(&self) -> SysSample {
        SysSample { mem_total_mb: 36_864, mem_avail_mb: 20_000, disk_total_mb: 1_000_000, disk_free_mb: 400_000, ..SysSample::default() }
    }
    fn cpu_pct(&self) -> i64 {
        30
    }
}

pub fn real_locks() -> aq_sys::lock::LockDir {
    let d = tempfile::tempdir().unwrap().keep();
    aq_sys::lock::LockDir { dir: d }
}

#[derive(Default)]
pub struct FakeLocks {
    held: Rc<RefCell<HashSet<String>>>,
}
pub struct FakeGuard(Rc<RefCell<HashSet<String>>>, String);
impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.0.borrow_mut().remove(&self.1);
    }
}
impl Locks for FakeLocks {
    type Guard = FakeGuard;
    fn try_lock(&self, name: &str) -> Result<Option<FakeGuard>, String> {
        if !self.held.borrow_mut().insert(name.to_string()) {
            return Ok(None);
        }
        Ok(Some(FakeGuard(self.held.clone(), name.to_string())))
    }
}

/// 真的 SimCtl 适配器，对接 tests/support/fake-simctl（验证命令拼装与 JSON 解析）。
pub fn simctl_adapter() -> aq_sys::sim::SimCtl {
    let dir = tempfile::tempdir().unwrap().keep();
    // SAFETY 说明不需要：测试进程内设置子进程要读的环境变量
    std::env::set_var("FAKE_SIM_DIR", &dir);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake-simctl");
    aq_sys::sim::SimCtl { cmd: vec![script.to_string_lossy().into_owned()], model: "iPhone 17 Pro".into() }
}

#[derive(Default)]
pub struct FakeSim {
    devs: RefCell<HashMap<String, String>>,
}
impl Simulator for FakeSim {
    fn create(&self, name: &str, _spec: &aq_core::devspec::DeviceSpec) -> Result<String, String> {
        let u = format!("U-{}", self.devs.borrow().len());
        self.devs.borrow_mut().insert(u.clone(), name.into());
        Ok(u)
    }
    fn delete(&self, udid: &str) -> Result<(), String> {
        self.devs.borrow_mut().remove(udid).map(|_| ()).ok_or_else(|| "no such device".into())
    }
    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self.devs.borrow().iter().filter(|(_, n)| n.starts_with("aq-")).map(|(u, n)| (n.clone(), u.clone())).collect())
    }
}

const AI_OUT: &str = r#"{"heavy": true, "mem_gb": 2, "disk_gb": 1, "net": false, "est_minutes": 3, "reason": "contract"}"#;

pub fn ai_command() -> aq_sys::ai::AiCommand {
    aq_sys::ai::AiCommand {
        argv: vec!["sh".into(), "-c".into(), format!("cat >/dev/null; echo '{AI_OUT}'")],
        timeout: Duration::from_secs(10),
    }
}

pub struct FakeAi;
impl AiCli for FakeAi {
    fn ask(&self, _prompt: &str) -> Result<String, String> {
        Ok(AI_OUT.into())
    }
}

fn sh_git(dir: &Path, args: &[&str]) {
    let o = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

pub struct RealGitWorld {
    _d: tempfile::TempDir,
    root: PathBuf,
    git: aq_sys::git::RealGit,
}
impl RealGitWorld {
    pub fn new() -> Self {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir_all(root.join("Modules")).unwrap();
        std::fs::write(root.join("Modules/x.swift"), "x").unwrap();
        sh_git(&root, &["init", "-q"]);
        sh_git(&root, &["add", "."]);
        sh_git(&root, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "i"]);
        RealGitWorld { _d: d, root, git: aq_sys::git::RealGit { budget: Duration::from_secs(30) } }
    }
}
impl GitWorld for RealGitWorld {
    fn git(&self) -> &dyn Git {
        &self.git
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn dirty(&self) {
        std::fs::write(self.root.join("Modules/x.swift"), "changed").unwrap();
    }
}

pub struct FakeGit {
    root: PathBuf,
    dirty: RefCell<bool>,
}
impl Git for FakeGit {
    fn locate(&self, cwd: &str) -> Result<Option<RepoInfo>, String> {
        let Ok(rel) = Path::new(cwd).strip_prefix(&self.root) else { return Ok(None) };
        let sha = "a".repeat(40);
        let fingerprint = if *self.dirty.borrow() { format!("{sha}+dirty.0123456789ab") } else { sha };
        Ok(Some(RepoInfo {
            toplevel: self.root.to_string_lossy().into_owned(),
            common_dir: self.root.join(".git").to_string_lossy().into_owned(),
            rel_cwd: rel.to_string_lossy().into_owned(),
            fingerprint: Some(fingerprint),
        }))
    }
}

pub struct FakeGitWorld(FakeGit);
impl FakeGitWorld {
    pub fn new() -> Self {
        FakeGitWorld(FakeGit { root: PathBuf::from("/fake/repo"), dirty: RefCell::new(false) })
    }
}
impl GitWorld for FakeGitWorld {
    fn git(&self) -> &dyn Git {
        &self.0
    }
    fn root(&self) -> &Path {
        &self.0.root
    }
    fn dirty(&self) {
        *self.0.dirty.borrow_mut() = true;
    }
}
