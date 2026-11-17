//! 读出「检查」需要的快照：进行中的执行、未归还的借用、设备行、账号行。

use aq_core::check::Snapshot;
use aq_core::model::{ExecView, HoldView, ProcId};
use rusqlite::Connection;

use crate::{accounts, active_execs, active_holds, devices, Result};

pub fn snapshot(c: &Connection, now_ms: i64) -> Result<Snapshot> {
    let execs = active_execs(c)?
        .into_iter()
        .map(|e| ExecView {
            pgid: match (e.pgid, e.pgid_started_at) {
                (Some(pid), Some(started_at)) => Some(ProcId { pid, started_at }),
                _ => None,
            },
            id: e.id,
            status: e.status,
            submitted_at: e.submitted_at,
            started_at: e.started_at,
            res: e.res,
            timeout_s: e.timeout_s,
        })
        .collect();
    let holds = active_holds(c)?
        .into_iter()
        .map(|h| HoldView {
            id: h.id,
            status: h.status,
            requested_at: h.requested_at,
            acquired_at: h.acquired_at,
            request: h.request,
            owner: h.owner,
        })
        .collect();
    Ok(Snapshot { now_ms, execs, holds, devices: devices(c)?, accounts: accounts(c)? })
}
