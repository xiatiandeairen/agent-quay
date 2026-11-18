//! 运行上下文：目录、配置、时间阈值、账本连接、外部能力。每条 aq 命令开头建一个。

use std::path::PathBuf;
use std::time::Duration;

use aq_core::capacity::Limits;
use aq_core::ports::Clock;
use aq_store::Store;
use aq_sys::clock::RealClock;
use aq_sys::dirs::Dirs;
use aq_sys::git::RealGit;
use aq_sys::lock::LockDir;
use aq_sys::procs::RealProcs;
use aq_sys::sim::SimCtl;
use aq_sys::sysres::RealSys;

use super::error::AppError;
use crate::config::Config;

/// 时间与容量阈值。正式值是文档里的数；验收测试用 AQ_T_* 环境变量缩短（见 docs/design.md 的测试注入表）。
#[derive(Debug, Clone)]
pub struct Tuning {
    pub poll: Duration,
    pub kill_grace: Duration,
    pub ai_timeout: Duration,
    pub simgc_every_ms: i64,
    pub log_cap_bytes: u64,
    pub log_total_bytes: u64,
    pub keep: i64,
    pub default_wait_s: u64,
    /// 运行中纠正重任务的观察窗口（文档：10 秒）。
    pub heavy_window_ms: i64,
    /// 只给验收测试：首次见到的命令先同步等 AI 评估再放行（旧行为），让测试能用假 AI 的输出设定首次资源。正式运行为假。
    pub ai_blocking: bool,
}

fn env_i64(k: &str) -> Option<i64> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

impl Tuning {
    pub fn from_env() -> (Tuning, Limits) {
        let d = Limits::default();
        let lim = Limits {
            idle_reclaim_ms: env_i64("AQ_T_IDLE_RECLAIM_S").map_or(d.idle_reclaim_ms, |s| s * 1000),
            timeout_min_s: env_i64("AQ_T_TIMEOUT_MIN_S").unwrap_or(d.timeout_min_s),
            timeout_max_s: env_i64("AQ_T_TIMEOUT_MAX_S").unwrap_or(d.timeout_max_s),
        };
        let t = Tuning {
            poll: Duration::from_millis(env_i64("AQ_T_POLL_MS").unwrap_or(1000) as u64),
            kill_grace: Duration::from_millis(env_i64("AQ_T_KILL_GRACE_MS").unwrap_or(10_000) as u64),
            ai_timeout: Duration::from_secs(env_i64("AQ_T_AI_TIMEOUT_S").unwrap_or(60) as u64),
            simgc_every_ms: env_i64("AQ_T_SIMGC_S").unwrap_or(600) * 1000,
            log_cap_bytes: env_i64("AQ_T_LOG_CAP_MB").unwrap_or(200) as u64 * 1024 * 1024,
            log_total_bytes: env_i64("AQ_T_LOG_TOTAL_MB").unwrap_or(2048) as u64 * 1024 * 1024,
            keep: env_i64("AQ_T_KEEP").unwrap_or(5000),
            default_wait_s: 100,
            heavy_window_ms: env_i64("AQ_T_HEAVY_WINDOW_S").unwrap_or(10) * 1000,
            ai_blocking: env_i64("AQ_T_AI_BLOCKING") == Some(1),
        };
        (t, lim)
    }
}

pub struct Ctx {
    pub dirs: Dirs,
    pub config: Config,
    pub tuning: Tuning,
    pub limits: Limits,
    pub clock: RealClock,
    pub procs: RealProcs,
    pub git: RealGit,
    pub sys: RealSys,
    pub locks: LockDir,
    pub exe: PathBuf,
    /// 触发命令名，写进 aq.log。
    pub by: &'static str,
    /// 本账本标识，写进虚拟设备名；对账只删带本标识的设备（CASE-HOLD-9）。
    pub ledger: String,
    /// 默认账本也认领旧格式（不带标识）的设备名（CASE-HOLD-10）。
    pub default_ledger: bool,
}

