//! aq-store：SQLite 账本。
//! 每个连接：busy_timeout = 5000、foreign_keys = ON；建库时：journal_mode = WAL、synchronous = NORMAL。
//! 查询约定：列出进行中的执行分两次等值查询（先 running 再 queued），不写 status IN (...)。

mod exec;
mod hold;
mod profile;
mod retention;
mod snapshot;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, TransactionBehavior};
use rusqlite_migration::{Migrations, M};

pub use exec::{ExecRow, Finish, NewExec};
pub use hold::{HoldRow, NewHold};
pub use profile::Profile;
pub use rusqlite::types::{FromSql, FromSqlResult, ValueRef};
pub use rusqlite::Connection as Conn;
pub use rusqlite::ToSql;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// 等写锁超过 5 秒：reason=busy，退出码 1，数据未改。
    #[error("database busy: waited over 5s for the write lock")]
    Busy,
    #[error("database error: {0}")]
    Sql(String),
    #[error("database corrupt: {0}")]
    Corrupt(String),
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked) => StoreError::Busy,
            _ => StoreError::Sql(e.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

const MIGRATION_SQL: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_browser_desktop.sql"),
    include_str!("../migrations/0003_disk_samples.sql"),
];

fn migrations() -> Migrations<'static> {
    Migrations::new(MIGRATION_SQL.iter().map(|s| M::up(s)).collect())
}

/// 当前结构版本（PRAGMA user_version）。
pub const SCHEMA_VERSION: i64 = MIGRATION_SQL.len() as i64;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_millis(5000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        if !Self::ready(&conn)? {
            // 多个进程同时第一次建库：用库旁边的文件锁排队，拿到锁后再看一次版本（迁移库自己在事务外读版本，会撞车）
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path.with_extension("db.migrate.lock"))
                .map_err(|e| StoreError::Sql(format!("cannot open the migration lock: {e}")))?;
            lock.lock().map_err(|e| StoreError::Sql(format!("cannot take the migration lock: {e}")))?;
            let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(StoreError::Sql(format!("cannot set journal_mode: {mode}")));
            }
            Self::migrate(&mut conn)?;
        }
        Ok(Store { conn })
    }

    /// 已是 WAL 且结构是当前版本：不用迁移。
    fn ready(conn: &Connection) -> Result<bool> {
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        Ok(mode.eq_ignore_ascii_case("wal") && v == SCHEMA_VERSION)
    }

    /// 升级到当前版本：只做加法，旧数据保留（CASE-STORE-6）。迁移在 IMMEDIATE 事务里，多进程同时打开也安全。
    fn migrate(conn: &mut Connection) -> Result<()> {
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v == SCHEMA_VERSION {
            return Ok(());
        }
        if v > SCHEMA_VERSION {
            return Err(StoreError::Corrupt(format!("database version {v} is newer than {SCHEMA_VERSION}, the newest this aq supports")));
        }
        migrations().to_latest(conn).map_err(|e| match e {
            rusqlite_migration::Error::RusqliteError { err, .. } => StoreError::from(err),
            other => StoreError::Sql(other.to_string()),
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 一个 BEGIN IMMEDIATE 事务：开事务 → 读快照 → 算动作 → 执行 → 提交。出错整体回滚。
    pub fn tx<T, E: From<StoreError>>(&mut self, f: impl FnOnce(&Connection) -> std::result::Result<T, E>) -> std::result::Result<T, E> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(StoreError::from)?;
        let out = f(&tx)?;
        tx.commit().map_err(StoreError::from)?;
        Ok(out)
    }
}

/// 迁移后的结构（建表语句），用于和 docs/schema.sql 比对。
pub fn schema_dump(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type, name")?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub use exec::{active_execs, disk_samples, disk_shared, find_inflight, get_exec, history, machine_history, measured_before};
pub use exec::{
    cancel_queued, cancel_running, finish, insert_exec, interrupt, mark_heavy, mark_running, reject_exec, request_cancel, set_estimate,
};
pub use hold::{accounts, active_holds, allocate, clear_allocation, devices, end_hold, get_hold, insert_hold, mark_held, set_device};
pub use hold::{delete_device_row, orphan_sim_rows, reject_hold};
pub use profile::{get_profile, put_profile};
pub use retention::prune;
pub use snapshot::snapshot;
