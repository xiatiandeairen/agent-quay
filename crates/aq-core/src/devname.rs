//! 虚拟设备的命名与归属：`aq-<账本标识>-<借用id>-<序号>`。
//! 同一台机器上可能有多个账本（不同 XDG_STATE_HOME：开发、演示、测试的 aq）。设备名带上账本标识，
//! 对账和半成品清理只认本账本的设备，互不删除（CASE-HOLD-9）。旧格式 `aq-<借用id>-<序号>` 只由默认账本认领（CASE-HOLD-10）。

/// 账本标识：状态目录路径的 blake3 前 8 位十六进制。
pub fn ledger_tag(state_dir: &str) -> String {
    blake3::hash(state_dir.as_bytes()).to_hex().as_str()[..8].to_string()
}

pub fn name(ledger: &str, hold_id: &str, seq: i64) -> String {
    format!("aq-{ledger}-{hold_id}-{seq}")
}

/// 设备名属于哪个账本的哪组借用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// 本账本的设备，附借用 id。
    Mine(String),
    /// 别的账本、认不出的名字（含 Android 实例没有名字记录时的 `aq-unknown-0`）：不归本账本管，一律不动。
    Other,
}

/// `legacy` 为真（本账本是默认账本）时，旧格式名字也算本账本的。
pub fn owner(name: &str, ledger: &str, legacy: bool) -> Owner {
    let Some(rest) = name.strip_prefix("aq-") else { return Owner::Other };
    let hold = |s: &str| s.rsplit_once('-').map(|(h, _)| h.to_string()).filter(|h| h.starts_with("hd_"));
    if rest.starts_with("hd_") {
        return match hold(rest) {
            Some(h) if legacy => Owner::Mine(h),
            _ => Owner::Other,
        };
    }
    match rest.split_once('-') {
        Some((tag, tail)) if tag == ledger => hold(tail).map_or(Owner::Other, Owner::Mine),
        _ => Owner::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CASE-HOLD-9
    #[test]
    fn tagged_names_belong_only_to_their_ledger() {
        let a = ledger_tag("/x/a/aq");
        let b = ledger_tag("/x/b/aq");
        assert_ne!(a, b);
        let n = name(&a, "hd_01abc", 1);
        assert_eq!(owner(&n, &a, false), Owner::Mine("hd_01abc".into()));
        assert_eq!(owner(&n, &b, false), Owner::Other);
        assert_eq!(owner(&n, &b, true), Owner::Other, "默认账本也不认别的账本的新格式名字");
    }

    // CASE-HOLD-10
    #[test]
    fn legacy_names_only_for_default_ledger() {
        let t = ledger_tag("/x/aq");
        assert_eq!(owner("aq-hd_x-1", &t, true), Owner::Mine("hd_x".into()));
        assert_eq!(owner("aq-hd_x-1", &t, false), Owner::Other);
    }

    // CASE-HOLD-9
    #[test]
    fn unknown_names_are_never_mine() {
        let t = ledger_tag("/x/aq");
        for n in ["aq-unknown-0", "aq-demo", "my-sim", "aq-", &format!("aq-{t}-nohold-1")] {
            assert_eq!(owner(n, &t, true), Owner::Other, "{n}");
        }
    }
}