impl Ctx {
    pub fn open(by: &'static str) -> Result<(Ctx, Store), AppError> {
        let dirs = Dirs::resolve().map_err(AppError::Fatal)?;
        let config = Config::load(&dirs.config_file).map_err(AppError::Usage)?;
        dirs.ensure().map_err(AppError::Fatal)?;
        let store = Store::open(&dirs.db).map_err(AppError::from)?;
        let (tuning, limits) = Tuning::from_env();
        let exe = std::env::current_exe().map_err(|e| AppError::Fatal(format!("找不到 aq 可执行文件：{e}")))?;
        let canon = std::fs::canonicalize(&dirs.state).unwrap_or_else(|_| dirs.state.clone());
        let ledger = aq_core::devname::ledger_tag(&canon.to_string_lossy());
        let default_ledger = dirs.is_default_state();
        let ctx = Ctx {
            ledger,
            default_ledger,
            sys: RealSys::new(dirs.state.clone()),
            locks: LockDir { dir: dirs.locks.clone() },
            git: RealGit { budget: Duration::from_secs(30) },
            dirs,
            config,
            tuning,
            limits,
            clock: RealClock,
            procs: RealProcs,
            exe,
            by,
        };
        Ok((ctx, store))
    }

    /// 等待节奏：先 50ms，逐次翻倍到 poll（默认 1 秒）；不超过 deadline。
    /// 没人排队时短命令几乎不多等（CASE-LOAD-4），长任务仍按每秒一次读库。
    pub fn pace(&self, round: u32, deadline: Option<std::time::Instant>) {
        let step = Duration::from_millis(50u64.saturating_mul(1 << round.min(10))).min(self.tuning.poll);
        let step = match deadline {
            Some(d) => step.min(d.saturating_duration_since(std::time::Instant::now())),
            None => step,
        };
        std::thread::sleep(step.max(Duration::from_millis(10)));
    }

    /// 借用者：按环境变量认出已验证的 agent，再沿父进程链找到它的会话主进程 (pid, 名字, 启动时间)；
    /// 认不出或链上找不到返回 None（CASE-RECLAIM-5、CASE-RECLAIM-5）。
    pub fn agent(&self) -> Option<(i64, String, i64)> {
        use aq_core::ports::Procs;
        let v = aq_sys::agent::verified()?;
        let ppid = i64::from(std::os::unix::process::parent_id());
        self.procs.ancestors(ppid).into_iter().find(|(_, name, _)| *name == v.argv0)
    }

    /// 借用结束：删掉 .used（agent-device 据此发现已收回）和 Stop hook 的 .reminded 标记。
    pub fn clear_hold_files(&self, id: &str) {
        for f in [self.dirs.used_file(id), self.dirs.holds.join(format!("{id}.reminded"))] {
            let _ = std::fs::remove_file(f); // 文件不在就算已删
        }
    }

    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    pub fn sim(&self) -> SimCtl {
        SimCtl::from_env(&self.config.ios_sim_model)
    }

    pub fn emu(&self) -> aq_sys::emu::Emulator {
        aq_sys::emu::Emulator::from_env(&self.config.android_avd, &self.dirs.state.join("emu"))
    }

    /// 虚拟设备的后端：ios-sim → simctl，android-emu → emulator，browser → agent-browser，desktop → agent-desktop 会话名。
    /// 真机没有后端（返回 None）。
    pub fn virtual_backend(&self, t: aq_core::model::DeviceType) -> Option<Box<dyn aq_core::ports::Simulator>> {
        use aq_core::model::DeviceType;
        match t {
            DeviceType::IosSim => Some(Box::new(self.sim())),
            DeviceType::AndroidEmu => Some(Box::new(self.emu())),
            DeviceType::Browser => Some(Box::new(aq_sys::browser::AgentBrowser::from_env())),
            DeviceType::Desktop => Some(Box::new(aq_sys::desktop::AgentDesktop)),
            DeviceType::IosDevice | DeviceType::AndroidDevice => None,
        }
    }

    /// 虚拟设备名：aq-<账本标识>-<借用id>-<序号>。
    pub fn device_name(&self, hold_id: &str, seq: i64) -> String {
        aq_core::devname::name(&self.ledger, hold_id, seq)
    }

    /// 设备名属于本账本的哪组借用；别的账本或认不出的名字为 None。
    pub fn my_hold_of(&self, name: &str) -> Option<String> {
        match aq_core::devname::owner(name, &self.ledger, self.default_ledger) {
            aq_core::devname::Owner::Mine(h) => Some(h),
            aq_core::devname::Owner::Other => None,
        }
    }

    pub fn log_transition(&self, id: &str, from: &str, to: &str, reason: &str) {
        aq_sys::applog::transition(&self.dirs.aq_log, self.now(), id, from, to, reason, self.by);
    }

    pub fn log_note(&self, id: &str, msg: &str) {
        aq_sys::applog::note(&self.dirs.aq_log, self.now(), id, msg, self.by);
    }
}
