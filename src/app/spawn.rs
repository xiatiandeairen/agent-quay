//! 拉起后台进程（runner、模拟器清理）：都经 `aq __detach` 脱离 agent 会话。

use std::os::fd::RawFd;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};

/// 拉起 `aq __runner <id>`，把已拿住的锁交给它。
pub fn runner(ctx: &Ctx, id: &str, lock_fd: RawFd) -> AppResult<()> {
    detach(ctx, &["__runner".into(), id.into()], lock_fd)
}

pub fn detach(ctx: &Ctx, args: &[String], lock_fd: RawFd) -> AppResult<()> {
    let err_log = ctx.dirs.state.join("runner.err");
    aq_sys::spawn::detach(&ctx.exe, args, Some(lock_fd), &err_log).map_err(AppError::Fatal)
}
