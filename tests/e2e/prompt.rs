#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice, unsafe_code)]
//! The REPL at its prompt: a pasted block and a line typed ahead, a bare `!`, Ctrl-\, what a `!cmd` is blamed for, and
//! the ends that come from outside: the terminal closing (at the prompt, at a question and during a `!cmd`), TERM
//! during a compound `!cmd`, and a stdout nobody reads.
use crate::harness::*;
use crate::mock::*;
use crate::pty::{Pty, plain, shown, unswitched};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Generous for a loaded machine; every wait ends as soon as the text shows.
const W: Duration = Duration::from_secs(15);
const REFUSED: &str = "henki: a trust file changed; restart henki to go on (henki --resume continues this session)\r\n";
const HINT: &str = "(Ctrl-C does not quit; /exit or Ctrl-D quits)\r\n";

fn asking(m: &Mock) -> Run {
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r
}

/// The REPL on a 24 x 80 terminal, at its first prompt.
fn start(r: &Run) -> Pty<'_> {
    let p = Pty::spawn(r, &[], 24, 80);
    p.prompt(W);
    p
}

/// `/exit`, exit 0 and the postconditions.
fn exit(p: Pty<'_>) -> Done {
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    d
}

/// A block pasted where the terminal does not mark pastes: every line after the first comes with more input pending, so
/// the block is one message, and a pasted `!` line never runs.
#[test]
fn pty_paste_bang_line_not_run() {
    let m = Mock::start(vec![say("first answer streams here")]);
    let r = asking(&m);
    let pwned = r.path("PWNED");
    let p = start(&r);
    let block = format!("please explain this log:\nERROR something failed\n!touch {}", pwned.display());
    p.send(format!("{block}\n").as_bytes());
    p.expect("first answer streams here", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!pwned.exists(), "a pasted ! line ran\n{}", d.diag());
    assert_eq!(m.log().len(), 1, "a pasted line became a message\n{}", d.diag());
    assert!(m.log()[0].contents("user")[0].ends_with(&format!("\n\n{block}")), "{:?}", m.log()[0].contents("user"));
}

/// What is typed while henki works is dropped at the next prompt, with a note, so a typed-ahead `!` line never runs and
/// nothing typed ahead becomes a message.
#[test]
fn pty_typed_ahead_dropped_with_note() {
    let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 };
    let first = Step::Stream { parts: vec![say("working on it"), pause, say(" done")], finish: Some("stop".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![first, say("second reply")]);
    let r = asking(&m);
    let ran = r.path("ran.txt");
    let p = start(&r);
    p.line("first");
    p.expect("working on it", W);
    let ahead = format!("and then this\n!touch {}\n", ran.display());
    p.send(ahead.as_bytes());
    // The terminal is canonical while henki works: its echo shows both lines are in its queue.
    p.expect(&ahead.replace('\n', "\r\n"), W);
    m.open_gate();
    p.expect(" done", W);
    p.expect(&format!("henki: dropped {} bytes typed before this prompt\r\n", ahead.len()), W);
    p.prompt(W);
    p.line("next");
    p.expect("second reply", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!ran.exists(), "a typed-ahead line ran\n{}", d.diag());
    assert_eq!(m.log().len(), 2, "{}", d.diag());
    assert_eq!(m.log()[1].contents("user")[1], "next");
}

/// A block without paste marks whose last line has no newline: the line holds it and waits for Enter, and Enter sends
/// the whole block as one message.
#[test]
fn pty_paste_unended_last_line_waits() {
    let m = Mock::start(vec![say("first answer")]);
    let r = asking(&m);
    let p = start(&r);
    p.send(b"question 0\nunended");
    p.expect("question 0\r\nunended", W);
    p.send(b"\r");
    p.expect("first answer", W);
    p.prompt(W);
    let d = exit(p);
    assert!(m.log()[0].contents("user")[0].ends_with("\n\nquestion 0\nunended"), "{:?}\n{}", m.log()[0].contents("user"), d.diag());
}

/// `!` with nothing after it runs nothing and tells the model nothing.
#[test]
fn pty_bare_bang_ignored() {
    let m = Mock::start(vec![say("plain reply")]);
    let r = asking(&m);
    let p = start(&r);
    for bare in ["!", "!   ", "  !"] {
        p.line(bare);
        p.prompt(W);
    }
    p.line("hello");
    p.expect("plain reply", W);
    p.prompt(W);
    let d = exit(p);
    let users = m.log()[0].contents("user");
    assert!(users[0].ends_with("\n\nhello") && !users[0].contains("the user ran"), "{users:?}\n{}", d.diag());
}

