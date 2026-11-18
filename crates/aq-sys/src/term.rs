//! 终端：标准输出是不是终端、有多宽。只给 status --top 用。

use std::io::IsTerminal;

pub fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// 标准输出所在终端的列数；不是终端或取不到时为 None。
pub fn width() -> Option<usize> {
    let mut ws: libc::winsize = crate::procs_zeroed();
    // SAFETY: TIOCGWINSZ 只把窗口大小写进 ws，ws 是有效的可写 winsize。
    let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    (rc == 0 && ws.ws_col > 0).then_some(ws.ws_col as usize)
}
