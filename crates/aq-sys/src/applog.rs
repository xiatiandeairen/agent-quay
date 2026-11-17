//! aq.log：每次状态转移写一行 JSON（id、原状态、新状态、原因、触发命令）。超过 50MB 时滚动为 aq.log.1。
//! 每行一次 O_APPEND 写入，多进程并发也不会交错。

use std::io::Write;
use std::path::Path;

const ROTATE_BYTES: u64 = 50 * 1024 * 1024;

pub fn transition(log: &Path, now_ms: i64, id: &str, from: &str, to: &str, reason: &str, by: &str) {
    let line = serde_json::json!({"ts": now_ms, "id": id, "from": from, "to": to, "reason": reason, "by": by});
    write_line(log, &line.to_string());
}

pub fn note(log: &Path, now_ms: i64, id: &str, msg: &str, by: &str) {
    let line = serde_json::json!({"ts": now_ms, "id": id, "note": msg, "by": by});
    write_line(log, &line.to_string());
}

fn write_line(log: &Path, line: &str) {
    if std::fs::metadata(log).map(|m| m.len() > ROTATE_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(log, log.with_extension("log.1")); // 并发滚动时只有一个成功，其他进程写进新文件
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = f.write_all(format!("{line}\n").as_bytes()); // 诊断日志写失败不影响业务结果
    }
}