/// Ctrl-\ at the prompt is Ctrl-C's hint, and henki keeps running.
#[test]
fn pty_ctrl_backslash_idle_hint() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r);
    p.send(b"\x1c");
    p.expect(&format!("\r\n{HINT}> "), W);
    exit(p);
}

/// A trust file changed while the REPL was idle is not blamed on the next `!cmd`, and once the REPL refuses messages
/// a later `!cmd` repeats nothing; a file the `!cmd` itself changes is named as its change.
#[test]
fn pty_bang_note_only_new_changes() {
    let m = Mock::start(vec![say("never")]);
    let r = asking(&m);
    let agents = r.config_dir().join("AGENTS.md");
    let p = start(&r);
    fs::OpenOptions::new().append(true).open(&r.config_path).unwrap().write_all(b"# edited outside\n").unwrap();
    p.line("!true");
    p.expect(&format!("henki: trust file changed: {}\r\n", r.config_path.display()), W);
    p.prompt(W);
    p.line("!true");
    p.prompt(W);
    p.line(&format!("!touch {}", agents.display()));
    p.expect(&format!("henki: {} changed by !cmd; restart henki to apply it (henki --resume continues this session)\r\n", agents.display()), W);
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    let d = exit(p);
    let screen = d.out();
    assert!(!screen.contains("config.toml changed by !cmd"), "an outside change blamed on !cmd:\n{screen}");
    assert_eq!(screen.matches("trust file changed: ").count(), 1, "the change noted again:\n{screen}");
    assert!(m.log().is_empty());
}

/// TERM during a `!cmd` whose bash does not exec the command: exit 130 within 1 s and nothing of it left.
#[test]
fn pty_bang_term_exits_compound() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r);
    p.line(&format!("!{}", r.sleep()));
    assert!(poll(W, || marked(&r.marker).iter().any(|(_, c)| c.starts_with("sleep "))), "bash did not start the sleep");
    let t = Instant::now();
    p.signal(libc::SIGTERM);
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert!(d.exited - t <= Duration::from_secs(1), "exit {:?} after SIGTERM", d.exited - t);
}

/// The terminal closing at the prompt is a hangup, exit 130, never the Ctrl-D exit 0; several rounds, since the end of
/// input and SIGHUP reach henki in either order.
#[test]
fn pty_hangup_at_prompt_130() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    for round in 0..5 {
        let t = Term::spawn(&r, false);
        t.expect("\n> ", W);
        let d = t.hang_up();
        assert_eq!(d.code, Some(130), "round {round}: {}", d.diag());
        assert!(d.journal.is_none(), "round {round}: a session without a message");
    }
}

/// The terminal closing while a question waits: exit 130, the question is not answered (no `approval` record), and no
/// later call of the turn runs or is asked; several rounds, since the end of input and SIGHUP reach henki in either order.
#[test]
fn pty_hangup_at_question_130() {
    for round in 0..3 {
        let m = Mock::start(vec![shell("touch hup.txt"), shell("touch hup2.txt"), say("never")]);
        let r = asking(&m);
        let t = Term::spawn(&r, false);
        t.expect("\n> ", W);
        t.line("make the files");
        t.expect("run? [y/N/always] ", W);
        let d = t.hang_up();
        assert_eq!(d.code, Some(130), "round {round}: {}", d.diag());
        assert_eq!((d.of("ask").len(), d.of("approval").len(), m.log().len()), (1, 0, 1), "round {round}: {}", d.diag());
        assert_eq!(d.results()[0]["content"], "not run: interrupted", "round {round}: {}", d.diag());
        assert!(!r.path("hup.txt").exists() && !r.path("hup2.txt").exists());
    }
}

/// The terminal closing during a `!cmd`: exit 130, nothing of the command left, and the journal ends with `close`.
#[test]
fn pty_hangup_during_bang() {
    let m = Mock::start(vec![say("first reply")]);
    let r = asking(&m);
    let t = Term::spawn(&r, false);
    t.expect("\n> ", W);
    t.line("hello");
    t.expect("first reply", W);
    t.expect("\n> ", W);
    t.line(&format!("!{}", r.sleep()));
    assert!(poll(W, || marked(&r.marker).iter().any(|(_, c)| c.starts_with("sleep "))), "bash did not start the sleep");
    let d = t.hang_up();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    let kinds = d.kinds();
    assert!(kinds.starts_with(&["session", "user", "assistant", "turn_end"].map(String::from)) && kinds.last().is_some_and(|k| k == "close"), "{}", d.diag());
}

