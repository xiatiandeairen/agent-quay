//! 机器配置 config.toml：严格解析，未知字段、类型不对都报错并带行号。文件不存在时用内置默认值。

use std::path::Path;

use aq_core::capacity::Capacity;
use serde::Deserialize;

/// 默认把 aq 启动的进程降到 nice 10：机器空闲时不变慢，忙时让用户手上的 App 先拿到 CPU。
pub const DEFAULT_NICE: i64 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub cap: Capacity,
    /// "auto" 或具体命令（如 "claude -p"）。
    pub ai_cli: String,
    /// 借 ios-sim 不写 `:<机型>` 时用的机型。
    pub ios_sim_model: String,
    /// 借 android-emu 时默认的镜像、机型、内存从这个 AVD 读；aq 每次新建自己的 AVD，不启动它（CASE-HOLD-16）。
    pub android_avd: String,
    /// aq 启动的命令、Android 模拟器、浏览器会话的 nice 值：0～20，默认 10，0 不降（CASE-PROT-1～3）。
    pub nice: i32,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Raw {
    heavy_slots: Option<i64>,
    mem: Option<String>,
    disk_floor: Option<String>,
    net_slots: Option<i64>,
    sim_slots: Option<i64>,
    ai_cli: Option<String>,
    nice: Option<i64>,
    #[serde(rename = "ios-sim")]
    ios_sim: Option<Sim>,
    #[serde(rename = "android-emu")]
    android_emu: Option<Emu>,
    #[serde(rename = "ios-device")]
    ios_device: Option<Ids>,
    #[serde(rename = "android-device")]
    android_device: Option<Ids>,
    browser: Option<Browser>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Browser {
    mem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sim {
    model: Option<String>,
    mem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Emu {
    avd: Option<String>,
    mem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ids {
    ids: Vec<String>,
}

/// "28G"、"512M" → MB。
pub fn parse_size_mb(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let (num, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(t.len()));
    let n: f64 = num.parse().map_err(|_| format!("bad size: {s} (write it like 28G or 512M)"))?;
    let mb = match unit.trim().to_ascii_uppercase().as_str() {
        "G" | "GB" => n * 1024.0,
        "M" | "MB" => n,
        "T" | "TB" => n * 1024.0 * 1024.0,
        _ => return Err(format!("unknown size unit: {s} (use G or M)")),
    };
    Ok(mb.round() as i64)
}

fn size(v: &Option<String>, default: i64, key: &str) -> Result<i64, String> {
    match v {
        Some(s) => parse_size_mb(s).map_err(|e| format!("{key}: {e}")),
        None => Ok(default),
    }
}

impl Config {
    /// `physical_mb`：本机物理内存，没配置 `mem` 时据此推出容量（CASE-QUEUE-7）。
    pub fn load(path: &Path, physical_mb: i64) -> Result<Config, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        Self::parse_for(&text, physical_mb).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// 物理内存未知（0）时容量退回内置默认值。
    pub fn parse_for(text: &str, physical_mb: i64) -> Result<Config, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string().replace('\n', " "))?;
        let d = Capacity::default();
        let (sim_model, sim_mem) = raw.ios_sim.map(|s| (s.model, s.mem)).unwrap_or((None, None));
        let (avd, emu_mem) = raw.android_emu.map(|s| (s.avd, s.mem)).unwrap_or((None, None));
        let cap = Capacity {
            heavy_slots: raw.heavy_slots.unwrap_or(d.heavy_slots),
            mem_mb: size(&raw.mem, aq_core::capacity::default_mem_mb(physical_mb), "mem")?,
            disk_floor_mb: size(&raw.disk_floor, d.disk_floor_mb, "disk_floor")?,
            net_slots: raw.net_slots.unwrap_or(d.net_slots),
            sim_slots: raw.sim_slots.unwrap_or_else(|| aq_core::capacity::default_sim_slots(physical_mb)),
            ios_sim_mem_mb: size(&sim_mem, d.ios_sim_mem_mb, "ios-sim.mem")?,
            android_emu_mem_mb: size(&emu_mem, d.android_emu_mem_mb, "android-emu.mem")?,
            browser_mem_mb: size(&raw.browser.and_then(|b| b.mem), d.browser_mem_mb, "browser.mem")?,
            ios_devices: raw.ios_device.map(|i| i.ids).unwrap_or_default(),
            android_devices: raw.android_device.map(|i| i.ids).unwrap_or_default(),
        };
        if cap.heavy_slots < 0 || cap.net_slots < 0 || cap.sim_slots < 0 {
            return Err("heavy_slots, net_slots and sim_slots cannot be negative".into());
        }
        let nice = raw.nice.unwrap_or(DEFAULT_NICE);
        if !(0..=20).contains(&nice) {
            return Err(format!("nice must be between 0 and 20 (got {nice}); 0 turns it off"));
        }
        Ok(Config {
            cap,
            ai_cli: raw.ai_cli.unwrap_or_else(|| "auto".into()),
            ios_sim_model: sim_model.unwrap_or_else(|| "iPhone 17 Pro".into()),
            android_avd: avd.unwrap_or_else(|| "Pixel_8_API_35".into()),
            nice: nice as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_example_parses() {
        let c = Config::parse_for(
            r#"heavy_slots = 2
mem = "28G"
disk_floor = "50G"
net_slots = 3
ai_cli = "auto"
[ios-sim]
model = "iPhone 17 Pro"
mem = "3G"
[android-emu]
avd = "Pixel_8_API_35"
mem = "4G"
[ios-device]
ids = ["00008150-x"]
[android-device]
ids = []
[browser]
mem = "1.5G"
"#,
            0,
        )
        .unwrap();
        assert_eq!(c.cap.mem_mb, 28 * 1024);
        assert_eq!(c.cap.ios_devices, vec!["00008150-x".to_string()]);
        assert_eq!(c.cap.browser_mem_mb, 1536);
    }

    // CASE-STATUS-5
    #[test]
    fn unknown_key_reports_line() {
        let e = Config::parse_for("heavy_slots = 2\nheavy_slot = 3\n", 0).unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        let e = Config::parse_for("mem = 28\n", 0).unwrap_err();
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn empty_means_defaults() {
        assert_eq!(Config::parse_for("", 0).unwrap().cap, Capacity::default());
    }

    // CASE-QUEUE-7
    #[test]
    fn default_memory_from_physical() {
        assert_eq!(Config::parse_for("", 36 * 1024).unwrap().cap.mem_mb, 27 * 1024, "36G 的 3/4");
        assert_eq!(Config::parse_for("", 16 * 1024).unwrap().cap.mem_mb, 12 * 1024);
        assert_eq!(Config::parse_for("mem = \"20G\"\n", 36 * 1024).unwrap().cap.mem_mb, 20 * 1024, "配置了以配置为准");
        assert_eq!(Config::parse_for("", 0).unwrap().cap.mem_mb, 28 * 1024, "读不到物理内存退回 28G");
    }

    // CASE-QUEUE-7
    #[test]
    fn default_sim_slots_from_physical() {
        let slots = |text: &str, phys: i64| Config::parse_for(text, phys).unwrap().cap.sim_slots;
        assert_eq!(slots("", 36 * 1024), 4, "36G：每 8G 一台");
        assert_eq!(slots("", 16 * 1024), 2);
        assert_eq!(slots("", 64 * 1024), 8);
        assert_eq!(slots("", 6 * 1024), 1, "至少 1 台");
        assert_eq!(slots("", 0), 4, "读不到物理内存时 4 台");
        assert_eq!(slots("sim_slots = 6\n", 36 * 1024), 6, "配置了以配置为准");
    }
}
