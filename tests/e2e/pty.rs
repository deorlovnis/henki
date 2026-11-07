#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice, unsafe_code)]
//! The REPL under a pseudo-terminal: henki is the session leader with the pty as its controlling terminal, so Ctrl-C
//! typed here reaches it as a terminal's would. A reader thread keeps every byte henki and its commands write. Every
//! wait is a bounded poll on that screen; nothing sleeps blindly. Every run that ends with the terminal still open is
//! held to two things: the terminal modes are the ones it started with, and bracketed paste is off.
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
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// What the REPL writes before an approval display. It ends on a newly scrolled bottom row, so the display lands below
/// everything already on the screen.
pub const RESET: &str = "\x1b[!p\x1b[0m\x1b[r\x1b[?1049l\x1b[?7h\x1b[999;1H\x1bD";
/// Bracketed paste on and off, in order with everything else henki writes.
pub const PASTE_ON: &str = "\x1b[?2004h";
pub const PASTE_OFF: &str = "\x1b[?2004l";
/// The line editor's redraws: up a row, and from column 0 everything below cleared.
const REDRAW: [&str; 2] = ["\x1b[A", "\r\x1b[J"];

/// Borrows its `Run`, whose drop deletes the directories henki works in.
pub struct Pty<'a> {
    /// None once `hang_up` closed it.
    master: Option<File>,
    child: Child,
    screen: Arc<Mutex<Vec<u8>>>,
    /// Set when the reader thread ended: no process has the terminal open, or `hang_up` stopped it.
    eof: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    /// Where the next `expect` starts looking.
    from: Cell<usize>,
    /// The terminal's device path, as `tty` prints it.
    pub name: String,
    started: Instant,
    /// The terminal modes before henki started.
    pub initial: libc::termios,
    state: PathBuf,
    before: HashMap<PathBuf, u64>,
    marker: String,
    run: PhantomData<&'a Run>,
}