/// A stdout nobody reads: the answer's first write fails, and whether the turn or the next prompt finds that out, the
/// REPL says `(output closed)` once on the terminal and exits 141.
#[test]
fn pty_stdout_closed_141() {
    let m = Mock::start(vec![say("an answer nobody reads")]);
    let r = asking(&m);
    let t = Term::spawn(&r, true);
    t.expect("\n> ", W);
    t.line("hello");
    t.expect("(output closed)\r\n", W);
    let d = t.wait(W);
    d.finish();
    assert_eq!(d.code, Some(141), "{}", d.diag());
    assert_eq!(d.out().matches("(output closed)").count(), 1, "{}", d.diag());
}

/// henki on a pty whose master this test can close, as a terminal window closing does (`Pty` keeps a copy of the
/// master open to the end). With `closed_stdout`, stdout is a pipe whose reading end is already closed.
struct Term<'a> {
    master: Option<File>,
    child: Child,
    screen: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    from: Cell<usize>,
    run: &'a Run,
    before: HashMap<PathBuf, u64>,
    started: Instant,
}

impl<'a> Term<'a> {
    fn spawn(r: &'a Run, closed_stdout: bool) -> Term<'a> {
        let (mut m, mut s) = (0, 0);
        let ws = libc::winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty writes two descriptors into the out-pointers; the name and termios pointers may be null and
        // the window size is read only.
        assert_eq!(unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null(), &ws) }, 0, "openpty");
        // SAFETY: openpty just returned both descriptors, and nothing else owns them.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(m), OwnedFd::from_raw_fd(s)) };
        for fd in [&master, &slave] {
            // SAFETY: F_SETFD on a descriptor owned here; another test's child must not inherit it.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let mut c = std::process::Command::new(bin());
        r.prepare(&mut c);
        c.stdin(slave.try_clone().unwrap()).stderr(slave.try_clone().unwrap());
        if closed_stdout {
            let (rd, wr) = std::io::pipe().unwrap();
            drop(rd);
            c.stdout(wr);
        } else {
            c.stdout(slave);
        }
        // SAFETY: runs in the forked child before exec; signal, setsid and ioctl are async-signal-safe.
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
        drop(c); // the slave's last copies here
        let master = File::from(master);
        let (screen, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (mut rd, sc, st) = (master.try_clone().unwrap(), screen.clone(), stop.clone());
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
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    _ => return, // EIO: no process has the terminal open
                }
            }
        });
        Term { master: Some(master), child, screen, stop, reader: Some(reader), from: Cell::new(0), run: r, before, started: Instant::now() }
    }

    fn line(&self, s: &str) {
        self.master.as_ref().unwrap().write_all(format!("{s}\n").as_bytes()).unwrap();
    }

    fn screen(&self) -> Vec<u8> {
        unswitched(&self.screen.lock().unwrap())
    }

    /// Bounded poll on the screen for `text` after the last match.
    fn expect(&self, text: &str, within: Duration) {
        let end = Instant::now() + within;
        loop {
            let s = self.screen();
            if let Some(i) = s[self.from.get()..].windows(text.len()).position(|w| w == text.as_bytes()) {
                return self.from.set(self.from.get() + i + text.len());
            }
            assert!(Instant::now() < end, "{text:?} not on the screen within {within:?}:\n{}", shown(&s));
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Closes every copy of the master, then waits for the end and checks the postconditions.
    fn hang_up(mut self) -> Done {
        self.stop.store(true, Ordering::SeqCst);
        self.reader.take().unwrap().join().unwrap();
        drop(self.master.take());
        let d = self.wait(W);
        d.finish();
        d
    }

    /// The exit within `within` (a timeout kills henki and fails the test). `stdout` is the screen with the reset and
    /// "\r" of "\r\n" taken out, as `Pty::wait` has it.
    fn wait(mut self, within: Duration) -> Done {
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
        let journal = changed_journal(&self.run.state(), &self.before);
        Done {
            code: status.code(),
            signal: status.signal(),
            stdout: plain(&self.screen()).into_bytes(),
            stderr: Vec::new(),
            elapsed: exited - self.started,
            exited,
            maxrss_kb: 0,
            json: false,
            journal_bytes: journal.as_ref().map(|p| fs::read(p).unwrap_or_default()).unwrap_or_default(),
            journal,
            killed: false,
            closed_stdout: false,
            stdout_complete: true,
            marker: self.run.marker.clone(),
            stdin_file: None,
            repl: true,
        }
    }
}

impl Drop for Term<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
