//! 锁目录收尾（CASE-STORE-7）：锁文件跟着账本走。执行、借用的锁在账本里已是终态或已被保留策略删掉就删；
//! AI 评估的锁和失败标记 1 小时没动就删（AI 最多等 ai_timeout + 10 秒）。
//! 只删此刻锁得上的：锁不上说明有人正拿着（例如提交方已拿锁、事务还没提交，账本里暂时查不到这一行）。

use std::time::{Duration, SystemTime};

use aq_core::ports::Locks;
use aq_store::Store;

use super::ctx::Ctx;
use super::error::AppResult;

const AI_STALE: Duration = Duration::from_secs(3600);

pub fn locks(ctx: &Ctx, store: &Store) -> AppResult<()> {
    let Ok(rd) = std::fs::read_dir(&ctx.dirs.locks) else { return Ok(()) };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale = match name.strip_suffix(".lock") {
            Some(id) if id.starts_with("ex_") => aq_store::get_exec(store.conn(), id)?.is_none_or(|e| e.status.is_terminal()),
            Some(id) if id.starts_with("hd_") => aq_store::get_hold(store.conn(), id)?.is_none_or(|h| h.status.is_settled()),
            _ if name.starts_with("ai-") => older_than(&entry, AI_STALE),
            _ => false,
        };
        if !stale {
            continue;
        }
        let guard = match name.strip_suffix(".lock") {
            Some(n) => ctx.locks.try_lock(n).ok().flatten().map(Some),
            None => Some(None), // .failed 只是标记，不是锁
        };
        if guard.is_some() {
            let _ = std::fs::remove_file(entry.path()); // 删不掉留到下次，不影响业务
        }
    }
    Ok(())
}

fn older_than(entry: &std::fs::DirEntry, age: Duration) -> bool {
    let modified = entry.metadata().and_then(|m| m.modified());
    modified.ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|d| d > age)
}
