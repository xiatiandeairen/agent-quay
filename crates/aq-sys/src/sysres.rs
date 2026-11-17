//! 系统资源采样：可用内存（sysinfo）、磁盘（statvfs）、CPU（1 分钟负载 / 核数）。
//! 验收测试用 AQ_SYS_OVERRIDE 指向一个 JSON 文件覆盖部分字段（模拟别的程序吃掉内存、磁盘见底）。

use std::ffi::CString;
use std::path::PathBuf;

use aq_core::admit::SysSample;
use aq_core::ports::SysRes;
use serde::Deserialize;

pub struct RealSys {
    /// 统计磁盘用的目录（STATE 目录所在的卷）。
    pub disk_path: PathBuf,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Override {
    mem_total_mb: Option<i64>,
    mem_avail_mb: Option<i64>,
    disk_total_mb: Option<i64>,
    disk_free_mb: Option<i64>,
    cpu_pct: Option<i64>,
}

fn disk(path: &std::path::Path) -> (i64, i64) {
    let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else { return (0, 0) };
    let mut s: libc::statvfs = unsafe_zeroed();
    // SAFETY: c 是有效 C 字符串，s 是可写缓冲区。
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut s) } != 0 {
        return (0, 0);
    }
    let mb = |blocks: u64| (blocks as u128 * s.f_frsize as u128 / (1024 * 1024)) as i64;
    (mb(s.f_blocks as u64), mb(s.f_bavail as u64))
}

fn unsafe_zeroed<T: Copy>() -> T {
    // SAFETY: 只用于 libc 的纯数据结构体。
    unsafe { std::mem::zeroed() }
}

fn cpu_pct() -> i64 {
    let mut load = [0f64; 3];
    // SAFETY: load 有 3 个元素可写。
    let n = unsafe { libc::getloadavg(load.as_mut_ptr(), 3) };
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    if n < 1 {
        return 0;
    }
    ((load[0] / cores) * 100.0).round().clamp(0.0, 100.0) as i64
}

fn apply_override(s: &mut SysSample) {
    let Some(path) = std::env::var_os("AQ_SYS_OVERRIDE") else { return };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Ok(o) = serde_json::from_str::<Override>(&text) else { return };
    s.mem_total_mb = o.mem_total_mb.unwrap_or(s.mem_total_mb);
    s.mem_avail_mb = o.mem_avail_mb.unwrap_or(s.mem_avail_mb);
    s.disk_total_mb = o.disk_total_mb.unwrap_or(s.disk_total_mb);
    s.disk_free_mb = o.disk_free_mb.unwrap_or(s.disk_free_mb);
    s.cpu_pct = o.cpu_pct.unwrap_or(s.cpu_pct);
}

impl SysRes for RealSys {
    fn sample(&self) -> SysSample {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let (disk_total_mb, disk_free_mb) = disk(&self.disk_path);
        let mut s = SysSample {
            mem_total_mb: (sys.total_memory() / (1024 * 1024)) as i64,
            mem_avail_mb: (sys.available_memory() / (1024 * 1024)) as i64,
            disk_total_mb,
            disk_free_mb,
            cpu_pct: cpu_pct(),
        };
        apply_override(&mut s);
        s
    }
}
