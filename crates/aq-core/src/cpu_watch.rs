//! 运行中纠正重任务：按轻任务放行的执行，运行满一个窗口且窗口内平均 ≥ 2 核，就改记为重任务。
//!
//! 输入是每次采样时进程组成员及其后代各自的累计 CPU 时间。进程在两次采样之间退出，它最后一段 CPU 时间会漏算，
//! 所以测到的是偏低的下界；只用来「轻 → 重」，不会把真轻的任务误判成重。

use std::collections::{HashMap, VecDeque};

use crate::estimate::HEAVY_CORES;

#[derive(Debug)]
pub struct CpuWatch {
    window_ms: i64,
    /// 上一次采样时各进程的累计 CPU 纳秒。
    prev: HashMap<i64, u64>,
    /// (采样时刻 ms, 到这次为止累计算到的 CPU 纳秒)，只留最近一个窗口再多一个点。
    points: VecDeque<(i64, u64)>,
    total_ns: u64,
    first_ms: Option<i64>,
}

impl CpuWatch {
    pub fn new(window_ms: i64) -> Self {
        CpuWatch { window_ms: window_ms.max(1), prev: HashMap::new(), points: VecDeque::new(), total_ns: 0, first_ms: None }
    }

    /// 记一次采样；`procs` 是 (pid, 累计 CPU 纳秒)。第一次采样只建立基线。
    pub fn observe(&mut self, now_ms: i64, procs: &[(i64, u64)]) {
        let first = self.first_ms.is_none();
        self.first_ms.get_or_insert(now_ms);
        let mut next = HashMap::with_capacity(procs.len());
        for &(pid, ns) in procs {
            if !first {
                let before = self.prev.get(&pid).copied().unwrap_or(0);
                self.total_ns += ns.saturating_sub(before);
            }
            next.insert(pid, ns);
        }
        self.prev = next;
        self.points.push_back((now_ms, self.total_ns));
        // 保留一个不晚于 now - window 的点作窗口起点
        while self.points.len() >= 2 && self.points.get(1).is_some_and(|p| p.0 <= now_ms - self.window_ms) {
            self.points.pop_front();
        }
    }

    /// 最近一个完整窗口里的平均核数；运行还不满一个窗口时为 None。
    pub fn window_cores(&self) -> Option<f64> {
        let (t0, c0) = *self.points.front()?;
        let (t1, c1) = *self.points.back()?;
        let span = t1 - t0;
        (span >= self.window_ms).then(|| (c1 - c0) as f64 / 1e6 / span as f64)
    }

    /// 窗口内平均达到重任务线。
    pub fn is_heavy(&self) -> bool {
        self.window_cores().is_some_and(|c| c >= HEAVY_CORES)
    }

    /// 整个运行期间按采样算到的平均核数；结束时与 rusage 取大者（rusage 漏掉没被等待的孙进程）。
    pub fn overall_cores(&self) -> Option<f64> {
        let (t0, t1) = (self.first_ms?, self.points.back()?.0);
        (t1 > t0).then(|| self.total_ns as f64 / 1e6 / (t1 - t0) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    // CASE-EST-10
    #[test]
    fn heavy_after_full_window_of_four_cores() {
        let mut w = CpuWatch::new(2000);
        for i in 0..=4 {
            let t = i * 500;
            w.observe(t, &[(10, 0), (11, 2 * S * t as u64 / 1000), (12, 2 * S * t as u64 / 1000)]);
            if t < 2000 {
                assert!(!w.is_heavy(), "不满一个窗口不判（t={t}）");
            }
        }
        assert!((w.window_cores().unwrap() - 4.0).abs() < 0.01);
        assert!(w.is_heavy());
    }

    // CASE-EST-10
    #[test]
    fn one_core_stays_light() {
        let mut w = CpuWatch::new(2000);
        for i in 0..=10 {
            w.observe(i * 500, &[(10, S * i as u64 / 2)]);
        }
        assert!((w.window_cores().unwrap() - 1.0).abs() < 0.01);
        assert!(!w.is_heavy());
    }

    // CASE-EST-10
    #[test]
    fn short_lived_processes_counted_by_pid_turnover() {
        // 每 500ms 换一批 4 个新进程，每个出现时已累计 0.5 秒：仍测到约 4 核
        let mut w = CpuWatch::new(2000);
        for i in 0..=8i64 {
            let procs: Vec<(i64, u64)> = (0..4).map(|k| (100 + i * 4 + k, S / 2)).collect();
            w.observe(i * 500, &procs);
        }
        assert!(w.window_cores().unwrap() >= 3.9);
    }

    // CASE-EST-10
    #[test]
    fn window_slides_so_old_burst_fades() {
        let mut w = CpuWatch::new(2000);
        let mut cum = 0u64;
        for i in 0..=20i64 {
            if i <= 4 {
                cum += 2 * S; // 前 2 秒每 500ms 4 核秒
            }
            w.observe(i * 500, &[(10, cum)]);
        }
        assert!(w.window_cores().unwrap() < 0.01, "早先的高峰滑出窗口");
    }
}
