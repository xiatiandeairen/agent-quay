//! aq release 与 aq cancel：接受即退出码 0；id 不存在为 rejected usage（3）。

use aq_core::model::{ExecStatus, HoldStatus};
use aq_store::Store;
use serde_json::Value;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};
use super::{check, view};

pub fn release(ctx: &Ctx, store: &mut Store, id: &str) -> AppResult<(Value, i32)> {
    let (h, changed, from) = store.tx(|c| {
        check::check_in(ctx, c, None)?;
        let h = aq_store::get_hold(c, id)?.ok_or_else(|| AppError::Usage(format!("借用不存在：{id}")))?;
        let changed = aq_store::end_hold(c, id, HoldStatus::Released, ctx.now())?;
        let h2 = aq_store::get_hold(c, id)?.unwrap_or(h.clone());
        Ok::<_, AppError>((h2, changed, h.status))
    })?;
    if changed {
        ctx.clear_hold_files(id);
        ctx.log_transition(id, from.as_str(), "released", "aq release");
        if h.request.devices.iter().any(|d| d.is_virtual()) {
            super::simgc::kick(ctx)?;
        }
    }
    Ok((view::hold_json(&h, &[]), 0))
}

pub fn cancel(ctx: &Ctx, store: &mut Store, id: &str) -> AppResult<(Value, i32)> {
    let e = store.tx(|c| {
        check::check_in(ctx, c, None)?;
        let e = aq_store::get_exec(c, id)?.ok_or_else(|| AppError::Usage(format!("执行不存在：{id}")))?;
        match e.status {
            ExecStatus::Queued if aq_store::cancel_queued(c, id, ctx.now())? => {
                ctx.log_transition(id, "queued", "cancelled", "aq cancel");
            }
            ExecStatus::Running => {
                aq_store::request_cancel(c, id)?;
                ctx.log_note(id, "收到撤销请求，等 runner 终止进程组");
            }
            _ => {}
        }
        Ok::<_, AppError>(aq_store::get_exec(c, id)?.unwrap_or(e))
    })?;
    Ok((view::exec_json(ctx, &e, false, None), 0))
}
