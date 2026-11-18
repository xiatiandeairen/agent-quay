//! runner 文件锁（flock，进程退出时系统自动释放）。拿得到锁 = runner 已不在。
//! 锁的文件描述符可以通过继承交给新拉起的 runner（提交者先拿锁再拉 runner，避免「刚拉起还没拿锁」被误判）。

use std::fs::{File, OpenOptions, TryLockError};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};

use aq_core::ports::Locks;

pub struct LockDir {
    pub dir: PathBuf,
}

impl LockDir {
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.lock"))
    }
}

pub fn try_lock_path(p: &Path) -> Result<Option<File>, String> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(p)
        .map_err(|e| format!("cannot open lock file {}: {e}", p.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(format!("cannot lock {}: {e}", p.display())),
    }
}

/// 阻塞等锁，最多 `secs` 秒（同签名的 AI 评估只做一次：后来者等先来者做完）。
pub fn lock_wait(p: &Path, secs: u64) -> Result<Option<File>, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(f) = try_lock_path(p)? {
            return Ok(Some(f));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

impl Locks for LockDir {
    type Guard = File;
    fn try_lock(&self, name: &str) -> Result<Option<File>, String> {
        try_lock_path(&self.path(name))
    }
}

/// 让 fd 能被子进程继承（清掉 FD_CLOEXEC）。
pub fn inheritable(f: &File) -> Result<RawFd, String> {
    let fd = f.as_raw_fd();
    set_cloexec(fd, false)?;
    Ok(fd)
}

pub fn set_cloexec(fd: RawFd, on: bool) -> Result<(), String> {
    // SAFETY: fcntl 只读写 fd 标志位。
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(format!("fcntl F_GETFD failed: fd {fd}"));
    }
    let new = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
    // SAFETY: 同上。
    if unsafe { libc::fcntl(fd, libc::F_SETFD, new) } < 0 {
        return Err(format!("fcntl F_SETFD failed: fd {fd}"));
    }
    Ok(())
}

/// runner 接手继承来的锁 fd，并设 CLOEXEC，保证它执行的命令不会带走锁。
///
/// # Safety
/// 调用方保证 fd 是继承来的、仍持有锁的锁文件描述符，且没有别的 File 拥有它。
pub unsafe fn adopt(fd: RawFd) -> Result<File, String> {
    set_cloexec(fd, true)?;
    // SAFETY: 由调用方保证 fd 有效且独占。
    Ok(unsafe { File::from_raw_fd(fd) })
}
