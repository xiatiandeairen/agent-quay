//! 模拟器后台删除与按名字对账。
//! 已结束借用的模拟器行：关机删除后再删行，删完它占的内存才算释放。
//! 每 10 分钟列出本账本的模拟器、实例和浏览器会话（名字带本账本标识，见 aq-core devname），对应借用已不是 waiting/held 的就删掉；
//! 别的账本的设备一律不动（CASE-HOLD-9）。同一时刻只有一个清理进程（锁 simgc）。

use std::collections::HashSet;

use aq_core::model::{DeviceType, HoldStatus};
use aq_core::ports::{Locks, Simulator};
use aq_store::Store;
use aq_sys::lock::inheritable;

use super::ctx::Ctx;
use super::error::{AppError, AppResult};

fn stamp(ctx: &Ctx) -> std::path::PathBuf {
    ctx.dirs.state.join("simgc.stamp")
}

/// 距上次对账超过间隔。
pub fn due(ctx: &Ctx) -> bool {
    let age = std::fs::metadata(stamp(ctx)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_millis() as i64);
    age.is_none_or(|a| a >= ctx.tuning.simgc_every_ms)
}

/// 没有清理进程在跑时拉起一个。
pub fn kick(ctx: &Ctx) -> AppResult<()> {
    let Some(lock) = ctx.locks.try_lock("simgc").map_err(AppError::Fatal)? else { return Ok(()) };
    let _ = std::fs::write(stamp(ctx), b""); // 只用修改时间；写失败最多多对账一次
    let fd = inheritable(&lock).map_err(AppError::Fatal)?;
    super::spawn::detach(ctx, &["__simgc".into()], fd)
}

/// `aq __simgc`：删除已结束借用的模拟器，然后按名字对账。
pub fn run(ctx: &Ctx, store: &mut Store) -> AppResult<()> {
    loop {
        let rows = aq_store::orphan_sim_rows(store.conn())?;
        if rows.is_empty() {
            break;
        }
        for (hold_id, seq, t, udid) in rows {
            let r = ctx.virtual_backend(t).map_or(Ok(()), |b| b.delete(&udid));
            let what = device_noun(t);
            if let Err(e) = r {
                ctx.log_note(&hold_id, &format!("deleting {what} {udid} failed: {e}; will retry on the next sweep"));
            }
            store.tx(|c| aq_store::delete_device_row(c, &hold_id, seq))?;
            ctx.log_note(&hold_id, &format!("{what} {udid} deleted, its booked memory released"));
        }
    }
    for t in [DeviceType::IosSim, DeviceType::AndroidEmu, DeviceType::Browser] {
        if let Some(b) = ctx.virtual_backend(t) {
            reconcile(ctx, store, t, b.as_ref())?;
        }
    }
    Ok(())
}

/// 日志里怎么称呼这类设备（CASE-RECLAIM-1）。
fn device_noun(t: DeviceType) -> &'static str {
    match t {
        DeviceType::IosSim => "simulator",
        DeviceType::AndroidEmu => "Android emulator",
        DeviceType::Browser => "browser session",
        DeviceType::Desktop => "desktop session",
        DeviceType::IosDevice | DeviceType::AndroidDevice => "physical device",
    }
}

fn reconcile(ctx: &Ctx, store: &mut Store, t: DeviceType, sim: &dyn Simulator) -> AppResult<()> {
    let Ok(list) = sim.list_aq() else { return Ok(()) }; // 没装 Xcode 等：没有模拟器可对账
    let holds = aq_store::active_holds(store.conn())?;
    let waiting: HashSet<String> = holds.iter().filter(|h| h.status == HoldStatus::Waiting).map(|h| h.id.clone()).collect();
    let held: HashSet<String> = holds.iter().filter(|h| h.status == HoldStatus::Held).map(|h| h.id.clone()).collect();
    let known: HashSet<String> = aq_store::devices(store.conn())?.into_iter().filter_map(|d| d.device).collect();
    for (name, udid) in list {
        let Some(hold_id) = ctx.my_hold_of(&name) else { continue }; // 别的账本或认不出的名字：不归本账本管
                                                                     // 等待中的借用可能正在创建，不动；已借出的只保留记在设备行里的那台；其余都是残留
        let keep = waiting.contains(&hold_id) || (held.contains(&hold_id) && known.contains(&udid));
        if !keep {
            let _ = sim.delete(&udid); // 删失败下次对账再删
            ctx.log_note(&hold_id, &format!("sweep: deleting leftover {} {name}", device_noun(t)));
        }
    }
    Ok(())
}
