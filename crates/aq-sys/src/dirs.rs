//! 存储目录（XDG）。
//! 锁文件和数据库放在一起，不放 XDG_RUNTIME_DIR（macOS 默认不设，环境不一致时账本会分家）。

use std::fs;
use std::path::{Path, PathBuf};

use etcetera::base_strategy::{BaseStrategy, Xdg};

#[derive(Debug, Clone)]
pub struct Dirs {
    pub config_file: PathBuf,
    pub state: PathBuf,
    pub db: PathBuf,
    pub locks: PathBuf,
    pub holds: PathBuf,
    pub logs: PathBuf,
    pub aq_log: PathBuf,
}

impl Dirs {
    pub fn resolve() -> Result<Dirs, String> {
        let x = Xdg::new().map_err(|e| format!("cannot find the HOME directory: {e}"))?;
        let state_root = x.state_dir().unwrap_or_else(|| x.home_dir().join(".local/state"));
        Ok(Self::at(&x.config_dir().join("aq/config.toml"), &state_root.join("aq")))
    }

    /// 是不是默认账本（`~/.local/state/aq`，不受 XDG_STATE_HOME 影响）。旧格式的设备名只由默认账本认领（CASE-HOLD-10）。
    pub fn is_default_state(&self) -> bool {
        let Ok(x) = Xdg::new() else { return false };
        let default = x.home_dir().join(".local/state/aq");
        let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(&self.state) == canon(&default)
    }

    pub fn at(config_file: &Path, state: &Path) -> Dirs {
        Dirs {
            config_file: config_file.to_path_buf(),
            state: state.to_path_buf(),
            db: state.join("aq.db"),
            locks: state.join("locks"),
            holds: state.join("holds"),
            logs: state.join("logs"),
            aq_log: state.join("aq.log"),
        }
    }

    /// 建目录并确认可写；写不进去时报错并给出具体路径（沙箱需放行这个目录）。
    pub fn ensure(&self) -> Result<(), String> {
        for d in [&self.state, &self.locks, &self.holds, &self.logs] {
            fs::create_dir_all(d).map_err(|e| unwritable(d, &e))?;
        }
        let probe = self.state.join(".write-test");
        fs::write(&probe, b"").map_err(|e| unwritable(&self.state, &e))?;
        let _ = fs::remove_file(&probe); // 探测文件删不掉不影响后续，只是多一个空文件
        Ok(())
    }

    pub fn lock_file(&self, name: &str) -> PathBuf {
        self.locks.join(format!("{name}.lock"))
    }
    pub fn log_file(&self, exec_id: &str) -> PathBuf {
        self.logs.join(format!("{exec_id}.log"))
    }
    pub fn used_file(&self, hold_id: &str) -> PathBuf {
        self.holds.join(format!("{hold_id}.used"))
    }
}

fn unwritable(p: &Path, e: &std::io::Error) -> String {
    format!("state directory is not writable: {} ({e}). In a sandbox, add this directory to the writable paths", p.display())
}
