#![allow(dead_code)] // 各功能测试文件只用到其中一部分辅助函数
//! 验收测试环境：每个测试一个独立的 XDG 目录、假 simctl、假 AI CLI、可改的系统资源覆盖文件。
//! 通过环境变量注入假件，按 aq 命令黑盒测试。

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const AQ: &str = env!("CARGO_BIN_EXE_aq");

pub fn support_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support")
}

pub struct Env {
    pub dir: tempfile::TempDir,
    pub extra: std::cell::RefCell<Vec<(String, String)>>,
}

pub struct Res {
    pub code: i32,
    pub lines: Vec<Value>,
    pub stdout: String,
}

impl Res {
    pub fn one(&self) -> &Value {
        self.lines.first().unwrap_or_else(|| panic!("没有输出：{}", self.stdout))
    }
    pub fn status(&self) -> &str {
        self.one()["status"].as_str().unwrap_or("")
    }
    pub fn id(&self) -> String {
        self.one()["id"].as_str().unwrap_or_else(|| panic!("没有 id：{}", self.stdout)).to_string()
    }
}

pub const DEFAULT_CONFIG: &str = r#"heavy_slots = 2
mem = "28G"
disk_floor = "50G"
net_slots = 3
[ios-sim]
mem = "3G"
[android-emu]
avd = "test_avd"
mem = "4G"
[ios-device]
ids = ["phone-1"]
[android-device]
ids = ["android-1"]
"#;

impl Env {
    pub fn new() -> Env {
        Self::with_config(DEFAULT_CONFIG)
    }

