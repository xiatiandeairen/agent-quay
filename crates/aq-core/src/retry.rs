//! `next` 里的 `--wait` 秒数：建议这次等多久，10–100 秒。只是等待建议，不是完成时间承诺。
//! 上限 100 = aq 的默认 `--wait`，配 agent 的 Bash 默认 120 秒超时；更长会被 Bash 杀掉（会话记录里 `--wait 560` 被杀过）。
//! 改成可直接执行的命令，是因为会话记录里 612 次返回的秒数没有一次被照着用（2026-11-20 分析）。

pub const MIN_S: i64 = 10;
pub const MAX_S: i64 = 100;

/// 接着等的命令。
pub fn next(id: &str, wait_s: i64) -> String {
    format!("aq status {id} --wait {wait_s}")
}

/// 运行中：预计剩余时间。
pub fn running(est_s: i64, elapsed_s: i64) -> i64 {
    let left = est_s - elapsed_s;
    if left <= 0 {
        return 30;
    }
    left.clamp(MIN_S, MAX_S)
}

/// 排队中：前面运行中任务最早结束的剩余时间加上自己的耗时的一半；不知道时 60 秒。
pub fn queued(soonest_free_s: Option<i64>, own_est_s: Option<i64>) -> i64 {
    let base = soonest_free_s.unwrap_or(60) + own_est_s.unwrap_or(0) / 2;
    base.clamp(MIN_S, MAX_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_in_range() {
        for (e, el) in [(0, 0), (5, 0), (100_000, 0), (100, 99), (100, 500)] {
            let r = running(e, el);
            assert!((MIN_S..=MAX_S).contains(&r));
        }
        assert_eq!(queued(None, None), 60);
        assert_eq!(queued(Some(10_000), None), MAX_S);
        assert_eq!(queued(Some(0), Some(0)), MIN_S);
        assert_eq!(next("ex_1", 60), "aq status ex_1 --wait 60");
    }
}
