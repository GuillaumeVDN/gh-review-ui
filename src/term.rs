//! Terminal queries that go straight to the tty.

use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use crate::theme::Appearance;

/// OSC 11: "what is your background color?".
const QUERY: &[u8] = b"\x1b]11;?\x1b\\";

/// How long the terminal has to answer. A terminal that knows the sequence
/// answers at once; one that does not never will.
const TIMEOUT: Duration = Duration::from_millis(150);

/// Whether the terminal paints on a dark or a light ground, asked of the
/// terminal itself.
///
/// Call it before the event reader starts: the reply arrives on the same input
/// queue as the keyboard, and this reads it back itself.
pub fn background() -> Option<Appearance> {
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
    let fd = tty.as_raw_fd();
    let saved = raw_mode(fd)?;
    tty.write_all(QUERY).ok()?;
    tty.flush().ok()?;
    let reply = read_reply(fd);
    drain(fd);
    restore(fd, &saved);
    crate::theme::parse_osc11(&String::from_utf8_lossy(&reply))
}

fn raw_mode(fd: RawFd) -> Option<libc::termios> {
    unsafe {
        let mut saved: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut saved) != 0 {
            return None;
        }
        let mut raw = saved;
        libc::cfmakeraw(&mut raw);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }
        Some(saved)
    }
}

fn restore(fd: RawFd, saved: &libc::termios) {
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, saved) };
}

fn readable(fd: RawFd, timeout: Duration) -> bool {
    let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe { libc::poll(&mut poll, 1, ms) == 1 && poll.revents & libc::POLLIN != 0 }
}

fn take(fd: RawFd, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 64];
    let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
    if n <= 0 {
        return false;
    }
    buf.extend_from_slice(&chunk[..n as usize]);
    true
}

fn read_reply(fd: RawFd) -> Vec<u8> {
    let deadline = Instant::now() + TIMEOUT;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !readable(fd, left) || !take(fd, &mut buf) {
            return buf;
        }
        if buf.len() > 256 || buf.contains(&0x07) || buf.windows(2).any(|w| w == b"\x1b\\") {
            return buf;
        }
    }
}

/// Throw away what is left on the tty, so a late or partial reply does not
/// reach the event loop as keystrokes.
fn drain(fd: RawFd) {
    let mut buf = Vec::new();
    while readable(fd, Duration::ZERO) && take(fd, &mut buf) {
        if buf.len() > 4096 {
            return;
        }
    }
}
