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

str_enum!(ReclaimReason { OwnerExit => "owner_exit", Idle => "idle", MaxHold => "max_hold" });

/// 退出码：一个码只对应调用方的一种下一步，分组原因见 docs/design.md 3.5（CASE-STATUS-3）。
pub mod exit {
    /// 完成，继续。
    pub const DONE: i32 = 0;
    /// 命令失败（failed、timeout），读日志改代码。
    pub const FAILED: i32 = 1;
    /// 还没完，接着等。
    pub const PENDING: i32 = 2;
    /// 写法错，改调用。
    pub const USAGE: i32 = 3;
    /// 可重试，原样再提交或重新借。
    pub const RETRY: i32 = 4;
    /// 需要人，把 message 转告用户。
    pub const HUMAN: i32 = 5;

    /// 多个对象合成一个退出码：取最需要处理的，5 > 3 > 1 > 4 > 2 > 0，不取数值最大（CASE-STATUS-2）。
    pub fn worst(codes: impl IntoIterator<Item = i32>) -> i32 {
        codes.into_iter().max_by_key(|c| rank(*c)).unwrap_or(DONE)
    }

    fn rank(code: i32) -> u8 {
        match code {
            HUMAN => 5,
            USAGE => 4,
            FAILED => 3,
            RETRY => 2,
            PENDING => 1,
            _ => 0,
        }
    }
}

impl Reason {
    pub fn exit_code(self) -> i32 {
        match self {
            Reason::Usage => exit::USAGE,
            Reason::DeviceError | Reason::Busy => exit::RETRY,
            Reason::Unschedulable | Reason::DeviceUnavailable => exit::HUMAN,
        }
    }
}

impl ExecStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, ExecStatus::Queued | ExecStatus::Running)
    }
    pub fn exit_code(self) -> i32 {
        match self {
            ExecStatus::Ok => exit::DONE,
            ExecStatus::Failed | ExecStatus::Timeout => exit::FAILED,
            ExecStatus::Queued | ExecStatus::Running => exit::PENDING,
            ExecStatus::Interrupted | ExecStatus::Cancelled => exit::RETRY,
            // 落库的 rejected 执行只有 unschedulable 一种（与 view::exec_json 一致）
            ExecStatus::Rejected => Reason::Unschedulable.exit_code(),
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
            HoldStatus::Held | HoldStatus::Released => exit::DONE,
            HoldStatus::Waiting => exit::PENDING,
            HoldStatus::Reclaimed => exit::RETRY,
            // 落库的 rejected 借用只有 device_error 一种（与 view::hold_json 一致）
            HoldStatus::Rejected => Reason::DeviceError.exit_code(),
        }
    }
}

impl DeviceType {
    /// 虚拟设备借时新建、还时删除，按内存供给；真机来自固定清单、独占。
    /// browser 是一个 agent-browser 会话（独立 Chrome），desktop 是一个 agent-desktop 会话名，都可多个 agent 同时借。
    pub fn is_virtual(self) -> bool {
        !matches!(self, DeviceType::IosDevice | DeviceType::AndroidDevice)
    }
    /// aq 能探测是否在用的设备（看进程参数，design.md 3.3）；浏览器、桌面会话不探测。
    pub fn usage_probed(self) -> bool {
        !matches!(self, DeviceType::Browser | DeviceType::Desktop)
    }
    pub fn is_android(self) -> bool {
        matches!(self, DeviceType::AndroidEmu | DeviceType::AndroidDevice)
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
    /// 命令原文：参数里带着借到的设备 id 时刷新该借用（CASE-RECLAIM-10）。
    pub argv: Vec<String>,
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
        use ExecStatus as E;
        use HoldStatus as H;
        let exec = [
            (E::Ok, 0),
            (E::Failed, 1),
            (E::Timeout, 1),
            (E::Queued, 2),
            (E::Running, 2),
            (E::Interrupted, 4),
            (E::Cancelled, 4),
            (E::Rejected, 5),
        ];
        for (s, c) in exec {
            assert_eq!(s.exit_code(), c, "{s:?}");
        }
        let hold = [(H::Held, 0), (H::Released, 0), (H::Waiting, 2), (H::Reclaimed, 4), (H::Rejected, 4)];
        for (s, c) in hold {
            assert_eq!(s.exit_code(), c, "{s:?}");
        }
        let reason =
            [(Reason::Usage, 3), (Reason::DeviceError, 4), (Reason::Busy, 4), (Reason::Unschedulable, 5), (Reason::DeviceUnavailable, 5)];
        for (r, c) in reason {
            assert_eq!(r.exit_code(), c, "{r:?}");
        }
    }

    // CASE-STATUS-2
    #[test]
    fn worst_exit_code_by_priority() {
        use exit::worst;
        assert_eq!(worst([2, 1]), 1, "running + failed");
        assert_eq!(worst([4, 5]), 5, "cancelled + unschedulable");
        assert_eq!(worst([0, 3]), 3, "ok + usage");
        assert_eq!(worst([5, 3, 1, 4, 2, 0]), 5);
        assert_eq!(worst([3, 1, 4, 2, 0]), 3);
        assert_eq!(worst([1, 4, 2, 0]), 1);
        assert_eq!(worst([4, 2, 0]), 4);
        assert_eq!(worst([2, 0]), 2);
        assert_eq!(worst([]), 0);
    }
}
