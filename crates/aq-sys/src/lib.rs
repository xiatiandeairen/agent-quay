//! aq-sys：外部世界（git、进程、锁、系统资源、simctl、emulator、agent-browser、devicectl、AI CLI、时钟、目录、日志）。
//! 只依赖 aq-core；实现 aq-core::ports 里的 trait。仅支持 macOS。

#[cfg(not(target_os = "macos"))]
compile_error!("aq 只支持 macOS（进程探测用 libproc，模拟器用 simctl）");

pub mod agent;
pub mod ai;
pub mod applog;
pub mod browser;
pub mod clock;
pub mod cmd;
pub mod desktop;
pub mod device;
pub mod dirs;
pub mod emu;
pub mod git;
pub mod lock;
pub mod procs;
pub mod sim;
pub mod spawn;
pub mod sysres;

pub(crate) fn procs_zeroed<T: Copy>() -> T {
    // SAFETY: 只用于 libc 的纯数据结构体（全零是合法值）。
    unsafe { std::mem::zeroed() }
}
