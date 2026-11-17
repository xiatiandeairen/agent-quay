//! 时钟：*_at 用真实时间；超时用 CLOCK_UPTIME_RAW（机器睡眠不计入，CASE-RUN-7）。

use std::time::{SystemTime, UNIX_EPOCH};

use aq_core::ports::Clock;

pub struct RealClock;

/// 验收用（CASE-RUN-7）：AQ_TEST_WALL_SKEW_FILE 指向的文件里写一个毫秒数，真实时间加上它，模拟机器睡眠后真实时间跳过一段、
/// 而 CLOCK_UPTIME_RAW 不动。正式环境不设。
fn test_skew_ms() -> i64 {
    std::env::var_os("AQ_TEST_WALL_SKEW_FILE")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

impl Clock for RealClock {
    fn now_ms(&self) -> i64 {
        let real = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        real + test_skew_ms()
    }

    fn uptime_ms(&self) -> i64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: ts 是有效的可写 timespec。
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &raw mut ts) };
        if rc != 0 {
            return 0;
        }
        ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
    }
}
