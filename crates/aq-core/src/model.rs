//! 状态与数据类型。
//! 枚举的字符串值与数据库 CHECK、对外 JSON 一致，改动前先对齐文档。

use serde::{Deserialize, Serialize};

macro_rules! str_enum {
    ($name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str { match self { $($name::$var => $s),+ } }
            pub fn parse(s: &str) -> Option<Self> { match s { $($s => Some($name::$var),)+ _ => None } }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

str_enum!(ExecStatus {
    Queued => "queued", Running => "running", Ok => "ok", Failed => "failed",
    Timeout => "timeout", Interrupted => "interrupted", Cancelled => "cancelled", Rejected => "rejected",
});

str_enum!(HoldStatus {
    Waiting => "waiting", Held => "held", Released => "released",
    Reclaimed => "reclaimed", Rejected => "rejected",
});

str_enum!(DeviceType {
    IosSim => "ios-sim", AndroidEmu => "android-emu",
    IosDevice => "ios-device", AndroidDevice => "android-device",
    Browser => "browser", Desktop => "desktop",
});

str_enum!(Reason {
    Usage => "usage", Unschedulable => "unschedulable", DeviceError => "device_error", Busy => "busy",
    DeviceUnavailable => "device_unavailable",
});

str_enum!(ReclaimReason { OwnerExit => "owner_exit", Idle => "idle" });

impl ExecStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, ExecStatus::Queued | ExecStatus::Running)
    }
    /// 退出码映射，见「执行对象」状态表。
    pub fn exit_code(self) -> i32 {
        match self {
            ExecStatus::Ok => 0,
            ExecStatus::Queued | ExecStatus::Running => 2,
            ExecStatus::Rejected => 3,
            _ => 1,
        }
    }
}

impl HoldStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, HoldStatus::Released | HoldStatus::Reclaimed | HoldStatus::Rejected)
    }
    /// waiting 之外都算「已有结论」，status --wait 等到这里为止。
    pub fn is_settled(self) -> bool {
        self != HoldStatus::Waiting
    }
    pub fn exit_code(self) -> i32 {
        match self {
            HoldStatus::Waiting => 2,
            HoldStatus::Rejected => 3,
            _ => 0,
        }
    }
}

impl DeviceType {
    /// 虚拟设备借时新建、还时删除，按内存供给；真机来自固定清单、独占。
    /// browser 是一个 agent-browser 会话（独立 Chrome），desktop 是一个 agent-desktop 会话名，都可多个 agent 同时借。
    pub fn is_virtual(self) -> bool {
        !matches!(self, DeviceType::IosDevice | DeviceType::AndroidDevice)
    }
}

/// 一次执行在放行时记账的资源量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Resources {
    pub heavy: bool,
    pub mem_mb: i64,
    pub disk_mb: i64,
    pub uses_net: bool,
}

/// `holds.request` 的内容：`{"devices":[...],"accounts":[...],"exclude":[...]}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldRequest {
    pub devices: Vec<DeviceType>,
    pub accounts: Vec<String>,
    /// 提交时声明了但不可用的真机 id，分配时跳过（见 devices.rs）。为空时不写进 JSON。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// 进程身份：进程号 + 启动时间（Unix 毫秒），避免进程号复用后误判。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcId {
    pub pid: i64,
    pub started_at: i64,
}

impl ProcId {
    /// 借用者未识别（文档 2.7 策略①）：不核对是否退出，只按闲置收回。
    pub const UNKNOWN: ProcId = ProcId { pid: 0, started_at: 0 };
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecView {
    pub id: String,
    pub status: ExecStatus,
    pub submitted_at: i64,
    pub started_at: Option<i64>,
    pub res: Option<Resources>,
    pub timeout_s: Option<i64>,
    pub pgid: Option<ProcId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoldView {
    pub id: String,
    pub status: HoldStatus,
    pub requested_at: i64,
    pub acquired_at: Option<i64>,
    pub request: HoldRequest,
    pub owner: ProcId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub hold_id: String,
    pub seq: i64,
    pub dtype: DeviceType,
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub account: String,
    pub hold_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_strings_roundtrip() {
        for s in ExecStatus::ALL {
            assert_eq!(ExecStatus::parse(s.as_str()), Some(*s));
        }
        for s in DeviceType::ALL {
            assert_eq!(DeviceType::parse(s.as_str()), Some(*s));
        }
        assert_eq!(DeviceType::parse("pad"), None);
    }

    // CASE-STATUS-3
    #[test]
    fn exit_codes_follow_doc() {
        assert_eq!(ExecStatus::Ok.exit_code(), 0);
        assert_eq!(ExecStatus::Failed.exit_code(), 1);
        assert_eq!(ExecStatus::Cancelled.exit_code(), 1);
        assert_eq!(ExecStatus::Queued.exit_code(), 2);
        assert_eq!(ExecStatus::Rejected.exit_code(), 3);
        assert_eq!(HoldStatus::Reclaimed.exit_code(), 0);
        assert_eq!(HoldStatus::Waiting.exit_code(), 2);
    }
}
