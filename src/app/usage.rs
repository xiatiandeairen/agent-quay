//! 检查时探测借出的设备是否在用（design.md 5.5）：列出本机进程交给 aq-core 的 usage 判定；
//! 浏览器、桌面会话不探测。

use std::collections::HashSet;
use std::fs::File;

use aq_core::check::Snapshot;
use aq_core::model::{DeviceRow, HoldStatus};
use aq_core::usage::{devices_in_use, mark_aq_descendants};

use super::ctx::Ctx;

/// 此刻在用的借用 id。没有借出的 iOS、Android 设备时不列进程。
pub fn in_use(s: &Snapshot) -> HashSet<String> {
    let held: HashSet<&str> = s.holds.iter().filter(|h| h.status == HoldStatus::Held).map(|h| h.id.as_str()).collect();
    let devs: Vec<&DeviceRow> =
        s.devices.iter().filter(|d| d.dtype.usage_probed() && d.device.is_some() && held.contains(d.hold_id.as_str())).collect();
    if devs.is_empty() {
        return HashSet::new();
    }
    let known: HashSet<String> = s.execs.iter().map(|e| e.id.clone()).chain(s.holds.iter().map(|h| h.id.clone())).collect();
    let mut procs = aq_sys::procs::list_all();
    mark_aq_descendants(&mut procs, &known);
    let busy = devices_in_use(&procs, devs.iter().filter_map(|d| d.device.as_deref()));
    devs.iter().filter(|d| d.device.as_ref().is_some_and(|id| busy.contains(id))).map(|d| d.hold_id.clone()).collect()
}

/// 把 holds/<id>.used 的修改时间改成现在（CASE-RECLAIM-4）。
pub fn mark_used(ctx: &Ctx, id: &str) {
    let path = ctx.dirs.used_file(id);
    if let Ok(f) = File::options().create(true).append(true).open(&path) {
        let _ = f.set_modified(std::time::SystemTime::now()); // 改不了时下次检查再改；最坏按闲置收回
    }
}
