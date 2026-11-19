//! aq run：定位 → 指纹与 hash → 事务里检查、复用或入队并拉起 runner → 等待。

use std::os::unix::process::CommandExt;
use std::time::{Duration, Instant};

use aq_core::ports::{Clock, Git};
use aq_core::signature::{exec_hash, profile_key, signature};
use aq_store::{NewExec, Store};
use aq_sys::lock::inheritable;
use serde_json::Value;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, spawn, view};

pub struct RunArgs {
    pub fresh: bool,
    pub wait_s: Option<u64>,
    pub argv: Vec<String>,
}

/// 任务里面又调用 aq run：直接执行、不排队，资源算在外层头上（CASE-RUN-8）。
pub fn nested(argv: &[String]) -> i32 {
    let Some((prog, args)) = argv.split_first() else { return 3 };
    let err = std::process::Command::new(prog).args(args).exec();
    eprintln!("aq: cannot execute {prog}: {err}");
    127
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::generate().to_string().to_lowercase())
}

struct Located {
    cwd: String,
    hash: Option<String>,
    fingerprint: Option<String>,
    profile_key: String,
    worktree: Option<String>,
}

fn locate(ctx: &Ctx, a: &RunArgs) -> AppResult<Located> {
    let cwd = std::env::current_dir().map_err(|e| AppError::Fatal(format!("cannot read the current directory: {e}")))?;
    let cwd = cwd.to_string_lossy().into_owned();
    let repo = ctx.git.locate(&cwd).ok().flatten();
    let sig = signature(&a.argv);
    let (repo_key, fp, rel) = match &repo {
        Some(r) => (r.common_dir.clone(), r.fingerprint.clone(), r.rel_cwd.clone()),
        None => (cwd.clone(), None, String::new()),
    };
    let hash = match (&fp, a.fresh) {
        (Some(f), false) => Some(exec_hash(f, &rel, &a.argv)),
        _ => None,
    };
    let worktree = repo.map(|r| r.toplevel);
    Ok(Located { profile_key: profile_key(&repo_key, &sig), cwd, hash, fingerprint: fp, worktree })
}

/// 提交：返回 (执行 id, 是否复用)。先拿锁再提交，提交后再拉 runner：
/// 检查永远不会把「刚拉起还没拿锁」的 runner 当成已消失；提交后、拉起前被杀，锁随进程释放，下次检查会重拉。
fn submit(ctx: &Ctx, store: &mut Store, a: &RunArgs, l: &Located) -> AppResult<(String, bool)> {
    let (id, lock) = store.tx(|c| {
        check::check_submitting(ctx, c, None, &a.argv)?;
        if let Some(h) = &l.hash {
            if let Some(id) = aq_store::find_inflight(c, h)? {
                return Ok::<_, AppError>((id, None));
            }
        }
        let id = new_id("ex");
        let n = NewExec {
            id: &id,
            hash: l.hash.as_deref(),
            cwd: &l.cwd,
            argv: &a.argv,
            profile_key: &l.profile_key,
            fingerprint: l.fingerprint.as_deref(),
            worktree: l.worktree.as_deref(),
            submitted_at: ctx.clock.now_ms(),
        };
        aq_store::insert_exec(c, &n)?;
        let lock = aq_sys::lock::try_lock_path(&ctx.dirs.lock_file(&id)).map_err(AppError::Fatal)?;
        let lock = lock.ok_or_else(|| AppError::Fatal(format!("lock of the new execution is taken: {id}")))?;
        Ok((id, Some(lock)))
    })?;
    let Some(lock) = lock else { return Ok((id, true)) };
    ctx.log_transition(&id, "", "queued", "submitted");
    let fd = inheritable(&lock).map_err(AppError::Fatal)?;
    spawn::runner(ctx, &id, fd)?;
    Ok((id, false))
}

pub fn run(ctx: &Ctx, store: &mut Store, a: &RunArgs) -> AppResult<(Value, i32)> {
    if a.argv.is_empty() {
        return Err(AppError::Usage("missing command: write aq run -- <command> [args...]".into()));
    }
    let l = locate(ctx, a)?;
    let (id, reused) = submit(ctx, store, a, &l)?;
    let wait = Duration::from_secs(a.wait_s.unwrap_or(ctx.tuning.default_wait_s));
    let (v, code) = wait_exec(ctx, store, &id, reused, Instant::now() + wait)?;
    Ok((v, code))
}

/// 每隔 poll 读一次库，直到结束或到期限。
pub fn wait_exec(ctx: &Ctx, store: &Store, id: &str, reused: bool, deadline: Instant) -> AppResult<(Value, i32)> {
    let mut round = 0;
    loop {
        let e = aq_store::get_exec(store.conn(), id)?.ok_or_else(|| AppError::Usage(format!("no such execution: {id}")))?;
        if e.status.is_terminal() || Instant::now() >= deadline {
            let soonest = super::status::soonest_free_s(ctx, store)?;
            return Ok((view::exec_json(ctx, &e, Some(reused), soonest), e.status.exit_code()));
        }
        ctx.pace(round, Some(deadline));
        round += 1;
    }
}
