//! 外部能力的 trait（8 个 port）。aq-sys 提供真件，测试提供假件；契约用例见 tests/contracts/。

use crate::admit::SysSample;
use crate::devices::Seen;
use crate::model::{DeviceType, ProcId};

/// 时钟：真实时间（写 *_at）和不含睡眠的单调时间（判断超时，见「三处补充约定」）。
pub trait Clock {
    fn now_ms(&self) -> i64;
    fn uptime_ms(&self) -> i64;
}

/// 一个工作区的定位与内容指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// worktree 根目录。
    pub toplevel: String,
    /// git common dir：同一仓库的所有 worktree 相同，用作评估缓存的仓库键。
    pub common_dir: String,
    /// 执行目录相对 worktree 根的路径（根目录为空串）。
    pub rel_cwd: String,
    /// 干净时 = HEAD SHA；有改动时 = SHA+dirty.<摘要>。算不出时为 None。
    pub fingerprint: Option<String>,
}

pub trait Git {
    /// 不在 git 仓库里返回 Ok(None)。只读，不拿 index.lock。
    fn locate(&self, cwd: &str) -> Result<Option<RepoInfo>, String>;
}

pub trait Procs {
    /// 进程启动时间（Unix 毫秒）；进程不存在返回 None。
    fn start_time(&self, pid: i64) -> Option<i64>;
    /// 进程号和启动时间都对得上才算还在。
    fn alive(&self, p: ProcId) -> bool {
        self.start_time(p.pid) == Some(p.started_at)
    }
    /// 进程组是否还有进程：组长在且启动时间对得上，或组长已退出但组里还有进程。
    fn group_alive(&self, leader: ProcId) -> bool;
    /// 进程组内所有进程的常驻内存之和（MB）。
    fn group_mem_mb(&self, pgid: i64) -> i64;
    /// 进程组成员及其所有后代 (pid, 累计 CPU 纳秒)，供运行中纠正重任务（cpu_watch）。测不到时为空。
    fn group_cpu_ns(&self, _pgid: i64) -> Vec<(i64, u64)> {
        Vec::new()
    }
    /// 从 pid 开始沿父进程链向上：(pid, 进程名, 启动时间)。
    fn ancestors(&self, pid: i64) -> Vec<(i64, String, i64)>;
}

pub trait SysRes {
    fn sample(&self) -> SysSample;
    /// 整机 CPU 使用率（0–100）。
    fn cpu_pct(&self) -> i64;
}

/// 模拟器：按名字新建并启动、关机删除、列出 aq- 开头的。
pub trait Simulator {
    fn create(&self, name: &str) -> Result<String, String>;
    fn delete(&self, udid: &str) -> Result<(), String>;
    fn list_aq(&self) -> Result<Vec<(String, String)>, String>;
}

/// 真机探测：列出本机此刻看得到的某类真机及其可用性。只读；工具缺失、超时、输出看不懂都返回 Err（原因给人看）。
pub trait RealDevices {
    fn scan(&self, t: DeviceType) -> Result<Vec<Seen>, String>;
}

pub trait AiCli {
    /// 提示词从标准输入传入；超时或失败返回 Err。
    fn ask(&self, prompt: &str) -> Result<String, String>;
}

/// 文件锁：拿得到 = 持有者已不在。
pub trait Locks {
    type Guard;
    fn try_lock(&self, name: &str) -> Result<Option<Self::Guard>, String>;
}
