//! 进程管理：runner 脱离会话、启动闸门、进程组终止、子进程资源统计。

use std::fs::File;
use std::io::Write;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

/// 用中间进程脱离会话：`aq __detach <args>` 先 setsid，再拉起真正的进程并立刻退出，
/// 真正的进程由 launchd 接管，离开 agent 的进程树。`keep_fd` 是要交给它的锁。
pub fn detach(exe: &Path, args: &[String], keep_fd: Option<RawFd>, stderr_log: &Path) -> Result<(), String> {
    let err = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("打开 {} 失败：{e}", stderr_log.display()))?;
    let mut cmd = Command::new(exe);
    cmd.arg("__detach").args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(err);
    if let Some(fd) = keep_fd {
        cmd.env("AQ_LOCK_FD", fd.to_string());
    }
    // SAFETY: pre_exec 里只调用 async-signal-safe 的 setsid 和 close。
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            close_inherited(keep_fd);
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("拉起后台执行者失败：{e}"))?;
    child.wait().map_err(|e| format!("等待中间进程失败：{e}"))?;
    Ok(())
}

/// 关掉从 agent 继承来的文件描述符（避免管道断开时被信号带走），只保留 0–2 和要交接的锁。
fn close_inherited(keep: Option<RawFd>) {
    for fd in 3..1024 {
        if Some(fd) != keep {
            // SAFETY: close 对无效 fd 只返回 EBADF。
            unsafe { libc::close(fd) };
        }
    }
}

/// 中间进程：拉起目标并退出（不等它）。
pub fn detach_child(exe: &Path, args: &[String]) -> Result<(), String> {
    Command::new(exe).args(args).spawn().map(|_| ()).map_err(|e| format!("拉起失败：{e}"))
}

/// 启动闸门：命令进程（经 `aq __exec`）自成进程组，阻塞读管道；runner 写入 running 并提交后再放行。
pub struct Gate {
    child: Child,
    write: Option<File>,
}

/// 命令进程结束：退出状态，以及它自己（含它等待过的后代）的 CPU 秒数和最大常驻内存（MB）。
/// 用 wait4 取这一个进程的用量，不用 RUSAGE_CHILDREN：后者累计 runner 回收过的所有子进程，会把 AI CLI、git 算进来（CASE-EST-8）。
#[derive(Debug, Clone, Copy)]
pub struct Exit {
    pub status: ExitStatus,
    pub cpu_s: f64,
    pub maxrss_mb: i64,
}

pub struct Launch<'a> {
    pub exe: &'a Path,
    pub argv: &'a [String],
    pub cwd: &'a Path,
    pub log: &'a Path,
    pub env: &'a [(&'a str, String)],
}

pub fn spawn_gated(l: &Launch<'_>) -> Result<Gate, String> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds 有两个元素可写。
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("建管道失败".into());
    }
    let (r, w) = (fds[0], fds[1]);
    crate::lock::set_cloexec(w, true)?;
    // SAFETY: w 是刚创建的管道写端，由 File 独占。
    let write = unsafe { File::from_raw_fd(w) };
    let log =
        std::fs::OpenOptions::new().create(true).append(true).open(l.log).map_err(|e| format!("打开日志 {} 失败：{e}", l.log.display()))?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(l.exe);
    cmd.arg("__exec").arg(r.to_string()).arg("--").args(l.argv);
    cmd.current_dir(l.cwd).stdin(Stdio::null()).stdout(log).stderr(log2).process_group(0);
    for (k, v) in l.env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|e| format!("启动命令失败：{e}"));
    // SAFETY: r 是管道读端，子进程已继承，父进程关掉自己这份。
    unsafe { libc::close(r) };
    Ok(Gate { child: child?, write: Some(write) })
}

impl Gate {
    pub fn pid(&self) -> i64 {
        self.child.id() as i64
    }
    /// 回收命令进程；`block` 为否时没结束就返回 None。回收后不再经 `Child` 等待（会报 ECHILD）。
    pub fn reap(&mut self, block: bool) -> Result<Option<Exit>, String> {
        let pid = self.child.id() as libc::pid_t;
        let mut status = 0;
        let mut ru: libc::rusage = crate::procs_zeroed();
        let flags = if block { 0 } else { libc::WNOHANG };
        // SAFETY: status 和 ru 都是可写的本地变量。
        let r = unsafe { libc::wait4(pid, &raw mut status, flags, &raw mut ru) };
        if r == 0 {
            return Ok(None);
        }
        if r < 0 {
            return Err(format!("等待命令进程 {pid} 失败：{}", std::io::Error::last_os_error()));
        }
        let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        Ok(Some(Exit {
            status: ExitStatus::from_raw(status),
            cpu_s: secs(ru.ru_utime) + secs(ru.ru_stime),
            maxrss_mb: ru.ru_maxrss / (1024 * 1024),
        }))
    }
    /// 放行：写 1 个字节。
    pub fn open(&mut self) -> Result<(), String> {
        let mut w = self.write.take().ok_or("闸门已打开")?;
        w.write_all(b"1").map_err(|e| format!("放行失败：{e}"))
    }
}

/// `aq __exec <fd> -- argv`：等闸门放行后 exec 目标命令。闸门关闭（runner 提交前崩溃）则从不执行。
pub fn exec_after_gate(fd: RawFd, argv: &[String]) -> i32 {
    let mut b = [0u8; 1];
    // SAFETY: b 有 1 字节可写。
    let n = unsafe { libc::read(fd, b.as_mut_ptr().cast(), 1) };
    // SAFETY: 关闭继承来的读端。
    unsafe { libc::close(fd) };
    if n != 1 {
        return 0;
    }
    let Some((prog, args)) = argv.split_first() else { return 127 };
    let err = Command::new(prog).args(args).exec();
    eprintln!("aq: 无法执行 {prog}：{err}");
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        126
    } else {
        127
    }
}

pub use libc::{SIGKILL, SIGTERM};

pub fn killpg(pgid: i64, sig: i32) {
    if let Ok(p) = i32::try_from(pgid) {
        // SAFETY: killpg 只发信号。
        unsafe { libc::killpg(p, sig) };
    }
}

/// 进程组是否已没有进程。
pub fn group_empty(pgid: i64) -> bool {
    crate::procs::group_members(pgid).is_empty()
}
