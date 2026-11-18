//! 系统资源采样：可用内存（sysinfo）、磁盘（statvfs）、CPU 使用率（两次读整机 CPU 时间计数相减）。
//! CPU 不用平均负载：macOS 的负载把等磁盘的线程也算进去，机器不忙时也可能远超核数。
//! 验收测试用 AQ_SYS_OVERRIDE 指向一个 JSON 文件覆盖部分字段（模拟别的程序吃掉内存、磁盘见底）。

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aq_core::admit::SysSample;
use aq_core::ports::SysRes;
use serde::Deserialize;

pub struct RealSys {
    /// 统计磁盘用的目录（STATE 目录所在的卷）。
    pub disk_path: PathBuf,
    cpu: Mutex<Option<CpuReading>>,
}

/// 上一次读到的 CPU 时间计数，和据此算出的使用率。
#[derive(Clone, Copy)]
struct CpuReading {
    at: Instant,
    ticks: [u32; 4],
    pct: Option<i64>,
}

impl RealSys {
    pub fn new(disk_path: PathBuf) -> RealSys {
        RealSys { disk_path, cpu: Mutex::new(None) }
    }
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

/// 整机 CPU 时间计数（用户、系统、空闲、nice，开机以来累计，单位为时钟滴答）。
fn cpu_ticks() -> Option<[u32; 4]> {
    static HOST: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: mach_host_self 只返回本机 host 端口；只取一次，避免反复取而累积端口引用。
    #[allow(deprecated)]
    let host = *HOST.get_or_init(|| unsafe { libc::mach_host_self() });
    let mut info: libc::host_cpu_load_info = unsafe_zeroed();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: info 是可写的 host_cpu_load_info，count 是它按 integer_t 计的长度。
    let rc = unsafe { libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, (&raw mut info).cast(), &raw mut count) };
    (rc == libc::KERN_SUCCESS).then_some(info.cpu_ticks)
}

/// 两次计数之间非空闲时间的占比（0–100）。计数是 u32，按回绕相减。
pub fn busy_pct(before: [u32; 4], after: [u32; 4]) -> i64 {
    let d: Vec<u64> = before.iter().zip(after).map(|(b, a)| u64::from(a.wrapping_sub(*b))).collect();
    let total: u64 = d.iter().sum();
    if total == 0 {
        return 0;
    }
    let idle = d[libc::CPU_STATE_IDLE as usize];
    (((total - idle) as f64 / total as f64) * 100.0).round().clamp(0.0, 100.0) as i64
}

/// 距上次读数不到这么久就沿用上次的结果；没有上次读数时先等这么久再读第二次。
const CPU_MIN_GAP: Duration = Duration::from_millis(100);
const CPU_FIRST_GAP: Duration = Duration::from_millis(200);

fn override_cpu() -> Option<i64> {
    let text = std::fs::read_to_string(std::env::var_os("AQ_SYS_OVERRIDE")?).ok()?;
    serde_json::from_str::<Override>(&text).ok()?.cpu_pct
}

fn apply_override(s: &mut SysSample) {
    let Some(path) = std::env::var_os("AQ_SYS_OVERRIDE") else { return };
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Ok(o) = serde_json::from_str::<Override>(&text) else { return };
    s.mem_total_mb = o.mem_total_mb.unwrap_or(s.mem_total_mb);
    s.mem_avail_mb = o.mem_avail_mb.unwrap_or(s.mem_avail_mb);
    s.disk_total_mb = o.disk_total_mb.unwrap_or(s.disk_total_mb);
    s.disk_free_mb = o.disk_free_mb.unwrap_or(s.disk_free_mb);
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
        };
        apply_override(&mut s);
        s
    }

    /// 和上次读数相减；同一进程里第一次读要等 0.2 秒取第二个读数（status --top 之后每帧直接用上一帧）。
    fn cpu_pct(&self) -> i64 {
        if let Some(v) = override_cpu() {
            return v;
        }
        let Ok(mut last) = self.cpu.lock() else { return 0 };
        let Some(now) = cpu_ticks() else { return 0 };
        let pct = match *last {
            Some(r) if r.at.elapsed() >= CPU_MIN_GAP => busy_pct(r.ticks, now),
            Some(CpuReading { pct: Some(p), .. }) => return p,
            _ => {
                std::thread::sleep(CPU_FIRST_GAP);
                let Some(next) = cpu_ticks() else { return 0 };
                let p = busy_pct(now, next);
                *last = Some(CpuReading { at: Instant::now(), ticks: next, pct: Some(p) });
                return p;
            }
        };
        *last = Some(CpuReading { at: Instant::now(), ticks: now, pct: Some(pct) });
        pct
    }
}

#[cfg(test)]
mod tests {
    use super::busy_pct;

    // CASE-STATUS-1
    #[test]
    fn busy_share_of_tick_delta_with_wraparound() {
        assert_eq!(busy_pct([100, 50, 1000, 0], [130, 70, 1050, 0]), 50, "用户 30 + 系统 20，空闲 50");
        assert_eq!(busy_pct([0, 0, 0, 0], [0, 0, 100, 0]), 0);
        assert_eq!(busy_pct([5, 5, 5, 5], [5, 5, 5, 5]), 0, "没有新计数不除零");
        assert_eq!(busy_pct([u32::MAX - 9, 0, 0, 0], [10, 0, 0, 0]), 100, "计数回绕后仍按增量算");
    }
}
