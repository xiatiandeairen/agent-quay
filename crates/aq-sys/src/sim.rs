//! iOS 模拟器（xcrun simctl）。借时按配置机型新建 aq-<账本标识>-<借用id>-<序号> 并启动，还时关机删除。
//! 删设备时一并删掉它在 ~/Library/Logs/CoreSimulator 下的日志目录。
//! 验收测试用 AQ_SIMCTL 指向假 simctl 脚本（可注入创建失败，CASE-HOLD-7、CASE-HOLD-7）。
//! android-emu 一期未实现：真实记录里没有 Android 用法，借用时返回 device_error。

use std::time::Duration;

use aq_core::ports::Simulator;
use serde::Deserialize;

use crate::cmd::{ok_text, Call};

pub struct SimCtl {
    pub cmd: Vec<String>,
    /// 新建用的机型名，如 "iPhone 17 Pro"。
    pub model: String,
}

impl SimCtl {
    pub fn from_env(model: &str) -> SimCtl {
        let cmd = std::env::var("AQ_SIMCTL").map(|s| vec![s]).unwrap_or_else(|_| vec!["xcrun".into(), "simctl".into()]);
        SimCtl { cmd, model: model.to_string() }
    }

    fn call(&self, args: &[&str], secs: u64) -> Result<String, String> {
        let mut argv = self.cmd.clone();
        argv.extend(args.iter().map(|s| s.to_string()));
        ok_text(&Call { argv: &argv, cwd: None, env: &[], stdin: None, timeout: Duration::from_secs(secs) })
    }

    /// 本机支持配置机型的最新可用 iOS 运行时。只为单一机型准备的运行时（如 iOS-27-2-iPhone19-4）不支持其他机型，要跳过。
    fn latest_ios_runtime(&self) -> Result<String, String> {
        #[derive(Deserialize)]
        struct DevType {
            name: String,
        }
        #[derive(Deserialize)]
        struct Rt {
            identifier: String,
            version: String,
            #[serde(rename = "isAvailable", default)]
            is_available: bool,
            #[serde(default)]
            platform: String,
            #[serde(rename = "supportedDeviceTypes", default)]
            supported: Vec<DevType>,
        }
        #[derive(Deserialize)]
        struct List {
            runtimes: Vec<Rt>,
        }
        let l: List = serde_json::from_str(&self.call(&["list", "runtimes", "-j"], 60)?).map_err(|e| format!("解析运行时列表失败：{e}"))?;
        let key = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
        l.runtimes
            .into_iter()
            .filter(|r| r.is_available && (r.platform == "iOS" || r.identifier.contains("iOS")))
            .filter(|r| r.supported.is_empty() || r.supported.iter().any(|d| d.name == self.model))
            .max_by_key(|r| (key(&r.version), r.supported.len()))
            .map(|r| r.identifier)
            .ok_or_else(|| format!("本机没有支持 {} 的 iOS 运行时", self.model))
    }
}

/// simctl delete 不删 ~/Library/Logs/CoreSimulator/<udid>（每台约 100K，会一直累积），删设备后顺手删掉。
/// udid 只允许十六进制和连字符，防止拼出别的路径。
fn remove_log_dir(udid: &str) {
    if udid.is_empty() || !udid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return;
    }
    let Ok(home) = std::env::var("HOME") else { return };
    let dir = std::path::Path::new(&home).join("Library/Logs/CoreSimulator").join(udid);
    let _ = std::fs::remove_dir_all(dir); // 不存在或删不掉都不影响归还
}

impl Simulator for SimCtl {
    fn create(&self, name: &str) -> Result<String, String> {
        let rt = self.latest_ios_runtime()?;
        let udid = self.call(&["create", name, &self.model, &rt], 120)?.trim().to_string();
        if udid.is_empty() {
            return Err("simctl create 没有返回 UDID".into());
        }
        let booted = self.call(&["boot", &udid], 180).and_then(|_| self.call(&["bootstatus", &udid, "-b"], 300));
        if let Err(e) = booted {
            let _ = self.delete(&udid); // 启动失败的半成品立即删；删失败由本账本的定期对账兜底
            return Err(e);
        }
        Ok(udid)
    }

    fn delete(&self, udid: &str) -> Result<(), String> {
        let _ = self.call(&["shutdown", udid], 60); // 已关机时会报错，忽略
        self.call(&["delete", udid], 120)?;
        remove_log_dir(udid);
        Ok(())
    }

    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        #[derive(Deserialize)]
        struct Dev {
            name: String,
            udid: String,
        }
        #[derive(Deserialize)]
        struct List {
            devices: std::collections::HashMap<String, Vec<Dev>>,
        }
        let l: List = serde_json::from_str(&self.call(&["list", "devices", "-j"], 60)?).map_err(|e| format!("解析设备列表失败：{e}"))?;
        Ok(l.devices.into_values().flatten().filter(|d| d.name.starts_with("aq-")).map(|d| (d.name, d.udid)).collect())
    }
}