    pub fn with_config(cfg: &str) -> Env {
        let dir = tempfile::Builder::new().prefix("aqt").tempdir().expect("tempdir");
        let e = Env { dir, extra: Default::default() };
        for d in ["cfg/aq", "st", "sim", "emu/running", "browser/sessions", "ai", "work", "dev"] {
            std::fs::create_dir_all(e.path(d)).expect("mkdir");
        }
        let ai = support_dir().join("fake-ai");
        e.write_config(&format!("ai_cli = \"{}\"\n{cfg}", ai.display()));
        e.set_sys(30_000, 500_000);
        e
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    pub fn work(&self) -> PathBuf {
        self.path("work")
    }
    pub fn state(&self) -> PathBuf {
        self.path("st/aq")
    }

    /// 这个环境的账本标识（与 aq 一样取状态目录真实路径的哈希）。
    pub fn ledger(&self) -> String {
        // aq 第一次运行前状态目录还不存在：取已存在的上一级的真实路径再接上 aq
        let p = std::fs::canonicalize(self.path("st")).map(|d| d.join("aq")).unwrap_or_else(|_| self.state());
        aq_core::devname::ledger_tag(&p.to_string_lossy())
    }

    /// 这个环境里 aq 给虚拟设备起的名字。
    pub fn dev_name(&self, hold_id: &str, seq: i64) -> String {
        aq_core::devname::name(&self.ledger(), hold_id, seq)
    }

    pub fn write_config(&self, text: &str) {
        std::fs::write(self.path("cfg/aq/config.toml"), text).expect("config");
    }

    /// 覆盖系统实测值（可用内存、剩余磁盘，单位 MB）。
    pub fn set_sys(&self, mem_avail_mb: i64, disk_free_mb: i64) {
        let v = serde_json::json!({"mem_total_mb": 36_864, "mem_avail_mb": mem_avail_mb, "disk_total_mb": 1_000_000, "disk_free_mb": disk_free_mb, "cpu_pct": 20});
        std::fs::write(self.path("sys.json"), v.to_string()).expect("sys");
    }

    /// 在 set_sys 的基础读数上再覆盖别的字段（内存压力、换出、设备内存）。
    pub fn set_sys_with(&self, extra: serde_json::Value) {
        let mut v = serde_json::json!({"mem_total_mb": 36_864, "mem_avail_mb": 30_000, "disk_total_mb": 1_000_000, "disk_free_mb": 500_000, "cpu_pct": 20});
        if let (Some(o), Some(x)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        std::fs::write(self.path("sys.json"), v.to_string()).expect("sys");
    }

    pub fn set_ai(&self, mode: &str) {
        std::fs::write(self.path("ai/mode"), mode).expect("ai mode");
    }
    pub fn set_ai_output(&self, json: &str) {
        std::fs::write(self.path("ai/ok.json"), json).expect("ai out");
    }
    pub fn ai_calls(&self) -> usize {
        std::fs::read_to_string(self.path("ai/calls")).map(|s| s.lines().count()).unwrap_or(0)
    }

    /// 用指向 /bin/bash 的符号链接冒充一个进程（argv[0] 为 name），带上 aq 的测试环境和 envs，执行 inner。
    /// 不能复制 /bin/sh：macOS 拒绝运行复制出来的系统程序，还会卡住同时启动的其他进程。
    pub fn as_process(&self, name: &str, envs: &[(&str, &str)], inner: &str) -> std::process::Child {
        let dir = self.path(&format!("as-{name}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let exe = dir.join(name);
        if !exe.exists() {
            std::os::unix::fs::symlink("/bin/bash", &exe).expect("symlink");
        }
        let base = self.command(&[]);
        let mut cmd = Command::new(&exe);
        cmd.arg("-c").arg(inner).env_clear();
        for (k, v) in base.get_envs() {
            if let Some(v) = v {
                cmd.env(k, v);
            }
        }
        cmd.envs(envs.iter().copied()).current_dir(self.work()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.spawn().expect("spawn")
    }

    /// 给之后运行的 aq 加一个环境变量。
    pub fn env(&self, k: &str, v: &str) -> &Self {
        self.extra.borrow_mut().push((k.into(), v.into()));
        self
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(AQ);
        c.args(args).current_dir(self.work());
        let bin = Path::new(AQ).parent().map(|p| p.display().to_string()).unwrap_or_default();
        let path = format!("{bin}:{}:{}", support_dir().display(), std::env::var("PATH").unwrap_or_default());
        c.env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.path("cfg"))
            .env("XDG_STATE_HOME", self.path("st"))
            .env("AQ_SIMCTL", support_dir().join("fake-simctl"))
            .env("FAKE_SIM_DIR", self.path("sim"))
            .env("AQ_EMULATOR", support_dir().join("fake-emulator"))
            .env("AQ_ADB", support_dir().join("fake-adb"))
            .env("FAKE_EMU_DIR", self.path("emu"))
            .env("AQ_DEVICECTL", support_dir().join("fake-devicectl"))
            .env("FAKE_DEV_DIR", self.path("dev"))
            .env("AQ_AGENT_BROWSER", support_dir().join("fake-agent-browser"))
            .env("FAKE_BROWSER_DIR", self.path("browser"))
            .env("FAKE_AI_DIR", self.path("ai"))
            .env("AQ_SYS_OVERRIDE", self.path("sys.json"))
            .env("AQ_T_POLL_MS", "100")
            .env("AQ_T_KILL_GRACE_MS", "1000")
            .env("AQ_T_SIMGC_S", "1")
            .env("AQ_T_AI_TIMEOUT_S", "3")
            // 首次见到的命令同步等假 AI：多数用例用假 AI 的输出设定首次资源。CASE-EST-13 设为 0 验正式行为
            .env("AQ_T_AI_BLOCKING", "1")
            .env("TMPDIR", self.dir.path());
        for (k, v) in self.extra.borrow().iter() {
            c.env(k, v);
        }
        c
    }

    pub fn aq(&self, args: &[&str]) -> Res {
        self.aq_in(&self.work(), args)
    }

    pub fn aq_in(&self, cwd: &Path, args: &[&str]) -> Res {
        let mut c = self.command(args);
        c.current_dir(cwd).stdin(Stdio::null());
        let out: Output = c.output().expect("运行 aq");
        parse(out)
    }

    /// 后台跑一条 aq 命令（不等）。
    pub fn spawn(&self, args: &[&str]) -> std::process::Child {
        let mut c = self.command(args);
        c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        c.spawn().expect("spawn aq")
    }

    /// 打开账本（等到 aq 建好库，最多 10 秒）。
    pub fn db(&self) -> aq_store::Conn {
        let p = self.state().join("aq.db");
        until(10, || p.exists());
        let c = aq_store::Conn::open(&p).expect("open db");
        c.busy_timeout(Duration::from_secs(10)).expect("busy_timeout");
        c
    }

    pub fn q_i64(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    pub fn q_str(&self, sql: &str) -> Option<String> {
        self.db().query_row(sql, [], |r| r.get(0)).ok()
    }

    /// 等到 status 满足条件，最多 secs 秒。
    pub fn wait_status(&self, id: &str, secs: u64, want: &[&str]) -> Res {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let r = self.aq(&["status", id]);
            if want.contains(&r.status()) || Instant::now() > deadline {
                return r;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn sims(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.path("sim/devices")) else { return vec![] };
        rd.flatten().map(|e| std::fs::read_to_string(e.path()).unwrap_or_default().trim().to_string()).collect()
    }

    /// 活着的假 agent-browser 会话名。
    pub fn browsers(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.path("browser/sessions")) else { return vec![] };
        let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// 正在运行的假 Android 模拟器端口。
    pub fn emus(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(self.path("emu/running")) else { return vec![] };
        let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    pub fn aq_log(&self) -> String {
        std::fs::read_to_string(self.state().join("aq.log")).unwrap_or_default()
    }
}

pub fn parse(out: Output) -> Res {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines = stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    Res { code: out.status.code().unwrap_or(-1), lines, stdout }
}

/// 读后台进程写到文件里的 aq 输出；等到有一行 JSON 为止（最多 10 秒）。
pub fn parse_file(p: &Path) -> Res {
    let mut stdout = String::new();
    until(10, || {
        stdout = std::fs::read_to_string(p).unwrap_or_default();
        stdout.lines().any(|l| serde_json::from_str::<Value>(l).is_ok())
    });
    let lines = stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    Res { code: 0, lines, stdout }
}

pub fn until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    f()
}

/// 建一个 git 仓库（带一次提交）。
pub fn git_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir repo");
    let g = |args: &[&str]| {
        let s = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_GLOBAL", "/dev/null").output().expect("git");
        assert!(s.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&s.stderr));
    };
    g(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "hello\n").expect("write");
    std::fs::write(dir.join(".gitignore"), "build/\n").expect("write");
    g(&["add", "."]);
    g(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_GLOBAL", "/dev/null").output().expect("git");
    String::from_utf8_lossy(&o.stdout).into_owned()
}
