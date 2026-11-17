//! 保留：已结束执行、已结束借用各保留最近 5000 条。返回被删执行的 id（用来删日志）。

use rusqlite::{params, Connection};

use crate::Result;

const TERMINAL_EXEC: [&str; 6] = ["ok", "failed", "timeout", "interrupted", "cancelled", "rejected"];

pub fn prune(c: &Connection, keep: i64) -> Result<Vec<String>> {
    let cutoff: Option<i64> = c
        .query_row(
            "SELECT finished_at FROM executions WHERE finished_at IS NOT NULL ORDER BY finished_at DESC LIMIT 1 OFFSET ?1",
            [keep - 1],
            |r| r.get(0),
        )
        .ok();
    let mut removed = vec![];
    if let Some(cut) = cutoff {
        for st in TERMINAL_EXEC {
            let mut q = c.prepare_cached("SELECT id FROM executions WHERE status = ?1 AND finished_at < ?2")?;
            for id in q.query_map(params![st, cut], |r| r.get::<_, String>(0))? {
                removed.push(id?);
            }
            c.execute("DELETE FROM executions WHERE status = ?1 AND finished_at < ?2", params![st, cut])?;
        }
    }
    let hold_cut: Option<i64> = c
        .query_row("SELECT ended_at FROM holds WHERE ended_at IS NOT NULL ORDER BY ended_at DESC LIMIT 1 OFFSET ?1", [keep - 1], |r| {
            r.get(0)
        })
        .ok();
    if let Some(cut) = hold_cut {
        for st in ["released", "reclaimed", "rejected"] {
            c.execute("DELETE FROM holds WHERE status = ?1 AND ended_at < ?2", params![st, cut])?;
        }
    }
    Ok(removed)
}
