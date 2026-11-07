#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice, unsafe_code)]
//! The REPL under a pseudo-terminal: henki is the session leader with the pty as its controlling terminal, so Ctrl-C
//! typed here reaches it as a terminal's would. A reader thread keeps every byte henki and its commands write. Every
//! wait is a bounded poll on that screen; nothing sleeps blindly.
use crate::harness::{Done, Run, bin, changed_journal};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// What the REPL writes before an approval display; the only control sequence henki itself sends.
pub const RESET: &str = "\x1b[!p\x1b[0m\x1b[r\x1b[?1049l";

/// Borrows its `Run`, whose drop deletes the directories henki works in.
pub struct Pty<'a> {
    master: File,
    child: Child,
    screen: Arc<Mutex<Vec<u8>>>,
    eof: Arc<AtomicBool>,
    /// Where the next `expect` starts looking.
    from: Cell<usize>,
    /// The terminal's device path, as `tty` prints it.
    pub name: String,
    started: Instant,
    state: PathBuf,
    before: HashMap<PathBuf, u64>,
    marker: String,
    run: PhantomData<&'a Run>,
}

impl<'a> Pty<'a> {
    /// henki with `args` under a fresh pty of `rows` x `cols`: the slave is fds 0-2; pre_exec runs setsid and
    /// TIOCSCTTY, so the pty is henki's controlling terminal and its group the foreground one.
    pub fn spawn(r: &'a Run, args: &[&str], rows: u16, cols: u16) -> Pty<'a> {
        let (mut m, mut s) = (0, 0);
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty writes two descriptors into the out-pointers; the name and termios pointers may be null and
        // the window size is read only.
        let rc = unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null(), &ws) };
        assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
        // SAFETY: openpty just returned both descriptors, and nothing else owns them.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
        for fd in [&master, &slave] {
            // SAFETY: F_SETFD on a descriptor owned here; another test's child must not inherit it.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let mut buf = [0 as libc::c_char; 256];
        // SAFETY: ttyname_r writes at most `buf.len()` bytes, NUL included, into the buffer.
        let rc = unsafe { libc::ttyname_r(slave.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
        assert_eq!(rc, 0, "ttyname_r");
        // SAFETY: on success ttyname_r left a NUL-terminated string in `buf`.
        let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
        let mut c = Command::new(bin());
        r.prepare(&mut c);
        c.args(args).stdin(slave.try_clone().unwrap()).stdout(slave.try_clone().unwrap()).stderr(slave);
        // SAFETY: runs in the forked child before exec; setsid, ioctl and signal are async-signal-safe.
        unsafe {
            c.pre_exec(|| {
                for sig in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGPIPE, libc::SIGTERM] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let before = r.sizes();
        let child = c.spawn().expect("start henki");
        drop(c); // the slave's last copies here: the master reads EIO once henki and its commands close theirs
        let master = File::from(master);
        let (screen, eof) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (mut rd, sc, e) = (master.try_clone().unwrap(), screen.clone(), eof.clone());
        thread::spawn(move || {
            let mut b = [0u8; 65536];
            loop {
                let mut pfd = libc::pollfd { fd: rd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: one valid pollfd on the stack, count 1, a 20 ms timeout.
                if unsafe { libc::poll(&mut pfd, 1, 20) } == 0 {
                    continue;
                }
                match rd.read(&mut b) {
                    Ok(k) if k > 0 => sc.lock().unwrap().extend_from_slice(&b[..k]),
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => return e.store(true, Ordering::SeqCst), // EIO: no process has the terminal open
                }
            }
        });
        Pty { master, child, screen, eof, from: Cell::new(0), name, started: Instant::now(), state: r.state(), before, marker: r.marker.clone(), run: PhantomData }
    }

    pub fn send(&self, b: &[u8]) {
        (&self.master).write_all(b).unwrap();
    }

    pub fn line(&self, s: &str) {
        self.send(format!("{s}\n").as_bytes());
    }

    /// 0x03 through the line discipline: SIGINT to the terminal's foreground group.
    pub fn ctrl_c(&self) {
        self.send(b"\x03");
    }

    /// 0x04: end of input at the start of a line.
    pub fn ctrl_d(&self) {
        self.send(b"\x04");
    }

    /// Bounded poll on the screen until `text` appears after where the last match ended; returns where this match
    /// ends (the next search starts there). Panics with the transcript.
    pub fn expect(&self, text: &str, within: Duration) -> usize {
        let end = Instant::now() + within;
        loop {
            let s = self.screen();
            if let Some(i) = s[self.from.get()..].windows(text.len()).position(|w| w == text.as_bytes()) {
                self.from.set(self.from.get() + i + text.len());
                return self.from.get();
            }
            let gone = self.eof.load(Ordering::SeqCst);
            assert!(!gone && Instant::now() < end, "{text:?} not on the screen within {within:?}{}:\n{}", if gone { " (henki ended)" } else { "" }, shown(&s));
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// The prompt `> ` at the start of a row; that row may begin right where the last match ended.
    pub fn prompt(&self, within: Duration) -> usize {
        self.from.set(self.from.get().saturating_sub(1));
        self.expect("\n> ", within)
    }

    pub fn screen(&self) -> Vec<u8> {
        self.screen.lock().unwrap().clone()
    }

    pub fn signal(&self, sig: i32) {
        // SAFETY: kill(2) on our own, unreaped child.
        unsafe { libc::kill(self.child.id() as i32, sig) };
    }

    /// The exit within `within` (a timeout kills henki and fails the test), then the screen until the terminal
    /// closes (at most 1 s more). `stdout` is the screen with the reset and "\r" of "\r\n" taken out.
    pub fn wait(mut self, within: Duration) -> Done {
        let end = Instant::now() + within;
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            if Instant::now() > end {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("henki did not exit within {within:?}:\n{}", shown(&self.screen()));
            }
            thread::sleep(Duration::from_millis(2));
        };
        let exited = Instant::now();
        let quiet = exited + Duration::from_secs(1);
        while !self.eof.load(Ordering::SeqCst) && Instant::now() < quiet {
            thread::sleep(Duration::from_millis(2));
        }
        let text = String::from_utf8_lossy(&self.screen()).replace(RESET, "").replace("\r\n", "\n");
        let journal = changed_journal(&self.state, &self.before);
        Done {
            code: status.code(),
            signal: status.signal(),
            stdout: text.into_bytes(),
            stderr: Vec::new(),
            elapsed: exited - self.started,
            exited,
            maxrss_kb: 0,
            json: false,
            journal_bytes: journal.as_ref().map(|p| std::fs::read(p).unwrap_or_default()).unwrap_or_default(),
            journal,
            killed: false,
            closed_stdout: false,
            stdout_complete: true,
            marker: self.marker.clone(),
            stdin_file: None,
            repl: true,
        }
    }
}

impl Drop for Pty<'_> {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The screen for a failure message: rows as lines, other control characters escaped.
pub fn shown(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").chars().map(|c| if c.is_control() && c != '\n' { c.escape_default().to_string() } else { c.to_string() }).collect()
}
