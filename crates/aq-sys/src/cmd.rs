//! 带超时调用外部命令（git、simctl、AI CLI）。超时后杀掉整个进程组。

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

pub struct Out {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub struct Call<'a> {
    pub argv: &'a [String],
    pub cwd: Option<&'a Path>,
    pub env: &'a [(&'a str, &'a str)],
    pub stdin: Option<&'a [u8]>,
    pub timeout: Duration,
}

fn reader<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = vec![];
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf); // 读失败时返回已读部分，由调用方按内容判断
        }
        buf
    })
}

pub fn run(c: &Call<'_>) -> Result<Out, String> {
    let (prog, args) = c.argv.split_first().ok_or("empty command")?;
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    cmd.stdin(if c.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    if let Some(d) = c.cwd {
        cmd.current_dir(d);
    }
    for (k, v) in c.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot execute {prog}: {e}"))?;
    if let (Some(input), Some(mut w)) = (c.stdin, child.stdin.take()) {
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = w.write_all(&input); // 对方提前退出时写失败无妨，结果按输出判断
        });
    }
    let out = reader(child.stdout.take());
    let err = reader(child.stderr.take());
    let status = match child.wait_timeout(c.timeout).map_err(|e| e.to_string())? {
        Some(s) => s,
        None => {
            // SAFETY: killpg 只发信号。子进程是自己进程组的组长。
            unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
            let _ = child.wait(); // 回收僵尸进程；已强杀，等待结果不重要
            return Err(format!("{prog} did not return within {}s, stopped", c.timeout.as_secs()));
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    Ok(Out { status: status.code().unwrap_or(-1), stdout, stderr })
}

/// 运行并要求退出码为 0，返回标准输出文本。
pub fn ok_text(c: &Call<'_>) -> Result<String, String> {
    let o = run(c)?;
    if o.status != 0 {
        return Err(format!("{} exited with {}: {}", c.argv.join(" "), o.status, o.stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}
