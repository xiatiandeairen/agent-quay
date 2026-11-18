//! executions 表的读写。每个改状态的函数都带上原状态条件，状态倒退由这里保证（DB 不拦，见 DB 用例约定）。

use aq_core::estimate::{MachineHistory, Sample};
use aq_core::model::{ExecStatus, Resources};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct ExecRow {
    pub id: String,
    pub hash: Option<String>,
    pub status: ExecStatus,
    pub cwd: String,
    pub argv: Vec<String>,
    pub profile_key: String,
    pub fingerprint: Option<String>,
    pub content_changed: Option<bool>,
    pub res: Option<Resources>,
    pub timeout_s: Option<i64>,
    pub submitted_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub pgid: Option<i64>,
    pub pgid_started_at: Option<i64>,
    pub cancel_requested: bool,
    pub exit_code: Option<i64>,
    pub peak_mem_mb: Option<i64>,
    pub cpu_avg_cores: Option<f64>,
    pub disk_delta_mb: Option<i64>,
}

const COLS: &str = "id, hash, status, cwd, argv, profile_key, fingerprint, content_changed, heavy, mem_mb, disk_mb, uses_net, \
timeout_s, submitted_at, started_at, finished_at, pgid, pgid_started_at, cancel_requested, exit_code, peak_mem_mb, cpu_avg_cores, disk_delta_mb";

fn bad(what: &str, v: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{what}: {v}").into())
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<ExecRow> {
    let status: String = r.get(2)?;
    let argv: String = r.get(4)?;
    // 资源量四个字段由 set_estimate 一起写；只要有一个为空就当作还没评估
    let res = match (r.get::<_, Option<i64>>(8)?, r.get::<_, Option<i64>>(9)?, r.get::<_, Option<i64>>(10)?, r.get::<_, Option<i64>>(11)?) {
        (Some(h), Some(mem_mb), Some(disk_mb), Some(n)) => Some(Resources { heavy: h == 1, mem_mb, disk_mb, uses_net: n == 1 }),
        _ => None,
    };
    Ok(ExecRow {
        id: r.get(0)?,
        hash: r.get(1)?,
        status: ExecStatus::parse(&status).ok_or_else(|| bad("status", &status))?,
        cwd: r.get(3)?,
        argv: serde_json::from_str(&argv).map_err(|_| bad("argv", &argv))?,
        profile_key: r.get(5)?,
        fingerprint: r.get(6)?,
        content_changed: r.get::<_, Option<i64>>(7)?.map(|v| v == 1),
        res,
        timeout_s: r.get(12)?,
        submitted_at: r.get(13)?,
        started_at: r.get(14)?,
        finished_at: r.get(15)?,
        pgid: r.get(16)?,
        pgid_started_at: r.get(17)?,
        cancel_requested: r.get::<_, i64>(18)? == 1,
        exit_code: r.get(19)?,
        peak_mem_mb: r.get(20)?,
        cpu_avg_cores: r.get(21)?,
        disk_delta_mb: r.get(22)?,
    })
}

pub struct NewExec<'a> {
    pub id: &'a str,
    pub hash: Option<&'a str>,
    pub cwd: &'a str,
    pub argv: &'a [String],
    pub profile_key: &'a str,
    pub fingerprint: Option<&'a str>,
    pub submitted_at: i64,
}

pub fn insert_exec(c: &Connection, n: &NewExec<'_>) -> Result<()> {
    let argv = serde_json::to_string(n.argv).map_err(|e| StoreError::Sql(e.to_string()))?;
    c.execute(
        "INSERT INTO executions (id, hash, status, cwd, argv, profile_key, fingerprint, submitted_at) VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7)",
        params![n.id, n.hash, n.cwd, argv, n.profile_key, n.fingerprint, n.submitted_at],
    )?;
    Ok(())
}

/// 同 hash 正在排队或运行的执行（两次等值查询，走 one_inflight 索引）。
pub fn find_inflight(c: &Connection, hash: &str) -> Result<Option<String>> {
    for st in ["running", "queued"] {
        let id = c.query_row("SELECT id FROM executions WHERE hash = ?1 AND status = ?2", params![hash, st], |r| r.get(0)).optional()?;
        if id.is_some() {
            return Ok(id);
        }
    }
    Ok(None)
}

pub fn get_exec(c: &Connection, id: &str) -> Result<Option<ExecRow>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM executions WHERE id = ?1"), [id], from_row).optional()?)
}

