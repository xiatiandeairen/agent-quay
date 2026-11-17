//! profiles：一类命令的 AI 评估缓存。只缓存 AI 成功的结果。

use aq_core::model::Resources;
use rusqlite::{params, Connection, OptionalExtension};

use crate::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub profile_key: String,
    pub signature: String,
    pub res: Resources,
    pub est_s: i64,
    pub ai_output: String,
}

pub fn get_profile(c: &Connection, key: &str) -> Result<Option<Profile>> {
    Ok(c.query_row(
        "SELECT profile_key, signature, heavy, mem_mb, disk_mb, uses_net, est_s, ai_output FROM profiles WHERE profile_key = ?1",
        [key],
        |r| {
            Ok(Profile {
                profile_key: r.get(0)?,
                signature: r.get(1)?,
                res: Resources {
                    heavy: r.get::<_, i64>(2)? == 1,
                    mem_mb: r.get(3)?,
                    disk_mb: r.get(4)?,
                    uses_net: r.get::<_, i64>(5)? == 1,
                },
                est_s: r.get(6)?,
                ai_output: r.get(7)?,
            })
        },
    )
    .optional()?)
}

pub fn put_profile(c: &Connection, p: &Profile) -> Result<()> {
    c.execute(
        "INSERT OR REPLACE INTO profiles (profile_key, signature, heavy, mem_mb, disk_mb, uses_net, est_s, ai_output) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![p.profile_key, p.signature, p.res.heavy as i64, p.res.mem_mb, p.res.disk_mb, p.res.uses_net as i64, p.est_s, p.ai_output],
    )?;
    Ok(())
}
