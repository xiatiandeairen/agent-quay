//! Android 模拟器（emulator + adb）。借时按配置的 AVD 用 `-read-only` 新起一个实例，还时 `adb emu kill`。
//! aq 只用 5600–5680 的端口，用户自己开的模拟器（默认 5554 起）不受影响；设备 id 为 `emulator-<端口>`。
//! 名字（aq-<账本标识>-<借用id>-<序号>）记在 STATE/emu/<端口>.name，用来按名字对账；别的账本起的实例在本账本没有名字记录，列成 aq-unknown-0，对账不动；端口用 STATE/emu/<端口>.lock 防并发撞车。
//! 验收测试用 AQ_EMULATOR、AQ_ADB 指向假脚本。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aq_core::ports::Simulator;

use crate::cmd::{run, Call};

pub const PORT_FIRST: u16 = 5600;
pub const PORT_LAST: u16 = 5680;

pub struct Emulator {
    pub emulator: Vec<String>,
    pub adb: Vec<String>,
    pub avd: String,
    pub dir: PathBuf,
    pub boot_timeout: Duration,
}

pub(crate) fn sdk_tool(rel: &str) -> String {
    let sdk = std::env::var("ANDROID_HOME")
        .or_else(|_| std::env::var("ANDROID_SDK_ROOT"))
        .unwrap_or_else(|_| format!("{}/Library/Android/sdk", std::env::var("HOME").unwrap_or_default()));
    format!("{sdk}/{rel}")
}

impl Emulator {
    pub fn from_env(avd: &str, dir: &Path) -> Emulator {
        let tool = |var: &str, rel: &str| std::env::var(var).map(|s| vec![s]).unwrap_or_else(|_| vec![sdk_tool(rel)]);
        let boot = std::env::var("AQ_T_EMU_BOOT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        Emulator {
            emulator: tool("AQ_EMULATOR", "emulator/emulator"),
            adb: tool("AQ_ADB", "platform-tools/adb"),
            avd: avd.to_string(),
            dir: dir.to_path_buf(),
            boot_timeout: Duration::from_secs(boot),
        }
    }

    fn adb(&self, args: &[&str], secs: u64) -> Result<String, String> {
        let mut argv = self.adb.clone();
        argv.extend(args.iter().map(|s| s.to_string()));
        let o = run(&Call { argv: &argv, cwd: None, env: &[], stdin: None, timeout: Duration::from_secs(secs) })?;
        if o.status != 0 {
            return Err(format!("adb {} 退出码 {}：{}", args.join(" "), o.status, o.stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }

    /// adb 看得到的、在 aq 端口范围里的模拟器端口。
    fn running_ports(&self) -> Result<Vec<u16>, String> {
        let out = self.adb(&["devices"], 30)?;
        Ok(out
            .lines()
            .filter_map(|l| l.split_whitespace().next()?.strip_prefix("emulator-")?.parse::<u16>().ok())
            .filter(|p| (PORT_FIRST..=PORT_LAST).contains(p))
            .collect())
    }

    fn name_file(&self, port: u16) -> PathBuf {
        self.dir.join(format!("{port}.name"))
    }

    /// 挑一个空闲端口并拿住它的锁（创建期间一直拿着，防止并发的借用挑到同一个）。
    fn reserve_port(&self) -> Result<(u16, std::fs::File), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("建目录 {} 失败：{e}", self.dir.display()))?;
        let busy = self.running_ports()?;
        for p in (PORT_FIRST..=PORT_LAST).step_by(2) {
            if busy.contains(&p) {
                continue;
            }
            if let Some(lock) = crate::lock::try_lock_path(&self.dir.join(format!("{p}.lock")))? {
                return Ok((p, lock));
            }
        }
        Err("aq 的模拟器端口 5600–5680 已用完".into())
    }

    fn boot_done(&self, serial: &str) -> bool {
        self.adb(&["-s", serial, "shell", "getprop", "sys.boot_completed"], 10).is_ok_and(|s| s.trim() == "1")
    }

    fn wait_boot(&self, serial: &str, child: &mut std::process::Child) -> Result<(), String> {
        let deadline = Instant::now() + self.boot_timeout;
        while Instant::now() < deadline {
            if let Ok(Some(st)) = child.try_wait() {
                return Err(format!("emulator 启动后退出（{st}）：{}", self.last_error(serial)));
            }
            if self.boot_done(serial) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!("模拟器 {serial} 超过 {} 秒未启动完成", self.boot_timeout.as_secs()))
    }

    /// emulator 日志里最后一行 ERROR。常见：该 AVD 正被不带 -read-only 的模拟器使用（要给 aq 配专用 AVD）、AVD 不存在。
    fn last_error(&self, serial: &str) -> String {
        let port = serial.trim_start_matches("emulator-");
        let log = self.dir.join(format!("{port}.log"));
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let line = text.lines().rev().find(|l| l.contains("ERROR") || l.contains("PANIC")).unwrap_or("没有错误行").trim();
        let hint = if line.contains("Another emulator instance is running") {
            "；这个 AVD 正被不带 -read-only 的模拟器使用，请在 config.toml 的 [android-emu] 配一个 aq 专用的 AVD"
        } else {
            ""
        };
        format!("{line}{hint}（日志 {}）", log.display())
    }

    fn launch(&self, port: u16) -> Result<std::process::Child, String> {
        let log = std::fs::File::create(self.dir.join(format!("{port}.log"))).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let (prog, pre) = self.emulator.split_first().ok_or("没有配置 emulator")?;
        let mut cmd = Command::new(prog);
        cmd.args(pre).args(["-avd", &self.avd, "-read-only", "-no-window", "-no-audio", "-no-boot-anim", "-no-snapshot"]);
        cmd.args(["-port", &port.to_string()]).stdin(Stdio::null()).stdout(log).stderr(log2);
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        cmd.spawn().map_err(|e| format!("无法启动 emulator：{e}"))
    }
}

impl Simulator for Emulator {
    fn create(&self, name: &str) -> Result<String, String> {
        let (port, _lock) = self.reserve_port()?;
        let serial = format!("emulator-{port}");
        std::fs::write(self.name_file(port), name).map_err(|e| e.to_string())?;
        let mut child = self.launch(port)?;
        if let Err(e) = self.wait_boot(&serial, &mut child) {
            let _ = self.adb(&["-s", &serial, "emu", "kill"], 30); // 可能还没连上 adb，接着直接杀进程
            let _ = child.kill();
            let _ = child.wait(); // 回收；结果不影响报错
            let _ = std::fs::remove_file(self.name_file(port)); // 没建成就没有名字可留
            return Err(e);
        }
        Ok(serial)
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        let port: u16 = id.strip_prefix("emulator-").and_then(|p| p.parse().ok()).ok_or_else(|| format!("不是模拟器 id：{id}"))?;
        if !self.running_ports()?.contains(&port) {
            let _ = std::fs::remove_file(self.name_file(port)); // 已经不在，清掉名字
            return Err(format!("no such device: {id}"));
        }
        self.adb(&["-s", id, "emu", "kill"], 30)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && self.running_ports()?.contains(&port) {
            std::thread::sleep(Duration::from_millis(300));
        }
        let _ = std::fs::remove_file(self.name_file(port)); // 删完清掉名字；文件不在无妨
        Ok(())
    }

    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        Ok(self
            .running_ports()?
            .into_iter()
            .map(|p| {
                let name =
                    std::fs::read_to_string(self.name_file(p)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "aq-unknown-0".into());
                (name, format!("emulator-{p}"))
            })
            .collect())
    }
}
