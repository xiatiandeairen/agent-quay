//! 命令行定义。5 个子命令 + 内部用的隐藏子命令。

use clap::{Parser, Subcommand};

/// `aq hold --help` 末尾的例子：agent 多半只读 help，不读 skill（CASE-HOLD-18）。
const HOLD_EXAMPLES: &str = "Examples:
  aq hold ios-sim                          newest iOS, model from config.toml
  aq hold ios-sim@26.2 ios-sim@26.2        two iOS 26.2 simulators
  aq hold \"ios-sim@26.2:iPhone 16 Pro\" android-emu@35:pixel_7 --account zhang0003";

#[derive(Parser, Debug)]
#[command(
    name = "aq",
    version,
    about = "Resource queue for many agents on one Mac: run heavy commands in turn, borrow and return devices and accounts"
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Submit a command: aq run [--fresh] [--wait <secs>] [--human] -- <command> [args...]
    Run {
        /// Do not join an identical execution that is already queued or running
        #[arg(long)]
        fresh: bool,
        /// Wait at most this many seconds (default 100; 0 submits without waiting)
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
        /// The command after `--` runs as is, without a shell
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Show the machine or the given ids; --wait waits until all finish or time runs out; --top shows a live dashboard
    Status {
        ids: Vec<String>,
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
        /// Dashboard redrawn every 0.5s in a terminal, Ctrl-C to quit; prints one frame when stdout is not a terminal
        #[arg(long, conflicts_with_all = ["ids", "wait", "human"])]
        top: bool,
    },
    /// Cancel a queued or running execution; for a running one, waits until it has stopped
    Cancel {
        id: String,
        #[arg(long)]
        human: bool,
    },
    /// Borrow a set of devices and accounts: aq hold <device>[@<version>][:<model>]... [--account <name>]...
    #[command(after_help = HOLD_EXAMPLES)]
    Hold {
        /// One per device: ios-sim, android-emu, ios-device, android-device, browser, desktop.
        /// Simulators take [@<version>][:<model>]: ios-sim@26.2, "ios-sim@26.2:iPhone 16 Pro", android-emu@35:pixel_7.
        /// Without them: the newest iOS / the [android-emu] avd, and the model from config.toml
        devices: Vec<String>,
        /// Test account to borrow with the devices; repeat for more
        #[arg(long = "account")]
        accounts: Vec<String>,
        /// Wait at most this many seconds (default 100; 0 submits without waiting)
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
    },
    /// Return a whole hold
    Release {
        id: String,
        #[arg(long)]
        human: bool,
    },
    #[command(name = "__runner", hide = true)]
    Runner { id: String },
    #[command(name = "__detach", hide = true)]
    Detach {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(name = "__exec", hide = true)]
    Exec {
        fd: i32,
        #[arg(last = true)]
        argv: Vec<String>,
    },
    #[command(name = "__simgc", hide = true)]
    Simgc,
}
