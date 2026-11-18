//! 资源评估：实测 → AI 评估缓存 → 首次见到（按轻任务，不等 AI）。

use serde::Deserialize;

use crate::capacity::{Capacity, Limits};
use crate::model::Resources;

/// 一次已结束执行的实测（只取终态执行，见 CASE-EST-5）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub peak_mem_mb: i64,
    pub disk_delta_mb: i64,
    pub duration_s: i64,
    pub cpu_avg_cores: f64,
    /// 中断、撤销的记录不参与时长估计。
    pub usable_for_duration: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Measured,
    Ai,
    FirstSeen,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Measured => "measurements",
            Source::Ai => "AI estimate",
            Source::FirstSeen => "first seen (booked as light)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub res: Resources,
    pub est_s: i64,
    pub timeout_s: i64,
    pub source: Source,
}

const HISTORY: usize = 20;
/// 平均占用不低于这么多核算重任务（估计与运行中纠正共用）。
pub const HEAVY_CORES: f64 = 2.0;
const DECAY: f64 = 0.8;

/// 加权分位数：按值排序后累计权重，第一个累计 ≥ q × 总权重的值。
pub fn weighted_quantile(pairs: &[(f64, f64)], q: f64) -> Option<f64> {
    let mut v: Vec<(f64, f64)> = pairs.iter().copied().filter(|(_, w)| *w > 0.0).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|p| p.1).sum();
    let mut acc = 0.0;
    for (x, w) in &v {
        acc += w;
        if acc >= q * total - 1e-9 {
            return Some(*x);
        }
    }
    v.last().map(|p| p.0)
}

/// 第 k 新（k 从 0 起）的权重 0.8^k。
fn weighted<F: Fn(&Sample) -> f64>(newest_first: &[Sample], f: F) -> Vec<(f64, f64)> {
    newest_first.iter().take(HISTORY).enumerate().map(|(k, s)| (f(s), DECAY.powi(k as i32))).collect()
}

/// 内存、磁盘：1 次取峰值 × 1.2；≥ 2 次取加权 p90 × 1.1。
fn upper(newest_first: &[Sample], f: impl Fn(&Sample) -> f64) -> i64 {
    let vals: Vec<f64> = newest_first.iter().take(HISTORY).map(f).collect();
    upper_of(&vals)
}

fn upper_of(newest_first: &[f64]) -> i64 {
    let v = match newest_first {
        [] => 0.0,
        [x] => x * 1.2,
        _ => {
            let pairs: Vec<(f64, f64)> = newest_first.iter().enumerate().map(|(k, x)| (*x, DECAY.powi(k as i32))).collect();
            weighted_quantile(&pairs, 0.9).unwrap_or(0.0) * 1.1
        }
    };
    v.ceil() as i64
}

/// 一次实测的磁盘增量，以及它是否可靠（运行期间没有别的写入来源）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiskSample {
    pub delta_mb: i64,
    pub reliable: bool,
}

/// 磁盘估计（CASE-EST-17）：有可靠实测时只用可靠的，算法同内存；只有不可靠的时取其中最小值
/// （别的写入来源只会往上加，每个不可靠值都是偏大的上限）；没有实测返回 None。
pub fn disk_upper(newest_first: &[DiskSample]) -> Option<i64> {
    let ok: Vec<f64> = newest_first.iter().filter(|s| s.reliable).take(HISTORY).map(|s| s.delta_mb.max(0) as f64).collect();
    if !ok.is_empty() {
        return Some(upper_of(&ok));
    }
    newest_first.iter().map(|s| s.delta_mb.max(0)).min()
}

/// 加权后 ≥ 2 核的执行占总权重一半以上。
fn is_heavy(newest_first: &[Sample]) -> bool {
    let w = weighted(newest_first, |s| s.cpu_avg_cores);
    let total: f64 = w.iter().map(|p| p.1).sum();
    let heavy: f64 = w.iter().filter(|p| p.0 >= HEAVY_CORES).map(|p| p.1).sum();
    total > 0.0 && heavy > total / 2.0
}

/// 时长：加权中位数；排除中断、撤销和不到中位数 10% 的快速失败（CASE-EST-5）。
fn duration(newest_first: &[Sample]) -> Option<i64> {
    let usable: Vec<Sample> = newest_first.iter().take(HISTORY).copied().filter(|s| s.usable_for_duration).collect();
    let all = weighted(&usable, |s| s.duration_s as f64);
    let median = weighted_quantile(&all, 0.5)?;
    let kept: Vec<(f64, f64)> = all.into_iter().filter(|(d, _)| *d >= median * 0.1).collect();
    weighted_quantile(&kept, 0.5).map(|d| d.round() as i64)
}

pub fn timeout_for(est_s: i64, lim: &Limits) -> i64 {
    (est_s.saturating_mul(3)).clamp(lim.timeout_min_s, lim.timeout_max_s)
}

/// 有至少 1 次实测就用实测。`uses_net` 测不到，沿用 AI 评估（没有则为否）。
pub fn from_history(newest_first: &[Sample], ai_uses_net: Option<bool>, lim: &Limits) -> Option<Estimate> {
    if newest_first.is_empty() {
        return None;
    }
    // 重任务：加权后多数次执行平均 ≥ 2 核。判轻实重（如增量为主的命令这次是全量）由运行中纠正兜住，
    // 见 cpu_watch；不取 p90，免得一次全量就让之后的增量编译都去排重任务名额（CASE-EST-10、CASE-EST-11）。
    let heavy = is_heavy(newest_first);
    let est_s = duration(newest_first).unwrap_or(lim.timeout_max_s / 3);
    Some(Estimate {
        res: Resources {
            heavy,
            mem_mb: upper(newest_first, |s| s.peak_mem_mb as f64).max(1),
            disk_mb: upper(newest_first, |s| s.disk_delta_mb as f64).max(0),
            uses_net: ai_uses_net.unwrap_or(false),
        },
        est_s,
        timeout_s: timeout_for(est_s, lim),
        source: Source::Measured,
    })
}

