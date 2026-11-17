//! 命令级错误 → 一行 JSON + 退出码。usage → rejected 3；busy → 1（数据未改，稍后重试）；其他 → 1。

use aq_store::StoreError;

#[derive(Debug)]
pub enum AppError {
    Usage(String),
    Busy,
    Fatal(String),
}

impl From<StoreError> for AppError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Busy => AppError::Busy,
            other => AppError::Fatal(other.to_string()),
        }
    }
}

impl AppError {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            AppError::Usage(m) => serde_json::json!({"status": "rejected", "reason": "usage", "message": m}),
            AppError::Busy => serde_json::json!({"reason": "busy", "message": "数据库忙：等写锁超过 5 秒，数据未改，请稍后重试"}),
            AppError::Fatal(m) => serde_json::json!({"message": m}), // 环境错误（目录不可写等）：文档未定义 reason，只给说明
        }
    }
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Usage(_) => 3,
            _ => 1,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
