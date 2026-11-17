//! 命令行定义。5 个子命令 + 内部用的隐藏子命令。

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "aq", version, about = "单机多 agent 资源调度：排队执行重任务、借还设备和账号")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// 提交一条命令：aq run [--fresh] [--wait <秒>] [--human] -- <命令> [参数...]
    Run {
        /// 不复用正在跑的同一次执行
        #[arg(long)]
        fresh: bool,
        /// 最多等多少秒（默认 100；0 只提交不等）
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
        /// `--` 之后的命令原样执行，不经过 shell
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// 查看全局或指定对象；--wait 等到全部结束或超时
    Status {
        ids: Vec<String>,
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
    },
    /// 撤销排队中或运行中的执行
    Cancel {
        id: String,
        #[arg(long)]
        human: bool,
    },
    /// 借一组设备和账号：aq hold <设备类型>... [--account <账号名>]...
    Hold {
        devices: Vec<String>,
        #[arg(long = "account")]
        accounts: Vec<String>,
        #[arg(long)]
        wait: Option<u64>,
        #[arg(long)]
        human: bool,
    },
    /// 整组归还
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
