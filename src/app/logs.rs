//! 日志总量：超过上限（2G）时删最旧的已结束任务日志；执行记录保留。

use aq_store::Store;

use super::ctx::Ctx;
use super::error::AppResult;

pub fn enforce_total(ctx: &Ctx, store: &Store) -> AppResult<()> {
    let Ok(rd) = std::fs::read_dir(&ctx.dirs.logs) else { return Ok(()) };
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= ctx.tuning.log_total_bytes {
        return Ok(());
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= ctx.tuning.log_total_bytes {
            break;
        }
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let finished = aq_store::get_exec(store.conn(), &id)?.is_none_or(|e| e.status.is_terminal());
        if finished && std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
    Ok(())
}
