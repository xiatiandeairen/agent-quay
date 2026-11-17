//! 真机探测：iOS 用 `devicectl list devices`，Android 用 `adb devices -l`。只读，不改设备状态；解析和判定在 aq-core::devices。
//! 验收测试用 AQ_DEVICECTL、AQ_ADB 指向假脚本。

use std::time::Duration;

use aq_core::devices::{parse_adb, parse_devicectl, Seen};
use aq_core::model::DeviceType;
use aq_core::ports::RealDevices;

use crate::cmd::{run, Call};
use crate::emu::sdk_tool;

pub struct Probe {
    pub devicectl: Vec<String>,
    pub adb: Vec<String>,
}

impl Probe {
    pub fn from_env() -> Probe {
        let devicectl = std::env::var("AQ_DEVICECTL").map(|s| vec![s]).unwrap_or_else(|_| vec!["xcrun".into(), "devicectl".into()]);
        let adb = vec![std::env::var("AQ_ADB").unwrap_or_else(|_| sdk_tool("platform-tools/adb"))];
        Probe { devicectl, adb }
    }

    /// 跑一次工具拿标准输出；`what` 是给人看的工具名和缺失时的提示。
    fn call(base: &[String], args: &[&str], what: &str) -> Result<String, String> {
        let mut argv = base.to_vec();
        argv.extend(args.iter().map(|s| s.to_string()));
        let o = run(&Call { argv: &argv, cwd: None, env: &[], stdin: None, timeout: Duration::from_secs(30) })
            .map_err(|e| format!("{what} 运行失败：{e}"))?;
        if o.status != 0 {
            return Err(format!("{what} 出错（退出码 {}）：{}", o.status, o.stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }
}

impl RealDevices for Probe {
    fn scan(&self, t: DeviceType) -> Result<Vec<Seen>, String> {
        match t {
            DeviceType::IosDevice => {
                let out =
                    Self::call(&self.devicectl, &["list", "devices", "-q", "--json-output", "/dev/stdout"], "devicectl（需要 Xcode）")?;
                parse_devicectl(&out)
            }
            DeviceType::AndroidDevice => {
                Ok(parse_adb(&Self::call(&self.adb, &["devices", "-l"], "adb（需要 Android SDK platform-tools）")?))
            }
            _ => Ok(vec![]),
        }
    }
}
