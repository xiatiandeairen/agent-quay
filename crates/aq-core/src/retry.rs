//! retry_after_s：建议多久后再查，10–300 秒。只是轮询建议，不是完成时间承诺。

pub const MIN_S: i64 = 10;
pub const MAX_S: i64 = 300;

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
    }
}
