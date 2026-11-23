//! iOS 模拟器（xcrun simctl）。借时按指定的版本、机型（不写取最新运行时、配置机型）新建 aq-<账本标识>-<借用id>-<序号> 并启动，还时关机删除。
//! 删设备时一并删掉它在 ~/Library/Logs/CoreSimulator 下的日志目录。
//! 验收测试用 AQ_SIMCTL 指向假 simctl 脚本（可注入创建失败，CASE-HOLD-7、CASE-HOLD-7）。
//! android-emu 一期未实现：真实记录里没有 Android 用法，借用时返回 device_error。

use std::time::Duration;

use aq_core::devspec::DeviceSpec;
use aq_core::ports::{Simulator, SpecError};
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

    fn ios_runtimes(&self) -> Result<Vec<Rt>, String> {
        #[derive(Deserialize)]
        struct List {
            runtimes: Vec<Rt>,
        }
        let l: List = serde_json::from_str(&self.call(&["list", "runtimes", "-j"], 60)?)
            .map_err(|e| format!("cannot parse the runtime list: {e}"))?;
        Ok(l.runtimes.into_iter().filter(|r| r.is_available && (r.platform == "iOS" || r.identifier.contains("iOS"))).collect())
    }

    /// 按写的版本、机型（不写的取最新、配置机型）挑运行时，组合不了时说明缺什么（CASE-HOLD-13、CASE-HOLD-14）。
    /// 同一版本有只为单一机型准备的运行时（如 iOS-27-2-iPhone19-4）时，选支持机型最多的通用那个。
    fn pick_runtime(&self, spec: &DeviceSpec) -> Result<String, SpecError> {
        let all = self.ios_runtimes().map_err(SpecError::Tool)?;
        Ok(pick(&all, spec, &self.model)?.identifier.clone())
    }
}

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

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect()
}

fn pick<'a>(all: &'a [Rt], spec: &DeviceSpec, default_model: &str) -> Result<&'a Rt, SpecError> {
    let mut installed: Vec<&str> = all.iter().map(|r| r.version.as_str()).collect();
    installed.sort_by_key(|v| version_key(v));
    installed.dedup();
    let list = installed.join(", ");
    let candidates: Vec<&Rt> = match &spec.version {
        Some(v) => all.iter().filter(|r| r.version == *v).collect(),
        None => all.iter().collect(),
    };
    if let (Some(v), true) = (&spec.version, candidates.is_empty()) {
        let head = format!("no iOS {v} simulator runtime on this Mac; installed: {list}.");
        return Err(SpecError::Unfit(match installed.iter().find(|i| i.starts_with(&format!("{v}."))) {
            Some(full) => format!("{head} Write the full version, e.g. ios-sim@{full}"),
            None => format!("{head} To install: xcodebuild -downloadPlatform iOS -buildVersion {v}"),
        }));
    }
    let model = spec.model.as_deref().unwrap_or(default_model);
    let supports = |r: &Rt| r.supported.is_empty() || r.supported.iter().any(|d| d.name == model);
    let fit = candidates.iter().copied().filter(|r| supports(r)).max_by_key(|r| (version_key(&r.version), r.supported.len()));
    if let Some(r) = fit {
        return Ok(r);
    }
    let whose = if spec.model.is_none() { " (the default model, [ios-sim] model in config.toml)" } else { "" };
    let Some(v) = &spec.version else {
        return Err(SpecError::Unfit(format!("no iOS simulator runtime on this Mac supports {model}{whose}; installed: {list}")));
    };
    let mut names: Vec<&str> = vec![];
    for n in candidates.iter().flat_map(|r| r.supported.iter().map(|d| d.name.as_str())) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let first = names.first().copied().unwrap_or("iPhone 17 Pro");
    Err(SpecError::Unfit(format!("iOS {v} does not support {model}{whose}; it supports: {}. E.g. ios-sim@{v}:{first}", names.join(", "))))
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
    /// 不写版本、机型时不查：沿用最新运行时和配置机型，与以前一样，不给每次借用多加一次 simctl 调用。
    fn check(&self, spec: &DeviceSpec) -> Result<(), SpecError> {
        if spec.is_default() {
            return Ok(());
        }
        self.pick_runtime(spec).map(|_| ())
    }

    fn create(&self, name: &str, spec: &DeviceSpec) -> Result<String, String> {
        let rt = self.pick_runtime(spec).map_err(|e| match e {
            SpecError::Unfit(m) | SpecError::Tool(m) => m,
        })?;
        let model = spec.model.as_deref().unwrap_or(&self.model);
        let udid = self.call(&["create", name, model, &rt], 120)?.trim().to_string();
        if udid.is_empty() {
            return Err("simctl create returned no UDID".into());
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
        let l: List =
            serde_json::from_str(&self.call(&["list", "devices", "-j"], 60)?).map_err(|e| format!("cannot parse the device list: {e}"))?;
        Ok(l.devices.into_values().flatten().filter(|d| d.name.starts_with("aq-")).map(|d| (d.name, d.udid)).collect())
    }
}
