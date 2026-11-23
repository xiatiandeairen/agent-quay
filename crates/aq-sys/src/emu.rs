//! Android 模拟器（emulator + adb）。借时按写的版本、机型新建一个 AVD（名字同设备名，放在 STATE/emu/avd，见 avd.rs），
//! 用 `-read-only` 无窗口启动，还时 `adb emu kill` 并删掉这个 AVD。配置里的 AVD 只提供默认的镜像、机型、内存，aq 不启动它（CASE-HOLD-16）。
//! aq 只用 5600–5680 的端口，用户自己开的模拟器（默认 5554 起）不受影响；设备 id 为 `emulator-<端口>`。
//! 名字（aq-<账本标识>-<借用id>-<序号>）记在 STATE/emu/<端口>.name，用来按名字对账；别的账本起的实例在本账本没有名字记录，列成 aq-unknown-0，对账不动；端口用 STATE/emu/<端口>.lock 防并发撞车。
//! 建好 AVD、还没启动就被杀时，AVD 目录没有实例对应，list_aq 列成 `avd:<名字>`，由对账删掉（CASE-RECLAIM-13）。
//! 验收测试用 AQ_EMULATOR、AQ_ADB、AQ_AVDMANAGER 指向假脚本。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use aq_core::devspec::DeviceSpec;
use aq_core::ports::{Simulator, SpecError};

use crate::avd::{self, Plan};
use crate::cmd::{ok_text, run, Call};

pub const PORT_FIRST: u16 = 5600;
pub const PORT_LAST: u16 = 5680;

pub struct Emulator {
    pub emulator: Vec<String>,
    pub adb: Vec<String>,
    pub avdmanager: Vec<String>,
    /// 配置里的 AVD：新建时默认的镜像、机型、内存从它读。
    pub avd: String,
    /// 配置 AVD 所在的目录（用户的 AVD 目录）。
    pub user_avd_home: PathBuf,
    pub sdk: PathBuf,
    /// 状态目录 STATE/emu：端口锁、名字、日志；aq 建的 AVD 在它下面的 avd/。
    pub dir: PathBuf,
    pub boot_timeout: Duration,
    /// 模拟器进程的 nice 值（0 不降，CASE-PROT-3）。
    pub nice: i32,
}

fn sdk_root() -> String {
    std::env::var("ANDROID_HOME")
        .or_else(|_| std::env::var("ANDROID_SDK_ROOT"))
        .unwrap_or_else(|_| format!("{}/Library/Android/sdk", std::env::var("HOME").unwrap_or_default()))
}

pub(crate) fn sdk_tool(rel: &str) -> String {
    format!("{}/{rel}", sdk_root())
}

impl Emulator {
    pub fn from_env(avd: &str, dir: &Path) -> Emulator {
        let tool = |var: &str, rel: &str| std::env::var(var).map(|s| vec![s]).unwrap_or_else(|_| vec![sdk_tool(rel)]);
        let boot = std::env::var("AQ_T_EMU_BOOT_S").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        Emulator {
            emulator: tool("AQ_EMULATOR", "emulator/emulator"),
            adb: tool("AQ_ADB", "platform-tools/adb"),
            avdmanager: tool("AQ_AVDMANAGER", "cmdline-tools/latest/bin/avdmanager"),
            avd: avd.to_string(),
            user_avd_home: avd::user_avd_home(),
            sdk: PathBuf::from(sdk_root()),
            dir: dir.to_path_buf(),
            boot_timeout: Duration::from_secs(boot),
            nice: 0,
        }
    }

