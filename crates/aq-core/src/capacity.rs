//! 机器容量与时间阈值（由 config.toml 和内置默认值得出）。

use crate::model::DeviceType;

#[derive(Debug, Clone, PartialEq)]
pub struct Capacity {
    pub heavy_slots: i64,
    pub mem_mb: i64,
    pub disk_floor_mb: i64,
    pub net_slots: i64,
    pub ios_sim_mem_mb: i64,
    pub android_emu_mem_mb: i64,
    pub browser_mem_mb: i64,
    pub ios_devices: Vec<String>,
    pub android_devices: Vec<String>,
}

impl Capacity {
    pub fn sim_mem_mb(&self, t: DeviceType) -> i64 {
        match t {
            DeviceType::IosSim => self.ios_sim_mem_mb,
            DeviceType::AndroidEmu => self.android_emu_mem_mb,
            DeviceType::Browser => self.browser_mem_mb,
            // desktop 只是 agent-desktop 的会话名，不起进程，不占内存
            DeviceType::Desktop | DeviceType::IosDevice | DeviceType::AndroidDevice => 0,
        }
    }
    pub fn real_devices(&self, t: DeviceType) -> &[String] {
        match t {
            DeviceType::IosDevice => &self.ios_devices,
            DeviceType::AndroidDevice => &self.android_devices,
            _ => &[],
        }
    }
}

impl Default for Capacity {
    fn default() -> Self {
        Capacity {
            heavy_slots: 2,
            mem_mb: 28 * 1024,
            disk_floor_mb: 50 * 1024,
            net_slots: 3,
            ios_sim_mem_mb: 3 * 1024,
            android_emu_mem_mb: 4 * 1024,
            // 一个无窗口 Chrome 会话实测约 1.2G（进程 RSS 加总，共享内存重复计入，偏高），按 1G 记
            browser_mem_mb: 1024,
            ios_devices: vec![],
            android_devices: vec![],
        }
    }
}

/// 时间阈值。验收测试通过环境变量缩短，正式值见各字段注释。
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    /// 闲置多久收回借用：2 小时。
    pub idle_reclaim_ms: i64,
    /// 超时下限 30 分钟、上限 2 小时。
    pub timeout_min_s: i64,
    pub timeout_max_s: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { idle_reclaim_ms: 2 * 3600 * 1000, timeout_min_s: 1800, timeout_max_s: 7200 }
    }
}