impl<'a> Pty<'a> {
    /// henki with `args` under a fresh pty of `rows` x `cols`: the slave is fds 0-2; pre_exec runs setsid and
    /// TIOCSCTTY, so the pty is henki's controlling terminal and its group the foreground one. TERM is
    /// `xterm-256color`, as a terminal emulator sets it, unless the run sets its own.
    pub fn spawn(r: &'a Run, args: &[&str], rows: u16, cols: u16) -> Pty<'a> {
        Pty::spawn_to(r, args, rows, cols, None)
    }

    /// The same, with henki's stdout to `stdout` when given (the pty stays its stdin and stderr).
    pub fn spawn_to(r: &'a Run, args: &[&str], rows: u16, cols: u16, stdout: Option<File>) -> Pty<'a> {
        Pty::spawn_io(r, args, rows, cols, stdout, None)
    }

    /// The same, with henki's stdout and stderr to the files given (the pty stays its stdin).
    pub fn spawn_io(r: &'a Run, args: &[&str], rows: u16, cols: u16, stdout: Option<File>, stderr: Option<File>) -> Pty<'a> {
        let (mut m, mut s) = (0, 0);
        let mut ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty writes two descriptors into the out-pointers; the name and termios pointers may be null and
        // the window size is only read (macOS declares the termios and window size pointers mutable).
        let rc = unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), &raw mut ws) };
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
        let initial = modes(slave.as_raw_fd());
        let mut c = Command::new(bin());
        r.prepare(&mut c);
        if !c.get_envs().any(|(k, _)| k == "TERM") {
            c.env("TERM", "xterm-256color");
        }
        c.args(args).stdin(slave.try_clone().unwrap());
        c.stderr(stderr.unwrap_or_else(|| File::from(slave.try_clone().unwrap())));
        match stdout {
            Some(f) => c.stdout(f),
            None => c.stdout(slave),
        };
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
        let (screen, eof, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (mut rd, sc, e, st) = (master.try_clone().unwrap(), screen.clone(), eof.clone(), stop.clone());
        let reader = thread::spawn(move || {
            let mut b = [0u8; 65536];
            while !st.load(Ordering::SeqCst) {
                let mut pfd = libc::pollfd { fd: rd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: one valid pollfd on the stack, count 1, a 20 ms timeout.
                if unsafe { libc::poll(&mut pfd, 1, 20) } == 0 {
                    continue;
                }
                match rd.read(&mut b) {
                    Ok(k) if k > 0 => sc.lock().unwrap().extend_from_slice(&b[..k]),
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => break, // EIO: no process has the terminal open
                }
            }
            e.store(true, Ordering::SeqCst);
        });
        let (from, started, state, marker) = (Cell::new(0), Instant::now(), r.state(), r.marker.clone());
        Pty { master: Some(master), child, screen, eof, stop, reader: Some(reader), from, name, started, initial, state, before, marker, run: PhantomData }
    }

    /// The terminal's modes now (tcgetattr on the master gives the terminal's own).
    pub fn modes(&self) -> libc::termios {
        modes(self.master.as_ref().expect("the terminal is open").as_raw_fd())
    }

    /// `text` as a terminal with bracketed paste on sends a paste.
    pub fn paste(&self, text: &str) {
        self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
    }

    pub fn send(&self, b: &[u8]) {
        self.master.as_ref().expect("the terminal is open").write_all(b).unwrap();
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

    /// Everything written to the terminal, without the bracketed paste switches.
    pub fn screen(&self) -> Vec<u8> {
        unswitched(&self.raw_screen())
    }

    /// Everything written to the terminal.
    pub fn raw_screen(&self) -> Vec<u8> {
        self.screen.lock().unwrap().clone()
    }

    /// The terminal goes away, as when its window closes: every copy of the master is closed (the reader thread's
    /// too, after it took what was on the screen), so henki gets SIGHUP and its reads end. Then `wait`.
    pub fn hang_up(mut self, within: Duration) -> Done {
        self.stop.store(true, Ordering::SeqCst);
        self.reader.take().unwrap().join().unwrap();
        drop(self.master.take());
        self.wait(within)
    }

    pub fn signal(&self, sig: i32) {
        // SAFETY: kill(2) on our own, unreaped child.
        unsafe { libc::kill(self.child.id() as i32, sig) };
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only a Linux test uses it
    pub fn pid(&self) -> i32 {
        self.child.id() as i32
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
        if self.master.is_some() {
            assert!(same_modes(&self.modes(), &self.initial), "the terminal modes differ after the exit:\n{}", shown(&self.raw_screen()));
            assert!(paste_off(&self.raw_screen()), "bracketed paste is on after the exit:\n{}", shown(&self.raw_screen()));
        }
        let text = plain(&self.raw_screen());
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

/// tcgetattr(fd).
pub fn modes(fd: i32) -> libc::termios {
    // SAFETY: an all-zero termios is a valid value to overwrite.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: a live termios, which tcgetattr fills.
    assert_eq!(unsafe { libc::tcgetattr(fd, &mut t) }, 0, "tcgetattr: {}", std::io::Error::last_os_error());
    t
}

/// The same input, output, control and local modes and control characters.
pub fn same_modes(a: &libc::termios, b: &libc::termios) -> bool {
    (a.c_iflag, a.c_oflag, a.c_cflag, a.c_lflag, a.c_cc) == (b.c_iflag, b.c_oflag, b.c_cflag, b.c_lflag, b.c_cc)
}

/// The last bracketed paste switch on `screen`, if any, turns it off.
pub fn paste_off(screen: &[u8]) -> bool {
    let s = String::from_utf8_lossy(screen);
    s.rfind(PASTE_ON).is_none_or(|on| s.rfind(PASTE_OFF).is_some_and(|off| off > on))
}

/// At every place `text` is on `screen`, bracketed paste is on: the last switch before it turns it on, as a terminal
/// that acts on each switch in the order it gets them would have it (so a paste comes with its marks). False when `text`
/// is not on the screen.
pub fn paste_on_at(screen: &[u8], text: &str) -> bool {
    let s = String::from_utf8_lossy(screen);
    let on = |at: usize| s[..at].rfind(PASTE_ON).is_some_and(|on| s[..at].rfind(PASTE_OFF).is_none_or(|off| off < on));
    s.match_indices(text).count() > 0 && s.match_indices(text).all(|(at, _)| on(at))
}

/// `screen` without the bracketed paste switches.
pub fn unswitched(screen: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(screen).replace(PASTE_ON, "").replace(PASTE_OFF, "").into_bytes()
}

/// The screen as text, "\r\n" as "\n", without what henki writes besides text: the reset, the bracketed paste switches and
/// the line editor's redraws.
pub fn plain(screen: &[u8]) -> String {
    let s = String::from_utf8_lossy(&unswitched(screen)).replace(RESET, "");
    REDRAW.iter().fold(s, |s, r| s.replace(r, "")).replace("\r\n", "\n")
}

/// The screen for a failure message: rows as lines, other control characters escaped.
pub fn shown(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").chars().map(|c| if c.is_control() && c != '\n' { c.escape_default().to_string() } else { c.to_string() }).collect()
}