/// 本机所有命令的实测（首次见到的命令和 AI 评估的上限用）。
#[derive(Debug, Clone, Default)]
pub struct MachineHistory {
    pub peaks_mb: Vec<i64>,
    pub disk_deltas_mb: Vec<i64>,
    pub durations_s: Vec<i64>,
}

fn quantile(v: &[i64], q: f64) -> Option<i64> {
    let pairs: Vec<(f64, f64)> = v.iter().map(|x| (*x as f64, 1.0)).collect();
    weighted_quantile(&pairs, q).map(|x| x as i64)
}

/// 本机还没有任何实测时，首次见到的命令按这么多内存记账。
pub const FIRST_SEEN_MEM_MB: i64 = 1024;
/// 内存估计的下限（与 AI 输出的下限一致）。
const MIN_MEM_MB: i64 = 256;

/// 首次见到的命令：不等 AI，立即按轻任务放行（CASE-EST-13）。内存、磁盘、时长取全机实测中位数，
/// 没有历史时内存 1G；超时取上限。判轻实重由运行中纠正兜住（CASE-EST-10），跑完一次之后按实测（CASE-EST-5）。
/// 之前是「AI 同步评估，失败退回保守默认（重任务、容量 1/4）」：实测首次多等 7–74 秒，AI 常把百 MB 级的命令估成 6–12G，
/// 高负载时一条 3 秒的测试因此排队 100 秒（agent-verify 实测 2026-11-17）。
pub fn first_seen(h: &MachineHistory, lim: &Limits) -> Estimate {
    let est_s = quantile(&h.durations_s, 0.5).unwrap_or(lim.timeout_max_s / 3).max(1);
    Estimate {
        res: Resources {
            heavy: false,
            mem_mb: quantile(&h.peaks_mb, 0.5).unwrap_or(FIRST_SEEN_MEM_MB).max(MIN_MEM_MB),
            disk_mb: quantile(&h.disk_deltas_mb, 0.5).unwrap_or(0).max(0),
            uses_net: false,
        },
        est_s,
        timeout_s: lim.timeout_max_s,
        source: Source::FirstSeen,
    }
}

/// AI 评估的内存不超过全机实测峰值 p90（CASE-EST-14）：AI 只看命令和仓库规模，常把小命令估成几 G。没有历史时不限。
pub fn cap_ai(mut est: Estimate, h: &MachineHistory) -> Estimate {
    if let Some(p90) = quantile(&h.peaks_mb, 0.9) {
        est.res.mem_mb = est.res.mem_mb.min(p90.max(MIN_MEM_MB));
    }
    est
}

/// AI CLI 必须输出的格式。`reason` 只要求存在（排查时看 profiles.ai_output 原文），不参与计算。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AiOutput {
    heavy: bool,
    mem_gb: f64,
    disk_gb: f64,
    net: bool,
    est_minutes: f64,
    reason: String,
}

/// 校验 AI 输出：严格 JSON；内存不超过整机容量、耗时不超过 2 小时；reason 非空。不合格返回 None。
/// 低于下限（内存 256MB、耗时 1 秒）的抬到下限：轻命令 AI 会给出很小的值，抬高是更保守的方向。
pub fn parse_ai(text: &str, cap: &Capacity, lim: &Limits) -> Option<Estimate> {
    let o = last_json(text)?;
    if !(o.mem_gb.is_finite() && o.est_minutes.is_finite() && o.disk_gb.is_finite())
        || o.mem_gb < 0.0
        || o.est_minutes < 0.0
        || o.disk_gb < 0.0
    {
        return None;
    }
    let mem_mb = ((o.mem_gb * 1024.0).round() as i64).max(MIN_MEM_MB);
    let est_s = ((o.est_minutes * 60.0).round() as i64).max(1);
    let disk_mb = (o.disk_gb * 1024.0).round() as i64;
    if mem_mb > cap.mem_mb || est_s > lim.timeout_max_s || o.reason.trim().is_empty() {
        return None;
    }
    Some(Estimate {
        res: Resources { heavy: o.heavy, mem_mb, disk_mb, uses_net: o.net },
        est_s,
        timeout_s: timeout_for(est_s, lim),
        source: Source::Ai,
    })
}

/// 取输出末尾的 JSON 对象：前面的说明文字和 ```json 代码块开头不管，之后只允许空白和代码块结尾。
/// claude -p 常先写一段分析再给 JSON，按严格 JSON 判会白白丢掉这次评估（实测）。字段仍按 AiOutput 严格校验（CASE-EST-3）。
fn last_json(text: &str) -> Option<AiOutput> {
    let t = text.trim_end();
    let t = t.strip_suffix("```").unwrap_or(t).trim_end();
    if !t.ends_with('}') {
        return None;
    }
    t.match_indices('{').rev().find_map(|(i, _)| serde_json::from_str(&t[i..]).ok())
}

#[cfg(test)]
mod tests;
