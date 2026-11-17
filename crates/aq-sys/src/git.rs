//! 内容指纹。
//! 所有 git 调用带 GIT_OPTIONAL_LOCKS=0：只读、不拿 .git/index.lock。
//! 干净时 = HEAD SHA；有改动时 = SHA+dirty.<摘要>，摘要覆盖未提交改动、未被忽略的新文件、子模块（递归）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aq_core::ports::{Git, RepoInfo};

use crate::cmd::{ok_text, Call};

pub struct RealGit {
    /// 整个指纹计算的时限（文档：30 秒）。
    pub budget: Duration,
}

struct Ctx {
    deadline: Instant,
}

impl Ctx {
    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, String> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("计算指纹超时".into());
        }
        let mut argv = vec!["git".to_string(), "-C".into(), dir.to_string_lossy().into_owned()];
        argv.extend(args.iter().map(|s| s.to_string()));
        let env = [("GIT_OPTIONAL_LOCKS", "0"), ("GIT_TERMINAL_PROMPT", "0"), ("LC_ALL", "C")];
        ok_text(&Call { argv: &argv, cwd: None, env: &env, stdin: None, timeout: left })
    }
}

/// 一个工作区（或子模块）的改动摘要；None 表示干净。
fn dirty_digest(cx: &Ctx, dir: &Path, h: &mut blake3::Hasher) -> Result<bool, String> {
    let diff = cx.git(dir, &["diff", "HEAD", "--binary", "--no-color", "--no-ext-diff", "--ignore-submodules=all"])?;
    let untracked = cx.git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut dirty = !diff.is_empty();
    h.update(&(diff.len() as u64).to_le_bytes());
    h.update(diff.as_bytes());
    for rel in untracked.split('\0').filter(|s| !s.is_empty()) {
        dirty = true;
        h.update(rel.as_bytes());
        h.update(b"\0");
        let p = dir.join(rel);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                h.update(std::fs::read_link(&p).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default().as_bytes());
            }
            Ok(m) if m.is_file() => {
                let bytes = std::fs::read(&p).map_err(|e| format!("读取 {} 失败：{e}", p.display()))?;
                h.update(blake3::hash(&bytes).as_bytes());
            }
            _ => {}
        }
    }
    Ok(dirty)
}

/// 子模块（递归列出）：每个已初始化的子模块算它自己的 HEAD 和改动。
fn submodules(cx: &Ctx, top: &Path, h: &mut blake3::Hasher) -> Result<bool, String> {
    if !top.join(".gitmodules").exists() {
        return Ok(false);
    }
    let status = cx.git(top, &["submodule", "status", "--recursive"])?;
    let mut dirty = false;
    for line in status.lines() {
        let Some(flag) = line.chars().next() else { continue };
        let mut parts = line[1..].split_whitespace();
        let (Some(_sha), Some(path)) = (parts.next(), parts.next()) else { continue };
        if flag == '-' {
            continue; // 未初始化（CASE-REUSE-6）：没有工作区内容可算
        }
        let dir = top.join(path);
        let head = cx.git(&dir, &["rev-parse", "HEAD"])?;
        h.update(path.as_bytes());
        h.update(head.trim().as_bytes());
        dirty |= flag == '+' || flag == 'U';
        dirty |= dirty_digest(cx, &dir, h)?;
    }
    Ok(dirty)
}

fn fingerprint(cx: &Ctx, top: &Path) -> Result<String, String> {
    let head = cx.git(top, &["rev-parse", "--verify", "HEAD"])?.trim().to_string();
    let mut h = blake3::Hasher::new();
    let mut dirty = dirty_digest(cx, top, &mut h)?;
    dirty |= submodules(cx, top, &mut h)?;
    if !dirty {
        return Ok(head);
    }
    Ok(format!("{head}+dirty.{}", &h.finalize().to_hex().as_str()[..12]))
}

fn abs(base: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    joined.canonicalize().unwrap_or(joined)
}

impl Git for RealGit {
    fn locate(&self, cwd: &str) -> Result<Option<RepoInfo>, String> {
        let cx = Ctx { deadline: Instant::now() + self.budget };
        let dir = Path::new(cwd);
        let Ok(out) = cx.git(dir, &["rev-parse", "--show-toplevel", "--git-common-dir", "--show-prefix"]) else {
            return Ok(None); // 不在 git 仓库里（CASE-REUSE-4）
        };
        let mut lines = out.lines();
        let (Some(top), Some(common)) = (lines.next(), lines.next()) else { return Ok(None) };
        let prefix = lines.next().unwrap_or("").trim_end_matches('/').to_string();
        let top = PathBuf::from(top);
        Ok(Some(RepoInfo {
            toplevel: top.to_string_lossy().into_owned(),
            common_dir: abs(dir, common).to_string_lossy().into_owned(),
            rel_cwd: prefix,
            fingerprint: fingerprint(&cx, &top).ok(),
        }))
    }
}
