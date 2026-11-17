//! 进程探测（macOS libproc）：启动时间、进程组成员、常驻内存、父进程链。
//! 判断进程是否还在时同时核对进程号和启动时间。

use aq_core::model::ProcId;
use aq_core::ports::Procs;

const PROC_PGRP_ONLY: u32 = 2;
const PROC_PPID_ONLY: u32 = 6;

pub struct RealProcs;

fn bsdinfo(pid: i64) -> Option<libc::proc_bsdinfo> {
    let pid = i32::try_from(pid).ok()?;
    if pid <= 0 {
        return None;
    }
    let mut info: libc::proc_bsdinfo = unsafe_zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: info 是足够大的已初始化缓冲区，size 与其类型一致。
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then_some(info)
}

fn unsafe_zeroed<T: Copy>() -> T {
    // SAFETY: 只用于 libc 的纯数据结构体（全零是合法值）。
    unsafe { std::mem::zeroed() }
}

fn start_ms(i: &libc::proc_bsdinfo) -> i64 {
    (i.pbi_start_tvsec as i64) * 1000 + (i.pbi_start_tvusec as i64) / 1000
}

fn cstr(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// argv[0] 的文件名（KERN_PROCARGS2）。Claude Code 会把进程名改成版本号（如 2.1.322），
/// 可执行文件也按版本命名，只有 argv[0] 还是 "claude"，所以按它识别借用者。
fn argv0_name(pid: i64) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: 先查所需大小，oldp 为空。
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &raw mut size, std::ptr::null_mut(), 0) } != 0 || size < 8 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: buf 有 size 字节可写。
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &raw mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buf.truncate(size);
    // 布局：argc（4 字节）| 可执行路径 \0 | 填充 \0 | argv[0] \0 | …
    let rest = buf.get(4..)?;
    let exe_end = rest.iter().position(|b| *b == 0)?;
    let after = &rest[exe_end..];
    let start = after.iter().position(|b| *b != 0)?;
    let arg0 = after[start..].split(|b| *b == 0).next()?;
    let s = String::from_utf8_lossy(arg0);
    let base = s.rsplit('/').next().unwrap_or(&s).trim_start_matches('-').to_string();
    (!base.is_empty()).then_some(base)
}

fn list_pids(kind: u32, arg: i64) -> Vec<i32> {
    let Ok(arg) = u32::try_from(arg) else { return vec![] };
    let mut buf = vec![0i32; 1024];
    let bytes = (buf.len() * std::mem::size_of::<i32>()) as i32;
    // SAFETY: buf 有 bytes 字节可写。
    let n = unsafe { libc::proc_listpids(kind, arg, buf.as_mut_ptr().cast(), bytes) };
    if n <= 0 {
        return vec![];
    }
    buf.truncate(n as usize / std::mem::size_of::<i32>());
    buf.into_iter().filter(|p| *p > 0).collect()
}

/// 进程组内的所有进程号。
pub fn group_members(pgid: i64) -> Vec<i32> {
    list_pids(PROC_PGRP_ONLY, pgid)
}

/// 进程组成员加上它们的所有后代（沿父进程号找）。xcodebuild 的 SWBBuildService 会另开进程组，
/// 编译进程都在它下面，只看进程组会一核都测不到（CASE-EST-10 实测）。
pub fn tree_members(pgid: i64) -> Vec<i32> {
    let mut seen: std::collections::HashSet<i32> = group_members(pgid).into_iter().collect();
    let mut todo: Vec<i32> = seen.iter().copied().collect();
    while let Some(p) = todo.pop() {
        for c in list_pids(PROC_PPID_ONLY, i64::from(p)) {
            if seen.insert(c) {
                todo.push(c);
            }
        }
    }
    seen.into_iter().collect()
}

/// 单个进程的常驻内存（字节）。
pub fn rss_bytes(pid: i32) -> u64 {
    let mut ti: libc::proc_taskinfo = unsafe_zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    // SAFETY: ti 是足够大的已初始化缓冲区。
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut ti).cast(), size) };
    if n == size {
        ti.pti_resident_size
    } else {
        0
    }
}

#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}

/// proc_taskinfo 的 CPU 时间是 mach 时钟单位（Apple Silicon 上 1 单位 = 125/3 纳秒），换算成纳秒。
fn mach_to_ns(t: u64) -> u64 {
    static TB: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    let (n, d) = *TB.get_or_init(|| {
        let mut tb = Timebase { numer: 1, denom: 1 };
        // SAFETY: tb 是有效的可写结构体。
        let rc = unsafe { mach_timebase_info(&raw mut tb) };
        if rc == 0 && tb.denom != 0 {
            (u64::from(tb.numer), u64::from(tb.denom))
        } else {
            (1, 1)
        }
    });
    (u128::from(t) * u128::from(n) / u128::from(d)) as u64
}

/// 单个进程的累计 CPU 时间（用户态 + 内核态，纳秒）。
pub fn cpu_ns(pid: i32) -> Option<u64> {
    let mut ti: libc::proc_taskinfo = unsafe_zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    // SAFETY: ti 是足够大的已初始化缓冲区。
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut ti).cast(), size) };
    (n == size).then(|| mach_to_ns(ti.pti_total_user.saturating_add(ti.pti_total_system)))
}

impl Procs for RealProcs {
    fn start_time(&self, pid: i64) -> Option<i64> {
        bsdinfo(pid).map(|i| start_ms(&i))
    }

    fn group_alive(&self, leader: ProcId) -> bool {
        match self.start_time(leader.pid) {
            Some(t) if t == leader.started_at => true,
            Some(_) => false, // 进程号已被别的进程复用
            None => !group_members(leader.pid).is_empty(),
        }
    }

    fn group_mem_mb(&self, pgid: i64) -> i64 {
        // 按进程树合计：xcodebuild 的编译进程在另开进程组的 SWBBuildService 下面（CASE-EST-10）
        let total: u64 = tree_members(pgid).into_iter().map(rss_bytes).sum();
        (total / (1024 * 1024)) as i64
    }

    fn group_cpu_ns(&self, pgid: i64) -> Vec<(i64, u64)> {
        tree_members(pgid).into_iter().filter_map(|p| cpu_ns(p).map(|ns| (i64::from(p), ns))).collect()
    }

    fn ancestors(&self, pid: i64) -> Vec<(i64, String, i64)> {
        let mut out = vec![];
        let mut cur = pid;
        for _ in 0..64 {
            let Some(i) = bsdinfo(cur) else { break };
            let kernel = if i.pbi_name[0] != 0 { cstr(&i.pbi_name) } else { cstr(&i.pbi_comm) };
            let name = argv0_name(cur).unwrap_or(kernel);
            out.push((cur, name, start_ms(&i)));
            if i.pbi_ppid <= 1 {
                break;
            }
            cur = i.pbi_ppid as i64;
        }
        out
    }
}

/// 本进程的启动时间。
pub fn self_start_ms() -> i64 {
    RealProcs.start_time(std::process::id() as i64).unwrap_or(0)
}