    fn adb(&self, args: &[&str], secs: u64) -> Result<String, String> {
        let mut argv = self.adb.clone();
        argv.extend(args.iter().map(|s| s.to_string()));
        let o = run(&Call { argv: &argv, cwd: None, env: &[], stdin: None, timeout: Duration::from_secs(secs) })?;
        if o.status != 0 {
            return Err(format!("adb {} exited with {}: {}", args.join(" "), o.status, o.stderr.trim()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }

    fn avd_home(&self) -> PathBuf {
        self.dir.join("avd")
    }

    /// 启动 emulator、avdmanager 时让它们用 aq 自己的 AVD 目录和同一个 SDK。
    fn tool_env(&self) -> [(&'static str, String); 3] {
        let sdk = self.sdk.to_string_lossy().into_owned();
        [("ANDROID_AVD_HOME", self.avd_home().to_string_lossy().into_owned()), ("ANDROID_HOME", sdk.clone()), ("ANDROID_SDK_ROOT", sdk)]
    }

    /// 按写的版本、机型和配置 AVD 定下新 AVD 的镜像、机型、内存；写了机型才问 avdmanager 有哪些机型配置。
    fn plan(&self, spec: &DeviceSpec) -> Result<Plan, SpecError> {
        let def = avd::read_conf(&self.user_avd_home, &self.avd)
            .map_err(|e| SpecError::Unfit(format!("{e}; set an existing AVD under [android-emu] avd in config.toml")))?;
        let profiles = match spec.model {
            Some(_) => Some(avd::parse_profiles(&self.avdmanager_call(&["list", "device"], None).map_err(SpecError::Tool)?)),
            None => None,
        };
        avd::plan(spec, &def, &avd::installed(&self.sdk), avd::host_abi(), profiles.as_deref()).map_err(SpecError::Unfit)
    }

    fn avdmanager_call(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let mut argv = self.avdmanager.clone();
        argv.extend(args.iter().map(|s| s.to_string()));
        let env = self.tool_env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        ok_text(&Call { argv: &argv, cwd: None, env: &env, stdin, timeout: Duration::from_secs(120) })
    }

    /// 新建 AVD 并把内存改成配置 AVD 的。avdmanager 会问要不要自定义硬件配置，答 no。
    fn make_avd(&self, name: &str, p: &Plan) -> Result<(), String> {
        std::fs::create_dir_all(self.avd_home()).map_err(|e| format!("cannot create {}: {e}", self.avd_home().display()))?;
        let pkg = p.image.package();
        self.avdmanager_call(&["create", "avd", "-n", name, "-k", &pkg, "-d", &p.device], Some(b"no\n"))
            .map_err(|e| format!("avdmanager could not create AVD {name}: {e}"))?;
        avd::set_ram(&self.avd_home().join(format!("{name}.avd")), &p.ram)
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
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let busy = self.running_ports()?;
        for p in (PORT_FIRST..=PORT_LAST).step_by(2) {
            if busy.contains(&p) {
                continue;
            }
            if let Some(lock) = crate::lock::try_lock_path(&self.dir.join(format!("{p}.lock")))? {
                return Ok((p, lock));
            }
        }
        Err("aq emulator ports 5600-5680 are all in use".into())
    }

    fn boot_done(&self, serial: &str) -> bool {
        self.adb(&["-s", serial, "shell", "getprop", "sys.boot_completed"], 10).is_ok_and(|s| s.trim() == "1")
    }

    fn wait_boot(&self, serial: &str, child: &mut std::process::Child) -> Result<(), String> {
        let deadline = Instant::now() + self.boot_timeout;
        while Instant::now() < deadline {
            if let Ok(Some(st)) = child.try_wait() {
                return Err(format!("emulator exited after start ({st}): {}", self.last_error(serial)));
            }
            if self.boot_done(serial) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!("emulator {serial} did not finish booting within {}s", self.boot_timeout.as_secs()))
    }

    /// emulator 日志里最后一行 ERROR。
    fn last_error(&self, serial: &str) -> String {
        let port = serial.trim_start_matches("emulator-");
        let log = self.dir.join(format!("{port}.log"));
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let line = text.lines().rev().find(|l| l.contains("ERROR") || l.contains("PANIC")).unwrap_or("no error line").trim();
        format!("{line} (log {})", log.display())
    }

    fn launch(&self, port: u16, name: &str) -> Result<std::process::Child, String> {
        let log = std::fs::File::create(self.dir.join(format!("{port}.log"))).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let (prog, pre) = self.emulator.split_first().ok_or("no emulator configured")?;
        let mut cmd = Command::new(prog);
        cmd.args(pre).args(["-avd", name, "-read-only", "-no-window", "-no-audio", "-no-boot-anim", "-no-snapshot"]);
        cmd.args(["-port", &port.to_string()]).envs(self.tool_env()).stdin(Stdio::null()).stdout(log).stderr(log2);
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        if self.nice > 0 {
            let nice = self.nice;
            // SAFETY: pre_exec 里只调用 async-signal-safe 的 setpriority。
            unsafe {
                std::os::unix::process::CommandExt::pre_exec(&mut cmd, move || {
                    libc::setpriority(libc::PRIO_PROCESS, 0, nice);
                    Ok(())
                });
            }
        }
        cmd.spawn().map_err(|e| format!("cannot start emulator: {e}"))
    }
}

impl Simulator for Emulator {
    fn check(&self, spec: &DeviceSpec) -> Result<(), SpecError> {
        self.plan(spec).map(|_| ())
    }

    fn create(&self, name: &str, spec: &DeviceSpec) -> Result<String, String> {
        let plan = self.plan(spec).map_err(|e| match e {
            SpecError::Unfit(m) | SpecError::Tool(m) => m,
        })?;
        let (port, _lock) = self.reserve_port()?;
        let serial = format!("emulator-{port}");
        std::fs::write(self.name_file(port), name).map_err(|e| e.to_string())?;
        let started = self.make_avd(name, &plan).and_then(|_| self.launch(port, name));
        let booted = started.and_then(|mut child| match self.wait_boot(&serial, &mut child) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = self.adb(&["-s", &serial, "emu", "kill"], 30); // 可能还没连上 adb，接着直接杀进程
                let _ = child.kill();
                let _ = child.wait(); // 回收；结果不影响报错
                Err(e)
            }
        });
        if let Err(e) = booted {
            let _ = std::fs::remove_file(self.name_file(port)); // 没建成就没有名字可留
            let _ = avd::remove(&self.avd_home(), name); // 删不掉由对账兜底
            return Err(e);
        }
        Ok(serial)
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        if let Some(name) = id.strip_prefix("avd:") {
            return avd::remove(&self.avd_home(), name); // 没启动起来的半成品 AVD
        }
        let port: u16 = id.strip_prefix("emulator-").and_then(|p| p.parse().ok()).ok_or_else(|| format!("not an emulator id: {id}"))?;
        let name = std::fs::read_to_string(self.name_file(port)).map(|s| s.trim().to_string()).ok();
        let forget = || {
            let _ = std::fs::remove_file(self.name_file(port)); // 文件不在无妨
            if let Some(n) = &name {
                let _ = avd::remove(&self.avd_home(), n); // 删不掉的由对账按 avd: 再删
            }
        };
        if !self.running_ports()?.contains(&port) {
            forget(); // 已经不在，清掉名字和 AVD
            return Err(format!("no such device: {id}"));
        }
        self.adb(&["-s", id, "emu", "kill"], 30)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && self.running_ports()?.contains(&port) {
            std::thread::sleep(Duration::from_millis(300));
        }
        forget();
        Ok(())
    }

    /// 在跑的实例，加上没有实例对应的 aq AVD（`avd:<名字>`）。
    fn list_aq(&self) -> Result<Vec<(String, String)>, String> {
        let running: Vec<(String, String)> = self
            .running_ports()?
            .into_iter()
            .map(|p| {
                let name =
                    std::fs::read_to_string(self.name_file(p)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "aq-unknown-0".into());
                (name, format!("emulator-{p}"))
            })
            .collect();
        let orphans: Vec<(String, String)> = avd::names(&self.avd_home())
            .into_iter()
            .filter(|n| !running.iter().any(|(r, _)| r == n))
            .map(|n| (n.clone(), format!("avd:{n}")))
            .collect();
        Ok(running.into_iter().chain(orphans).collect())
    }
}
