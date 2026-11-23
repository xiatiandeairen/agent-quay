//! 设备参数 `<类型>[@<版本>][:<机型>]`：模拟器可以指定系统版本和机型，不写的取默认（iOS 最新版本、配置机型；Android 配置 AVD 的镜像和机型）。
//! aq 只把写的和默认的组合起来，组合不了就在提交时拒绝，由调用方决定换什么（CASE-HOLD-13～15、CASE-HOLD-19）。

use serde::{Deserialize, Serialize};

use crate::model::DeviceType;

/// 一台模拟器指定的版本和机型；都为 None 时就是默认。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl DeviceSpec {
    pub fn is_default(&self) -> bool {
        self.version.is_none() && self.model.is_none()
    }
}

/// 只有模拟器能指定版本和机型；真机、浏览器、桌面会话是什么就是什么。
pub fn takes_spec(t: DeviceType) -> bool {
    matches!(t, DeviceType::IosSim | DeviceType::AndroidEmu)
}

fn example(t: DeviceType) -> &'static str {
    match t {
        DeviceType::AndroidEmu => "android-emu@35:pixel_7",
        _ => "ios-sim@26.2:iPhone 17 Pro",
    }
}

const TYPES: &str = "one of ios-sim, android-emu, ios-device, android-device, browser, desktop";

/// 解析一个设备参数。错误信息直接给调用方看（usage）。
pub fn parse_device(arg: &str) -> Result<(DeviceType, DeviceSpec), String> {
    let (head, model) = match arg.split_once(':') {
        Some((h, m)) => (h, Some(m)),
        None => (arg, None),
    };
    let mut parts = head.split('@');
    let ty = parts.next().unwrap_or_default();
    let version = parts.next();
    if parts.next().is_some() {
        return Err(format!("more than one @ in {arg}; name one device per argument, e.g. ios-sim@26.2 ios-sim@27.2"));
    }
    let t = DeviceType::parse(ty).ok_or_else(|| format!("unknown device type: {ty} ({TYPES})"))?;
    if (version.is_some() || model.is_some()) && !takes_spec(t) {
        return Err(format!("{t} does not take @version or :model; only ios-sim and android-emu do"));
    }
    if let Some(v) = version {
        if v.is_empty() {
            return Err(format!("empty version in {arg}; write it like {}", example(t)));
        }
        if !v.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(format!("version {v} in {arg} is not a number like 26.2 or 35"));
        }
    }
    if model.is_some_and(|m| m.trim().is_empty()) {
        return Err(format!("empty model in {arg}; write it like {}", example(t)));
    }
    Ok((t, DeviceSpec { version: version.map(String::from), model: model.map(|m| m.trim().to_string()) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> (DeviceType, Option<String>, Option<String>) {
        let (t, sp) = parse_device(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        (t, sp.version, sp.model)
    }
    fn v(s: &str) -> Option<String> {
        Some(s.into())
    }

    // CASE-HOLD-13
    #[test]
    fn parses_version_and_model() {
        assert_eq!(ok("ios-sim"), (DeviceType::IosSim, None, None));
        assert_eq!(ok("ios-sim@26.2"), (DeviceType::IosSim, v("26.2"), None));
        assert_eq!(ok("ios-sim@26.2:iPhone 16 Pro"), (DeviceType::IosSim, v("26.2"), v("iPhone 16 Pro")));
        assert_eq!(ok("ios-sim:iPhone 18 Pro"), (DeviceType::IosSim, None, v("iPhone 18 Pro")));
        assert_eq!(ok("android-emu@37.0:pixel_7"), (DeviceType::AndroidEmu, v("37.0"), v("pixel_7")));
        assert_eq!(ok("browser"), (DeviceType::Browser, None, None));
    }

    // CASE-HOLD-15
    #[test]
    fn rejects_bad_forms() {
        let err = |s: &str| parse_device(s).err().unwrap_or_else(|| panic!("{s} should fail"));
        assert!(err("ios-device@27").contains("ios-device does not take @version"));
        assert!(err("browser:x").contains("browser does not take"));
        assert!(err("desktop@1").contains("desktop does not take"));
        assert!(err("ios-sim@").contains("empty version"));
        assert!(err("ios-sim@26.2:").contains("empty model"));
        assert!(err("ios-sim@26.2@27.2").contains("more than one @"));
        assert!(err("ios-sim@abc").contains("not a number"));
        assert!(err("iphone").contains("unknown device type"));
    }

    #[test]
    fn default_spec_is_not_serialized() {
        assert_eq!(serde_json::to_string(&DeviceSpec::default()).unwrap_or_default(), "{}");
    }
}
