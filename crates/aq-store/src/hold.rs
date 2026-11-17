//! holds、hold_devices、hold_accounts 的读写。

use aq_core::model::{AccountRow, DeviceRow, DeviceType, HoldRequest, HoldStatus, ProcId};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq)]
pub struct HoldRow {
    pub id: String,
    pub status: HoldStatus,
    pub request: HoldRequest,
    pub owner: ProcId,
    pub requested_at: i64,
    pub acquired_at: Option<i64>,
    pub ended_at: Option<i64>,
}

const COLS: &str = "id, status, request, owner_pid, owner_started_at, requested_at, acquired_at, ended_at";

fn bad(what: &str, v: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{what}: {v}").into())
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<HoldRow> {
    let status: String = r.get(1)?;
    let req: String = r.get(2)?;
    Ok(HoldRow {
        id: r.get(0)?,
        status: HoldStatus::parse(&status).ok_or_else(|| bad("status", &status))?,
        request: serde_json::from_str(&req).map_err(|_| bad("request", &req))?,
        owner: ProcId { pid: r.get(3)?, started_at: r.get(4)? },
        requested_at: r.get(5)?,
        acquired_at: r.get(6)?,
        ended_at: r.get(7)?,
    })
}

pub struct NewHold<'a> {
    pub id: &'a str,
    pub request: &'a HoldRequest,
    pub owner: ProcId,
    pub requested_at: i64,
}

pub fn insert_hold(c: &Connection, n: &NewHold<'_>) -> Result<()> {
    let req = serde_json::to_string(n.request).map_err(|e| StoreError::Sql(e.to_string()))?;
    c.execute(
        "INSERT INTO holds (id, status, request, owner_pid, owner_started_at, requested_at) VALUES (?1, 'waiting', ?2, ?3, ?4, ?5)",
        params![n.id, req, n.owner.pid, n.owner.started_at, n.requested_at],
    )?;
    Ok(())
}

pub fn get_hold(c: &Connection, id: &str) -> Result<Option<HoldRow>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM holds WHERE id = ?1"), [id], from_row).optional()?)
}

/// 未归还的借用：先 held 再 waiting。
pub fn active_holds(c: &Connection) -> Result<Vec<HoldRow>> {
    let mut out = vec![];
    for st in ["held", "waiting"] {
        let mut q = c.prepare_cached(&format!("SELECT {COLS} FROM holds WHERE status = ?1 ORDER BY requested_at, id"))?;
        for r in q.query_map([st], from_row)? {
            out.push(r?);
        }
    }
    Ok(out)
}

pub fn devices(c: &Connection) -> Result<Vec<DeviceRow>> {
    let mut q = c.prepare_cached("SELECT hold_id, seq, type, device FROM hold_devices ORDER BY hold_id, seq")?;
    let rows = q.query_map([], |r| {
        let t: String = r.get(2)?;
        Ok(DeviceRow {
            hold_id: r.get(0)?,
            seq: r.get(1)?,
            dtype: DeviceType::parse(&t).ok_or_else(|| bad("type", &t))?,
            device: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn accounts(c: &Connection) -> Result<Vec<AccountRow>> {
    let mut q = c.prepare_cached("SELECT account, hold_id FROM hold_accounts ORDER BY account")?;
    let rows = q.query_map([], |r| Ok(AccountRow { account: r.get(0)?, hold_id: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 轮到分配：同一事务写入真机行（device 有值）、模拟器行（device 为空，占住内存）、账号行（CASE-STORE-1）。
pub fn allocate(c: &Connection, hold_id: &str, devs: &[(DeviceType, Option<String>)], accounts: &[String]) -> Result<()> {
    for (seq, (t, d)) in devs.iter().enumerate() {
        c.execute(
            "INSERT INTO hold_devices (hold_id, seq, type, device) VALUES (?1, ?2, ?3, ?4)",
            params![hold_id, seq as i64 + 1, t.as_str(), d],
        )?;
    }
    for a in accounts {
        c.execute("INSERT INTO hold_accounts (account, hold_id) VALUES (?1, ?2)", params![a, hold_id])?;
    }
    Ok(())
}

pub fn set_device(c: &Connection, hold_id: &str, seq: i64, udid: &str) -> Result<()> {
    c.execute("UPDATE hold_devices SET device = ?3 WHERE hold_id = ?1 AND seq = ?2", params![hold_id, seq, udid])?;
    Ok(())
}

pub fn mark_held(c: &Connection, id: &str, now: i64) -> Result<bool> {
    let n = c.execute("UPDATE holds SET status = 'held', acquired_at = ?2 WHERE id = ?1 AND status = 'waiting'", params![id, now])?;
    Ok(n == 1)
}

/// 删掉一组的全部设备行和账号行（创建失败回滚、runner 重拉前清理）。
pub fn clear_allocation(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM hold_devices WHERE hold_id = ?1", [id])?;
    c.execute("DELETE FROM hold_accounts WHERE hold_id = ?1", [id])?;
    Ok(())
}

/// 模拟器创建失败：rejected，写 ended_at，设备行和账号行全部删除（CASE-STORE-1）。
pub fn reject_hold(c: &Connection, id: &str, now: i64) -> Result<bool> {
    let n = c.execute("UPDATE holds SET status = 'rejected', ended_at = ?2 WHERE id = ?1 AND status = 'waiting'", params![id, now])?;
    clear_allocation(c, id)?;
    Ok(n == 1)
}

/// 归还或收回：held → released/reclaimed；waiting → released。账号行、真机行立即删除；
/// 模拟器行保留到后台删完（CASE-RECLAIM-1、CASE-STORE-1、CASE-STORE-1）。返回是否真的改了状态（CASE-RECLAIM-7：只有一个生效）。
pub fn end_hold(c: &Connection, id: &str, to: HoldStatus, now: i64) -> Result<bool> {
    let n = c.execute(
        "UPDATE holds SET status = ?2, ended_at = ?3 WHERE id = ?1 AND status IN ('held', 'waiting')",
        params![id, to.as_str(), now],
    )?;
    if n == 0 {
        return Ok(false);
    }
    c.execute("DELETE FROM hold_accounts WHERE hold_id = ?1", [id])?;
    c.execute("DELETE FROM hold_devices WHERE hold_id = ?1 AND (type IN ('ios-device','android-device') OR device IS NULL)", [id])?;
    Ok(true)
}

/// 已结束借用留下的模拟器行：(hold_id, seq, 类型, 设备 id)，交给后台删除。
pub fn orphan_sim_rows(c: &Connection) -> Result<Vec<(String, i64, DeviceType, String)>> {
    let mut q = c.prepare_cached(
        "SELECT d.hold_id, d.seq, d.type, d.device FROM hold_devices d JOIN holds h ON h.id = d.hold_id \
         WHERE h.status IN ('released','reclaimed','rejected') AND d.device IS NOT NULL",
    )?;
    let rows = q.query_map([], |r| {
        let t: String = r.get(2)?;
        Ok((r.get(0)?, r.get(1)?, DeviceType::parse(&t).ok_or_else(|| bad("type", &t))?, r.get(3)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn delete_device_row(c: &Connection, hold_id: &str, seq: i64) -> Result<()> {
    c.execute("DELETE FROM hold_devices WHERE hold_id = ?1 AND seq = ?2", params![hold_id, seq])?;
    Ok(())
}
