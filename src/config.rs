//! 机器配置 config.toml：严格解析，未知字段、类型不对都报错并带行号。文件不存在时用内置默认值。

use std::path::Path;

use aq_core::capacity::Capacity;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub cap: Capacity,
    /// "auto" 或具体命令（如 "claude -p"）。
    pub ai_cli: String,
    pub ios_sim_model: String,
    pub android_avd: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Raw {
    heavy_slots: Option<i64>,
    mem: Option<String>,
    disk_floor: Option<String>,
    net_slots: Option<i64>,
    ai_cli: Option<String>,
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
    let n: f64 = num.parse().map_err(|_| format!("大小写法不对：{s}（应为 28G、512M 这样）"))?;
    let mb = match unit.trim().to_ascii_uppercase().as_str() {
        "G" | "GB" => n * 1024.0,
        "M" | "MB" => n,
        "T" | "TB" => n * 1024.0 * 1024.0,
        _ => return Err(format!("大小单位不认识：{s}（用 G 或 M）")),
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
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
        };
        Self::parse(&text).map_err(|e| format!("{}：{e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string().replace('\n', " "))?;
        let d = Capacity::default();
        let (sim_model, sim_mem) = raw.ios_sim.map(|s| (s.model, s.mem)).unwrap_or((None, None));
        let (avd, emu_mem) = raw.android_emu.map(|s| (s.avd, s.mem)).unwrap_or((None, None));
        let cap = Capacity {
            heavy_slots: raw.heavy_slots.unwrap_or(d.heavy_slots),
            mem_mb: size(&raw.mem, d.mem_mb, "mem")?,
            disk_floor_mb: size(&raw.disk_floor, d.disk_floor_mb, "disk_floor")?,
            net_slots: raw.net_slots.unwrap_or(d.net_slots),
            ios_sim_mem_mb: size(&sim_mem, d.ios_sim_mem_mb, "ios-sim.mem")?,
            android_emu_mem_mb: size(&emu_mem, d.android_emu_mem_mb, "android-emu.mem")?,
            browser_mem_mb: size(&raw.browser.and_then(|b| b.mem), d.browser_mem_mb, "browser.mem")?,
            ios_devices: raw.ios_device.map(|i| i.ids).unwrap_or_default(),
            android_devices: raw.android_device.map(|i| i.ids).unwrap_or_default(),
        };
        if cap.heavy_slots < 0 || cap.net_slots < 0 {
            return Err("heavy_slots、net_slots 不能为负数".into());
        }
        Ok(Config {
            cap,
            ai_cli: raw.ai_cli.unwrap_or_else(|| "auto".into()),
            ios_sim_model: sim_model.unwrap_or_else(|| "iPhone 17 Pro".into()),
            android_avd: avd.unwrap_or_else(|| "Pixel_8_API_35".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_example_parses() {
        let c = Config::parse(
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
        )
        .unwrap();
        assert_eq!(c.cap.mem_mb, 28 * 1024);
        assert_eq!(c.cap.ios_devices, vec!["00008150-x".to_string()]);
        assert_eq!(c.cap.browser_mem_mb, 1536);
    }

    // CASE-STATUS-5
    #[test]
    fn unknown_key_reports_line() {
        let e = Config::parse("heavy_slots = 2\nheavy_slot = 3\n").unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        let e = Config::parse("mem = 28\n").unwrap_err();
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn empty_means_defaults() {
        assert_eq!(Config::parse("").unwrap().cap, Capacity::default());
    }
}
