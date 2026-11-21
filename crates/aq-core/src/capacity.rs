//! 机器容量与时间阈值（由 config.toml 和内置默认值得出）。

use crate::model::DeviceType;

#[derive(Debug, Clone, PartialEq)]
pub struct Capacity {
    pub heavy_slots: i64,
    pub mem_mb: i64,
    pub disk_floor_mb: i64,
    pub net_slots: i64,
    /// 同时存在的 iOS 模拟器加 Android 实例最多几台（浏览器会话不算，CASE-HOLD-12）。
    pub sim_slots: i64,
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

/// 没配置 `mem` 时 aq 可分配的内存：物理内存的 3/4，其余留给系统和人自己开的程序（CASE-QUEUE-7）。
/// 3/4 是假设：36G 的机器得到 27G，与原先写死的 28G 相当；在 16G、64G 的机器上分别得到 12G、48G。
/// 怎么验证：按真实负载回放（CASE-LOAD-1）时内存压力不进 warn。读不到物理内存时退回 28G。
pub fn default_mem_mb(physical_mb: i64) -> i64 {
    if physical_mb > 0 {
        physical_mb * 3 / 4
    } else {
        Capacity::default().mem_mb
    }
}

/// 没配置 `sim_slots` 时的模拟器台数上限：物理内存每 8G 一台，至少 1 台（CASE-QUEUE-7）。读不到物理内存时 4 台。
/// 8G/台是假设，依据（agent-verify 2026-11-17 实测，本机 36G）：同时 4 台、内存占用 80% 时人没感觉卡（批 2），
/// 到 7 台、94% 时一行 python 要约 50 秒（批 3）；3 台同时启动合计约占 14G（a3/），即每台约 4.5G，再给系统和人的程序留一半余量。
/// 36G → 4 台，16G → 2 台，64G → 8 台。怎么验证：在 16G、64G 的机器上各连借到上限，看压力是否进 warn、人是否感到卡。
pub fn default_sim_slots(physical_mb: i64) -> i64 {
    if physical_mb > 0 {
        (physical_mb / (8 * 1024)).max(1)
    } else {
        Capacity::default().sim_slots
    }
}

impl Default for Capacity {
    fn default() -> Self {
        Capacity {
            heavy_slots: 2,
            mem_mb: 28 * 1024,
            disk_floor_mb: 50 * 1024,
            net_slots: 3,
            sim_slots: 4,
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
    /// 设备连续多久没人用就收回借用：30 分钟（design.md 3.3）。
    pub idle_reclaim_ms: i64,
    /// 借出多久不管是否在用都收回：4 小时（CASE-RECLAIM-9）。
    pub max_hold_ms: i64,
    /// 超时下限 30 分钟、上限 2 小时。
    pub timeout_min_s: i64,
    pub timeout_max_s: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { idle_reclaim_ms: 30 * 60 * 1000, max_hold_ms: 4 * 3600 * 1000, timeout_min_s: 1800, timeout_max_s: 7200 }
    }
}
