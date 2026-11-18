//! aq 入口：解析命令行 → 调用例 → 输出一行一个 JSON（或 --human 文本）→ 映射退出码。

mod app;
mod cli;
mod config;

use std::fs::File;

use app::ctx::Ctx;
use app::error::{AppError, AppResult};
use clap::Parser;
use cli::{Cli, Cmd};
use serde_json::Value;

fn print_lines(vs: &[Value]) {
    for v in vs {
        println!("{v}");
    }
}

fn emit(r: AppResult<(Vec<Value>, i32)>) -> i32 {
    match r {
        Ok((vs, code)) => {
            print_lines(&vs);
            code
        }
        Err(e) => {
            println!("{}", e.to_json());
            e.exit_code()
        }
    }
}

/// --human：动作照常执行，输出换成易读文本，退出码不变。
fn humanize(ctx: &Ctx, store: &mut aq_store::Store, r: AppResult<(Vec<Value>, i32)>) -> i32 {
    match r {
        Ok((vs, code)) => {
            let ids: Vec<String> = vs.iter().filter_map(|v| v.get("id").and_then(Value::as_str).map(String::from)).collect();
            if ids.is_empty() {
                print_lines(&vs);
            } else {
                match app::human::objects(ctx, store, &ids) {
                    Ok(t) => print!("{t}"),
                    Err(e) => println!("{}", e.to_json()),
                }
            }
            code
        }
        Err(e) => emit(Err(e)),
    }
}

fn one(r: AppResult<(Value, i32)>) -> AppResult<(Vec<Value>, i32)> {
    r.map(|(v, c)| (vec![v], c))
}

fn user_cmd(cmd: Cmd) -> i32 {
    if let Cmd::Run { cmd: argv, .. } = &cmd {
        if std::env::var_os("AQ_EXECUTION_ID").is_some() && !argv.is_empty() {
            return app::run::nested(argv);
        }
    }
    let (ctx, mut store) = match Ctx::open("aq") {
        Ok(x) => x,
        Err(e) => return emit(Err(e)),
    };
    let s = &mut store;
    let (r, human) = match cmd {
        Cmd::Run { fresh, wait, human, cmd } => (one(app::run::run(&ctx, s, &app::run::RunArgs { fresh, wait_s: wait, argv: cmd })), human),
        Cmd::Status { top: true, .. } => {
            return match app::top::run(&ctx, s) {
                Ok(c) => c,
                Err(e) => emit(Err(e)),
            };
        }
        Cmd::Status { ids, wait, human, .. } if ids.is_empty() => {
            if human {
                return match app::human::overview(&ctx, s) {
                    Ok((t, c)) => {
                        print!("{t}");
                        c
                    }
                    Err(e) => emit(Err(e)),
                };
            }
            let _ = wait; // 不带 id 时 --wait 无意义，忽略
            (app::status::overview(&ctx, s), false)
        }
        Cmd::Status { ids, wait, human, .. } => {
            (app::status::require_ids(&ids).and_then(|_| app::status::by_ids(&ctx, s, &ids, wait)), human)
        }
        Cmd::Cancel { id, human } => (one(app::release::cancel(&ctx, s, &id)), human),
        Cmd::Hold { devices, accounts, wait, human } => {
            (one(app::hold::hold(&ctx, s, &app::hold::HoldArgs { devices, accounts, wait_s: wait })), human)
        }
        Cmd::Release { id, human } => (one(app::release::release(&ctx, s, &id)), human),
        _ => return 3,
    };
    if human {
        humanize(&ctx, s, r)
    } else {
        emit(r)
    }
}

/// runner 接手提交方交给它的锁；没有就自己拿，拿不到说明已有 runner 在跑。
fn runner_lock(ctx: &Ctx, id: &str) -> AppResult<Option<File>> {
    if let Some(fd) = std::env::var("AQ_LOCK_FD").ok().and_then(|v| v.parse::<i32>().ok()) {
        std::env::remove_var("AQ_LOCK_FD");
        // SAFETY: AQ_LOCK_FD 由 aq __detach 设置，指向继承来的、仍持有锁的锁文件。
        return unsafe { aq_sys::lock::adopt(fd) }.map(Some).map_err(AppError::Fatal);
    }
    aq_sys::lock::try_lock_path(&ctx.dirs.lock_file(id)).map_err(AppError::Fatal)
}

fn internal(cmd: Cmd) -> i32 {
    match cmd {
        Cmd::Detach { args } => {
            let exe = std::env::current_exe().unwrap_or_else(|_| "aq".into());
            match aq_sys::spawn::detach_child(&exe, &args) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("aq __detach: {e}");
                    1
                }
            }
        }
        Cmd::Exec { fd, argv } => aq_sys::spawn::exec_after_gate(fd, &argv),
        Cmd::Runner { id } => background("runner", |ctx, store| {
            let Some(lock) = runner_lock(ctx, &id)? else { return Ok(()) };
            if id.starts_with("hd_") {
                app::hold::runner(ctx, store, &id, lock)
            } else {
                app::runner::run(ctx, store, &id, lock)
            }
        }),
        Cmd::Simgc => background("simgc", |ctx, store| {
            let mut lock = runner_lock(ctx, "simgc")?;
            // 放锁后再看一次：清理快结束时别人发起的清理会因为拿不到锁而跳过，这里接着做
            while lock.is_some() {
                app::simgc::run(ctx, store)?;
                drop(lock.take());
                if aq_store::orphan_sim_rows(store.conn())?.is_empty() {
                    break;
                }
                lock = aq_sys::lock::try_lock_path(&ctx.dirs.lock_file("simgc")).map_err(AppError::Fatal)?;
            }
            Ok(())
        }),
        _ => 3,
    }
}

fn background(by: &'static str, f: impl FnOnce(&Ctx, &mut aq_store::Store) -> AppResult<()>) -> i32 {
    let (ctx, mut store) = match Ctx::open(by) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("aq {by}: {}", e.to_json());
            return 1;
        }
    };
    match f(&ctx, &mut store) {
        Ok(()) => 0,
        Err(e) => {
            ctx.log_note("", &format!("{by} failed: {}", e.to_json()));
            eprintln!("aq {by}: {}", e.to_json());
            1
        }
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) if matches!(e.kind(), clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion) => {
            let _ = e.print(); // 帮助文本输出失败无处可报
            std::process::exit(0);
        }
        Err(e) => {
            let msg = e.render().to_string();
            let first = msg.lines().next().unwrap_or("invalid arguments").trim_start_matches("error: ").to_string();
            println!("{}", app::view::rejected("usage", &format!("{first}; see aq --help")));
            std::process::exit(3);
        }
    };
    let code = match cli.cmd {
        c @ (Cmd::Runner { .. } | Cmd::Detach { .. } | Cmd::Exec { .. } | Cmd::Simgc) => internal(c),
        c => user_cmd(c),
    };
    std::process::exit(code);
}
