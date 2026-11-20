//! 真机可用性：解析 devicectl / adb 的输出，按「已声明 ∩ 此刻可用」判定能不能借。
//! 只在 aq hold 提交时判定一次；声明了但此刻不可用的 id 记进借用请求（`HoldRequest.exclude`），排队后分配时跳过。

use serde_json::Value;

use crate::capacity::Capacity;
use crate::model::{DeviceType, HoldRequest, Reason};

/// 探测看到的一台真机；`problem` 为 None 表示此刻可用，否则是给人看的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub kind: DeviceType,
    pub id: String,
    pub name: String,
    pub problem: Option<String>,
}

pub const NOT_FOUND: &str = "the Mac cannot find it (powered off, not connected, or on another network)";

/// `devicectl list devices --json-output` 的输出 → 真机列表（模拟器跳过）。
/// 可用 = 已配对 + 开发者模式已开 + 有线或同网无线；调试通道（tunnelState）用时才建，不作条件。
pub fn parse_devicectl(text: &str) -> Result<Vec<Seen>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("cannot parse devicectl output: {e}"))?;
    let list = v["result"]["devices"].as_array().ok_or("devicectl output has no result.devices")?;
    let mut out = vec![];
    for d in list.iter().filter(|d| d["hardwareProperties"]["reality"] == "physical") {
        let id = d["hardwareProperties"]["udid"].as_str().or_else(|| d["identifier"].as_str()).unwrap_or_default();
        let name = d["deviceProperties"]["name"].as_str().unwrap_or(id);
        let conn = &d["connectionProperties"];
        let problem = if conn["pairingState"] != "paired" {
            Some("not paired (pair it in Xcode and trust this Mac)".to_string())
        } else if d["deviceProperties"]["developerModeStatus"] != "enabled" {
            Some("Developer Mode is off".to_string())
        } else if !matches!(conn["transportType"].as_str(), Some("wired" | "localNetwork")) {
            Some(NOT_FOUND.to_string())
        } else {
            None
        };
        out.push(Seen { kind: DeviceType::IosDevice, id: id.to_string(), name: name.to_string(), problem });
    }
    Ok(out)
}

/// `adb devices -l` 的输出 → 真机列表（`emulator-` 开头的是模拟器，跳过）。只有状态 `device` 算可用。
pub fn parse_adb(text: &str) -> Vec<Seen> {
    let mut out = vec![];
    for line in text.lines().skip_while(|l| !l.starts_with("List of devices")).skip(1) {
        let mut f = line.split_whitespace();
        let (Some(id), Some(state)) = (f.next(), f.next()) else { continue };
        if id.starts_with("emulator-") {
            continue;
        }
        let name = f.find_map(|kv| kv.strip_prefix("model:")).map_or(id.to_string(), |m| m.replace('_', " "));
        let problem = match state {
            "device" => None,
            "unauthorized" => Some("USB debugging not authorized (tap Allow on the phone)".to_string()),
            "offline" => Some("offline (replug, or reconnect wireless debugging)".to_string()),
            s => Some(format!("adb state is {s}")),
        };
        out.push(Seen { kind: DeviceType::AndroidDevice, id: id.to_string(), name, problem });
    }
    out
}

/// 声明了但此刻不可用的一台。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub kind: DeviceType,
    pub id: String,
    pub name: String,
    pub why: String,
}

/// 判定结果。`reject` 为 Some((reason, message)) 时立即拒绝，这时才带 `undeclared` 和 `snippet`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub reject: Option<(Reason, String)>,
    pub unavailable: Vec<Unavailable>,
    pub undeclared: Vec<Seen>,
    pub snippet: Option<String>,
    /// 声明了但此刻不可用的 id：不拒绝时记进借用请求，分配时跳过。
    pub exclude: Vec<String>,
}

/// 请求里出现的真机类型（去重，按请求顺序）。
pub fn real_types(req: &HoldRequest) -> Vec<DeviceType> {
    let mut out: Vec<DeviceType> = vec![];
    for t in req.devices.iter().filter(|t| !t.is_virtual()) {
        if !out.contains(t) {
            out.push(*t);
        }
    }
    out
}

/// 按类型判定：要的台数 > 清单台数 → unschedulable；> 声明且可用的台数（被借走的也算可用）→ device_unavailable。
/// `scans` 是每类真机的探测结果；Err 表示探测不了（工具缺失、超时），这类声明的设备都按不可用算。
pub fn judge(req: &HoldRequest, cap: &Capacity, scans: &[(DeviceType, Result<Vec<Seen>, String>)]) -> Verdict {
    let mut v = Verdict::default();
    for (t, scan) in scans {
        let declared = cap.real_devices(*t);
        let seen: &[Seen] = scan.as_deref().unwrap_or(&[]);
        v.undeclared.extend(seen.iter().filter(|s| s.problem.is_none() && !declared.contains(&s.id)).cloned());
        let mut down = vec![];
        for id in declared {
            let hit = seen.iter().find(|s| &s.id == id);
            let why = match (scan, hit) {
                (Err(e), _) => Some(e.clone()),
                (Ok(_), None) => Some(NOT_FOUND.to_string()),
                (Ok(_), Some(s)) => s.problem.clone(),
            };
            if let Some(why) = why {
                let name = hit.map_or_else(|| id.clone(), |s| s.name.clone());
                down.push(Unavailable { kind: *t, id: id.clone(), name, why });
            }
        }
        let want = req.devices.iter().filter(|d| *d == t).count();
        let usable = declared.len() - down.len();
        if v.reject.is_none() {
            if want > declared.len() {
                v.reject = Some((Reason::Unschedulable, format!("needs {want} {t}, only {} declared", declared.len())));
            } else if want > usable {
                let list: Vec<String> = down.iter().map(|d| format!("{}: {}", d.name, d.why)).collect();
                v.reject =
                    Some((Reason::DeviceUnavailable, format!("needs {want} {t}, {usable} available right now. {}", list.join("; "))));
            }
        }
        v.exclude.extend(down.iter().map(|d| d.id.clone()));
        v.unavailable.extend(down);
    }
    if v.reject.is_some() {
        v.snippet = snippet(cap, &v.undeclared);
    } else {
        v.undeclared.clear();
    }
    v
}

/// 可整段替换进 config.toml 的配置：已声明的 + 新发现的。没有新发现的设备时为 None。
fn snippet(cap: &Capacity, undeclared: &[Seen]) -> Option<String> {
    let mut parts = vec![];
    for t in [DeviceType::IosDevice, DeviceType::AndroidDevice] {
        let new: Vec<&String> = undeclared.iter().filter(|s| s.kind == t).map(|s| &s.id).collect();
        if new.is_empty() {
            continue;
        }
        let ids: Vec<String> = cap.real_devices(t).iter().chain(new).map(|id| format!("\"{id}\"")).collect();
        parts.push(format!("[{t}]\nids = [{}]", ids.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod tests;
