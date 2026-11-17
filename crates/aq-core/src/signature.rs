//! 命令签名与 hash。

/// 去掉每次都会变的部分（绝对路径、临时目录、设备 id、长数字），让本质相同的命令共享一份评估。
/// 被执行的程序或脚本是绝对路径时保留文件名（`<路径>/build.sh`）：不同脚本不共用评估，AI 也能看出是什么（CASE-EST-7）。
pub fn signature(argv: &[String]) -> String {
    let progs = program_positions(argv);
    argv.iter()
        .enumerate()
        .map(|(i, a)| match a.rsplit('/').next() {
            Some(name) if progs.contains(&i) && is_path(a) && !name.is_empty() => format!("<路径>/{name}"),
            _ => normalize_token(a),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const INTERPRETERS: [&str; 7] = ["sh", "bash", "zsh", "python", "python3", "ruby", "node"];

/// 被执行的程序所在位置：跳过开头的 env 和 KEY=VALUE；解释器后面第一个不以 - 开头的参数是脚本，也算。
fn program_positions(argv: &[String]) -> Vec<usize> {
    let Some(p) = argv.iter().position(|a| a != "env" && !a.contains('=')) else { return vec![] };
    let name = argv[p].rsplit('/').next().unwrap_or("");
    let script = INTERPRETERS.contains(&name).then(|| argv[p + 1..].iter().position(|a| !a.starts_with('-'))).flatten();
    // `sh -c '…'` 的 -c 后面是代码不是脚本
    let inline = argv[p + 1..].iter().any(|a| a == "-c");
    [Some(p), script.filter(|_| !inline).map(|s| p + 1 + s)].into_iter().flatten().collect()
}

fn normalize_token(t: &str) -> String {
    if is_path(t) {
        return "<路径>".into();
    }
    // key=value 形式（如 -destination 'platform=iOS Simulator,id=ABCD-…'）逐段处理
    if t.contains('=') || t.contains(',') {
        return t
            .split(',')
            .map(|part| match part.split_once('=') {
                Some((k, v)) => format!("{k}={}", normalize_value(v)),
                None => normalize_value(part),
            })
            .collect::<Vec<_>>()
            .join(",");
    }
    normalize_value(t)
}

fn normalize_value(v: &str) -> String {
    if is_path(v) {
        "<路径>".into()
    } else if is_device_id(v) {
        "<id>".into()
    } else if v.len() >= 5 && v.chars().all(|c| c.is_ascii_digit()) {
        "<n>".into()
    } else {
        v.to_string()
    }
}

fn is_path(t: &str) -> bool {
    t.starts_with('/') || t.starts_with("~/")
}

/// UUID（模拟器）或 00008150-… 形式的真机 UDID。
fn is_device_id(v: &str) -> bool {
    let hexish = v.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    hexish && v.contains('-') && v.len() >= 20
}

fn hex(h: blake3::Hash, n: usize) -> String {
    h.to_hex().as_str()[..n].to_string()
}

/// 资源评估按「仓库 + 命令签名」缓存；同一仓库的所有 worktree 共用（repo_key 是 git common dir）。
pub fn profile_key(repo_key: &str, signature: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(repo_key.as_bytes());
    h.update(b"\0");
    h.update(signature.as_bytes());
    hex(h.finalize(), 32)
}

/// hash = H(内容指纹, 执行目录在仓库内的相对路径, 命令原文)。环境变量不进 hash（CASE-REUSE-3）。
pub fn exec_hash(fingerprint: &str, rel_cwd: &str, argv: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    for part in [fingerprint, rel_cwd] {
        h.update(&(part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    for a in argv {
        h.update(&(a.len() as u64).to_le_bytes());
        h.update(a.as_bytes());
    }
    hex(h.finalize(), 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    // CASE-EST-7
    #[test]
    fn signature_strips_volatile_parts() {
        let a = v(&["xcodebuild", "-scheme", "App", "-derivedDataPath", "/private/tmp/s1/dd", "build"]);
        let b = v(&["xcodebuild", "-scheme", "App", "-derivedDataPath", "/private/tmp/s2/dd", "build"]);
        assert_eq!(signature(&a), signature(&b));
        let d1 = v(&["-destination", "platform=iOS Simulator,id=ABADBEDA-1111-2222-3333-444455556666"]);
        let d2 = v(&["-destination", "platform=iOS Simulator,id=00008150-001A2B3C4D5E"]);
        assert_eq!(signature(&d1), signature(&d2));
        assert_ne!(signature(&v(&["-scheme", "App"])), signature(&v(&["-scheme", "Demo"])));
    }

    // CASE-EST-7
    #[test]
    fn script_name_kept_in_signature() {
        let run = |s: &str| v(&[&format!("/private/tmp/s1/scratchpad/{s}"), "Debug", "/private/tmp/s1/dd"]);
        assert_eq!(signature(&run("build137.sh")), "<路径>/build137.sh Debug <路径>");
        assert_ne!(signature(&run("build137.sh")), signature(&run("verify.sh")), "不同脚本不共用评估");
        let other_session = v(&["/private/tmp/s2/scratchpad/build137.sh", "Debug", "/private/tmp/s2/dd"]);
        assert_eq!(signature(&run("build137.sh")), signature(&other_session), "同一脚本换个会话目录仍共用");
        assert_eq!(signature(&v(&["env", "A=/x/y", "/opt/t/run.sh"])), "env A=<路径> <路径>/run.sh");
        assert_eq!(signature(&v(&["bash", "-x", "/opt/t/run.sh", "/opt/out"])), "bash -x <路径>/run.sh <路径>");
        assert_eq!(signature(&v(&["sh", "-c", "/opt/t/run.sh"])), "sh -c <路径>", "-c 后是代码");
        assert_eq!(signature(&v(&["xcodebuild", "-derivedDataPath", "/tmp/dd"])), "xcodebuild -derivedDataPath <路径>");
    }

    // CASE-REUSE-3
    #[test]
    fn argv_text_matters() {
        let base = exec_hash("sha", "", &v(&["a", "b"]));
        assert_ne!(base, exec_hash("sha", "", &v(&["b", "a"])));
        assert_ne!(base, exec_hash("sha", "", &v(&["a", "c"])));
        assert_eq!(base, exec_hash("sha", "", &v(&["a", "b"])));
    }

    // CASE-REUSE-3
    #[test]
    fn rel_cwd_matters_and_no_concat_ambiguity() {
        assert_ne!(exec_hash("sha", "", &v(&["x"])), exec_hash("sha", "Modules", &v(&["x"])));
        assert_ne!(exec_hash("ab", "c", &v(&[])), exec_hash("a", "bc", &v(&[])));
        assert_ne!(exec_hash("s", "", &v(&["a b"])), exec_hash("s", "", &v(&["a", "b"])));
    }
}
