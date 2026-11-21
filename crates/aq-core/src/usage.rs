//! 设备是否在用的判定（design.md 3.3）：纯函数，输入进程列表和设备 id，不列进程、不调 adb。
//! 进程由 aq-sys 列出。

use std::collections::{HashMap, HashSet};

/// 一个进程的快照。`aq_descendant` 由 [`mark_aq_descendants`] 填。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: i64,
    pub ppid: i64,
    /// 可执行文件的完整路径（读不到时为空）。
    pub exe: String,
    pub argv: Vec<String>,
    pub aq_descendant: bool,
}

/// 设备 id 前后紧挨的字符是这些时不算「整段」。
fn word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

const SIM_DEVICES: &str = "coresimulator/devices/";

/// `text` 里整段出现 `id`（不分大小写）。出现在 `CoreSimulator/Devices/<id>` 路径里的不算：那是模拟器自己的进程（launchd_sim、装进去的 App）。
pub fn mentions(text: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let hay = text.to_ascii_lowercase();
    let needle = id.to_ascii_lowercase();
    let (h, n) = (hay.as_bytes(), needle.len());
    let mut from = 0;
    while let Some(pos) = hay.get(from..).and_then(|rest| rest.find(&needle)) {
        let at = from + pos;
        let before_ok = at == 0 || !word(h[at - 1]);
        let after_ok = h.get(at + n).is_none_or(|b| !word(*b));
        if before_ok && after_ok && !hay[..at].ends_with(SIM_DEVICES) {
            return true;
        }
        from = at + 1;
    }
    false
}

/// 参数里有一项整段带着 `id`。
pub fn argv_mentions(argv: &[String], id: &str) -> bool {
    argv.iter().any(|a| mentions(a, id))
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 设备自身的进程：可执行文件在 `CoreSimulator/Devices/<id>/` 下（模拟器里的 App、UI 测试 runner），或是 emulator、qemu 本体。
fn device_own(p: &ProcInfo, id: &str) -> bool {
    let exe = p.exe.to_ascii_lowercase();
    if exe.contains(&format!("{SIM_DEVICES}{}/", id.to_ascii_lowercase())) {
        return true;
    }
    let b = base(&p.exe);
    b == "emulator" || b.starts_with("emulator64") || b.starts_with("qemu-system")
}

/// 这个进程算不算在用设备 `id`（CASE-RECLAIM-11、CASE-RECLAIM-12）。
pub fn uses_device(p: &ProcInfo, id: &str) -> bool {
    !p.aq_descendant && !device_own(p, id) && argv_mentions(&p.argv, id)
}

/// aq 进程：可执行文件名为 aq。例外是别的账本里执行命令的 runner（`__runner ex_…`，含经 `__detach` 拉起的）：
/// 如用 aq run 跑 aq 自己的测试时外层那个，算它的话它下面的进程都会被当成 aq 的。借用的 runner、对账进程只起设备管理工具，哪个账本的都算。
pub fn aq_root(p: &ProcInfo, known_ids: &HashSet<String>) -> bool {
    let exe_aq = base(&p.exe) == "aq" || p.argv.first().is_some_and(|a| base(a) == "aq");
    if !exe_aq {
        return false;
    }
    match p.argv.iter().position(|a| a == "__runner").and_then(|i| p.argv.get(i + 1)) {
        Some(id) if id.starts_with("ex_") => known_ids.contains(id),
        _ => true,
    }
}

/// 标出 aq 自己及其所有后代（沿父进程号向上找 aq 进程）。
pub fn mark_aq_descendants(procs: &mut [ProcInfo], known_ids: &HashSet<String>) {
    let parent: HashMap<i64, i64> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let roots: HashSet<i64> = procs.iter().filter(|p| aq_root(p, known_ids)).map(|p| p.pid).collect();
    for p in procs.iter_mut() {
        let mut cur = p.pid;
        for _ in 0..64 {
            if roots.contains(&cur) {
                p.aq_descendant = true;
                break;
            }
            match parent.get(&cur) {
                Some(&pp) if pp > 1 && pp != cur => cur = pp,
                _ => break,
            }
        }
    }
}

/// 在 `ids` 里，哪些设备此刻有进程在用。
pub fn devices_in_use<'a>(procs: &[ProcInfo], ids: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    ids.into_iter().filter(|id| procs.iter().any(|p| uses_device(p, id))).map(str::to_string).collect()
}

#[cfg(test)]
mod tests;