/// 全机进行中的执行：先 running 再 queued，各按提交时间。
pub fn active_execs(c: &Connection) -> Result<Vec<ExecRow>> {
    let mut out = vec![];
    for st in ["running", "queued"] {
        let mut q = c.prepare_cached(&format!("SELECT {COLS} FROM executions WHERE status = ?1 ORDER BY submitted_at, id"))?;
        for r in q.query_map([st], from_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

fn changed(n: usize, id: &str, what: &str) -> Result<bool> {
    if n > 1 {
        return Err(StoreError::Corrupt(format!("{what} 改到了 {n} 行：{id}")));
    }
    Ok(n == 1)
}

pub fn set_estimate(c: &Connection, id: &str, r: &Resources, timeout_s: i64) -> Result<bool> {
    let n = c.execute(
        "UPDATE executions SET heavy = ?2, mem_mb = ?3, disk_mb = ?4, uses_net = ?5, timeout_s = ?6 WHERE id = ?1 AND status = 'queued'",
        params![id, r.heavy as i64, r.mem_mb, r.disk_mb, r.uses_net as i64, timeout_s],
    )?;
    changed(n, id, "set_estimate")
}

/// 放行：同一事务内改为 running 并写入 started_at、pgid、pgid_started_at（CASE-STORE-1）。
pub fn mark_running(c: &Connection, id: &str, now: i64, pgid: i64, pgid_started_at: i64) -> Result<bool> {
    let n = c.execute(
        "UPDATE executions SET status = 'running', started_at = ?2, pgid = ?3, pgid_started_at = ?4 WHERE id = ?1 AND status = 'queued' AND cancel_requested = 0",
        params![id, now, pgid, pgid_started_at],
    )?;
    changed(n, id, "mark_running")
}

/// 运行中纠正：按轻任务放行、实测已达重任务线的执行改记为重任务。只升不降（CASE-EST-10、CASE-EST-10）。
pub fn mark_heavy(c: &Connection, id: &str) -> Result<bool> {
    let n = c.execute("UPDATE executions SET heavy = 1 WHERE id = ?1 AND status = 'running' AND heavy = 0", params![id])?;
    changed(n, id, "mark_heavy")
}

pub struct Finish {
    pub status: ExecStatus,
    pub finished_at: i64,
    pub exit_code: Option<i64>,
    pub peak_mem_mb: i64,
    pub cpu_avg_cores: f64,
    pub disk_delta_mb: i64,
    pub content_changed: bool,
}

/// 结束：一条 UPDATE 写完所有终态字段（CASE-STORE-1、CASE-STORE-1）。
pub fn finish(c: &Connection, id: &str, f: &Finish) -> Result<bool> {
    let n = c.execute(
        "UPDATE executions SET status = ?2, finished_at = ?3, exit_code = ?4, peak_mem_mb = ?5, cpu_avg_cores = ?6, disk_delta_mb = ?7, content_changed = ?8 \
         WHERE id = ?1 AND status = 'running'",
        params![id, f.status.as_str(), f.finished_at, f.exit_code, f.peak_mem_mb, f.cpu_avg_cores, f.disk_delta_mb, f.content_changed as i64],
    )?;
    changed(n, id, "finish")
}

fn end_from(c: &Connection, id: &str, from: &str, to: ExecStatus, now: i64) -> Result<bool> {
    let n = c.execute(
        &format!("UPDATE executions SET status = ?2, finished_at = ?3 WHERE id = ?1 AND status = '{from}'"),
        params![id, to.as_str(), now],
    )?;
    changed(n, id, "end")
}

pub fn cancel_queued(c: &Connection, id: &str, now: i64) -> Result<bool> {
    end_from(c, id, "queued", ExecStatus::Cancelled, now)
}

pub fn reject_exec(c: &Connection, id: &str, now: i64) -> Result<bool> {
    end_from(c, id, "queued", ExecStatus::Rejected, now)
}

pub fn interrupt(c: &Connection, id: &str, now: i64) -> Result<bool> {
    end_from(c, id, "running", ExecStatus::Interrupted, now)
}

pub fn request_cancel(c: &Connection, id: &str) -> Result<bool> {
    let n = c.execute("UPDATE executions SET cancel_requested = 1 WHERE id = ?1 AND status = 'running'", [id])?;
    changed(n, id, "request_cancel")
}

fn sample(r: &Row<'_>) -> rusqlite::Result<Sample> {
    let status: String = r.get(4)?;
    let started: i64 = r.get(2)?;
    let finished: i64 = r.get(3)?;
    Ok(Sample {
        peak_mem_mb: r.get(0)?,
        disk_delta_mb: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
        duration_s: (finished - started).max(0) / 1000,
        cpu_avg_cores: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
        usable_for_duration: status != "interrupted" && status != "cancelled",
    })
}

/// 同类命令最近 20 次终态实测，最新在前（只取终态，CASE-EST-5）。
pub fn history(c: &Connection, profile_key: &str) -> Result<Vec<Sample>> {
    let mut q = c.prepare_cached(
        "SELECT peak_mem_mb, disk_delta_mb, started_at, finished_at, status, cpu_avg_cores FROM executions \
         WHERE profile_key = ?1 AND finished_at IS NOT NULL AND peak_mem_mb IS NOT NULL AND started_at IS NOT NULL \
         ORDER BY finished_at DESC LIMIT 20",
    )?;
    let rows = q.query_map([profile_key], sample)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 同类命令在某时刻之前结束、有实测的次数（--human 用来说明评估来自哪里）。
pub fn measured_before(c: &Connection, profile_key: &str, before: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT count(*) FROM executions WHERE profile_key = ?1 AND finished_at IS NOT NULL AND finished_at < ?2 AND peak_mem_mb IS NOT NULL",
        params![profile_key, before],
        |r| r.get(0),
    )?)
}

/// 全机最近 500 次实测：首次见到的命令取中位数，AI 评估的内存以 p90 为上限。
pub fn machine_history(c: &Connection) -> Result<MachineHistory> {
    let mut h = MachineHistory::default();
    for st in ["ok", "failed", "timeout"] {
        let mut q = c.prepare_cached(
            "SELECT peak_mem_mb, disk_delta_mb, started_at, finished_at, status, cpu_avg_cores FROM executions \
             WHERE status = ?1 AND peak_mem_mb IS NOT NULL ORDER BY submitted_at DESC LIMIT 500",
        )?;
        for s in q.query_map([st], sample)? {
            let s = s?;
            h.peaks_mb.push(s.peak_mem_mb);
            h.disk_deltas_mb.push(s.disk_delta_mb);
            h.durations_s.push(s.duration_s);
        }
    }
    Ok(h)
}
