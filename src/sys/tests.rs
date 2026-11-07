#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A fresh, empty directory under the temp dir, removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-sys-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mkfifo(path: &Path) {
    let c = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a NUL-terminated path that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {}", path.display());
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

/// Runs `f` on a thread and fails the test if it does not return within 5 s (a blocking open would hang it).
fn within_5s<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(f()));
    rx.recv_timeout(Duration::from_secs(5)).expect("blocked for 5 s")
}

#[test]
fn fnv_vectors() {
    assert_eq!(fnv(b""), "cbf29ce484222325");
    assert_eq!(fnv(b"a"), "af63dc4c8601ec8c");
    assert_eq!(fnv(b"foobar"), "85944171f73967e8");
}

#[test]
fn utc_epochs() {
    assert_eq!(utc_at(0), ("1970-01-01T00:00:00Z".into(), "19700101T000000Z".into()));
    assert_eq!(utc_at(951_782_400), ("2000-02-29T00:00:00Z".into(), "20000229T000000Z".into()));
    assert_eq!(utc_at(4_102_444_800), ("2100-01-01T00:00:00Z".into(), "21000101T000000Z".into()));
    let (long, short) = utc();
    assert!(long.len() == 20 && long.ends_with('Z') && long.as_bytes()[10] == b'T', "{long}");
    assert!(short.len() == 16 && short.ends_with('Z') && short.as_bytes()[8] == b'T', "{short}");
    assert!(long.starts_with("20"), "{long}");
}

#[test]
fn local_date_format() {
    let d = local_date();
    assert_eq!(d.len(), 10, "{d}");
    for (i, b) in d.bytes().enumerate() {
        assert!(if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }, "{d}");
    }
    assert!(d.starts_with("20"), "{d}");
}

#[test]
fn random4_hex() {
    let ids: Vec<String> = (0..20).map(|_| random4().unwrap()).collect();
    for r in &ids {
        assert_eq!(r.len(), 4, "{r}");
        assert!(r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{r}");
    }
    assert!(ids.iter().any(|r| r != &ids[0]), "20 equal draws: {ids:?}");
}

#[test]
fn tree_descendants() {
    let me = std::process::id() as i32;
    let p = |pid, ppid, sid, zombie| Proc { pid, ppid, sid, zombie };
    let ps = [
        p(10, 1, 10, false),  // the call's shell, session leader
        p(11, 10, 10, false), // its child
        p(12, 11, 12, false), // a grandchild that called setsid
        p(13, 11, 10, true),  // a zombie child: excluded
        p(14, 12, 12, false), // below the setsid grandchild
        p(20, 1, 20, false),  // unrelated
        p(me, 10, 10, false), // henki itself: never included
        p(30, me, 30, false), // henki's own child
    ];
    assert_eq!(tree(&ps, |p| p.pid == 10), [10, 11, 12, 14]);
    assert_eq!(tree(&ps, |p| p.sid == 10), [10, 11, 12, 14]);
    assert_eq!(tree(&ps, |p| p.ppid == me), [30]);
    assert_eq!(tree(&ps, |p| p.pid == 13), Vec::<i32>::new());
    assert_eq!(tree(&ps, |p| p.pid == me), Vec::<i32>::new());
    assert_eq!(tree(&[], |_| true), Vec::<i32>::new());
}

#[cfg(target_os = "linux")]
#[test]
fn procs_sees_self() {
    let me = std::process::id() as i32;
    let ps = procs();
    let mine = ps.iter().find(|p| p.pid == me).expect("own pid in /proc");
    // SAFETY: getppid and getsid take no pointers.
    let (ppid, sid) = unsafe { (libc::getppid(), libc::getsid(0)) };
    assert_eq!(*mine, Proc { pid: me, ppid, sid, zombie: false });
    assert!(ps.len() > 1);
    // A command name with spaces and parentheses still parses from the fields after the last ')'.
    assert_eq!(stat(7, "7 (a) b (c)) Z 3 4 5 0 0"), Some(Proc { pid: 7, ppid: 3, sid: 5, zombie: true }));
    assert_eq!(stat(7, "7 (truncated"), None);
}

#[test]
fn open_regular_refuses_fifo() {
    let t = Tmp::new();
    let f = t.join("fifo");
    mkfifo(&f);
    let err = within_5s(move || open_regular(&f).map(|_| ())).unwrap_err();
    assert_eq!(err, format!("not a regular file: {} (fifo)", t.join("fifo").display()));
}

#[test]
fn open_regular_refuses_dir() {
    let t = Tmp::new();
    assert_eq!(open_regular(&t.0).map(|_| ()).unwrap_err(), format!("not a regular file: {} (directory)", t.0.display()));
    let f = t.join("file");
    fs::write(&f, "12345").unwrap();
    assert_eq!(open_regular(&f).unwrap().1, 5);
    assert!(open_regular(&t.join("missing")).unwrap_err().starts_with(&format!("{}: ", t.join("missing").display())));
}

#[test]
fn open_read_fifo_does_not_block() {
    let t = Tmp::new();
    let f = t.join("fifo");
    mkfifo(&f);
    let (_, m) = within_5s(move || open_read(&f)).unwrap();
    assert!(m.file_type().is_fifo());
}

#[test]
fn create_excl_eexist() {
    let t = Tmp::new();
    let f = t.join("f");
    create_excl(&f, 0o600, false).unwrap().write_all(b"first").unwrap();
    assert_eq!(create_excl(&f, 0o600, false).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&f).unwrap(), b"first");
    assert_eq!(mode(&f), 0o600);
}

#[test]
fn create_excl_nofollow() {
    let t = Tmp::new();
    symlink(t.join("target"), t.join("dangling")).unwrap();
    assert!(create_excl(&t.join("dangling"), 0o600, false).is_err());
    assert!(!t.join("target").exists(), "created through a dangling symlink");
    fs::write(t.join("real"), "keep").unwrap();
    symlink(t.join("real"), t.join("link")).unwrap();
    assert!(create_excl(&t.join("link"), 0o600, true).is_err());
    assert_eq!(fs::read_to_string(t.join("real")).unwrap(), "keep");
}

#[test]
fn create_excl_append() {
    let t = Tmp::new();
    let f = t.join("journal");
    let mut file = create_excl(&f, 0o600, true).unwrap();
    file.write_all(b"one\n").unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"two\n").unwrap();
    assert_eq!(fs::read_to_string(&f).unwrap(), "one\ntwo\n");
    let mut plain = create_excl(&t.join("spill"), 0o600, false).unwrap();
    plain.write_all(b"abc").unwrap();
    plain.seek(SeekFrom::Start(0)).unwrap();
    plain.write_all(b"X").unwrap();
    assert_eq!(fs::read_to_string(t.join("spill")).unwrap(), "Xbc");
}

#[test]
fn open_append_writes_at_end() {
    let t = Tmp::new();
    let f = t.join("journal");
    fs::write(&f, "one\ntwo\ntorn").unwrap();
    let mut file = open_append(&f).unwrap();
    let mut s = String::new();
    file.read_to_string(&mut s).unwrap();
    assert_eq!(s, "one\ntwo\ntorn");
    file.set_len(8).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"three\n").unwrap();
    assert_eq!(fs::read_to_string(&f).unwrap(), "one\ntwo\nthree\n");
    symlink(&f, t.join("link")).unwrap();
    assert!(open_append(&t.join("link")).is_err());
    assert!(open_append(&t.0).is_err());
}

#[test]
fn open_append_refuses_fifo() {
    let t = Tmp::new();
    let f = t.join("fifo");
    mkfifo(&f);
    assert!(within_5s(move || open_append(&f)).is_err());
}

#[test]
fn private_dir_tightens_mode() {
    let t = Tmp::new();
    let d = t.join("state");
    fs::DirBuilder::new().mode(0o755).create(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o775)).unwrap();
    private_dir(&d).unwrap();
    assert_eq!(mode(&d), 0o700);
    fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).unwrap();
    private_dir(&d).unwrap();
    assert_eq!(mode(&d), 0o500, "a narrower mode is left alone");
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn private_dir_refuses_symlink() {
    let t = Tmp::new();
    let real = t.join("real");
    fs::DirBuilder::new().mode(0o755).create(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&real, t.join("link")).unwrap();
    assert!(private_dir(&t.join("link")).is_err());
    assert_eq!(mode(&real), 0o755, "the symlink's target was changed");
    fs::write(t.join("file"), "").unwrap();
    assert!(private_dir(&t.join("file")).is_err());
}

#[test]
fn private_dir_refuses_foreign() {
    let me = euid();
    assert!(owner(me, me).is_ok());
    let err = owner(me.wrapping_add(1), me).unwrap_err();
    assert_eq!(err.to_string(), format!("owned by uid {}, not by uid {me}", me.wrapping_add(1)));
    if me != 0 {
        assert!(owner(0, me).is_err(), "a root-owned dir is foreign too");
    }
}

#[test]
fn private_dir_creates_ancestors_0700() {
    let t = Tmp::new();
    let d = t.join("a/b/c");
    private_dir(&d).unwrap();
    for p in ["a", "a/b", "a/b/c"] {
        assert_eq!(mode(&t.join(p)) & 0o077, 0, "{p} is open to others");
        assert!(t.join(p).is_dir());
    }
    assert_eq!(mode(&d), 0o700);
    private_dir(&d).unwrap();
}

#[test]
fn dir_sub_refuses_symlink() {
    let t = Tmp::new();
    fs::create_dir(t.join("real")).unwrap();
    symlink(t.join("real"), t.join("link")).unwrap();
    let d = Dir::open(&t.0).unwrap();
    assert!(d.sub(OsStr::new("link"), false).is_err());
    assert!(d.sub(OsStr::new("link"), true).is_err());
    assert!(Dir::open(&t.join("link")).is_err());
    for bad in ["", ".", "..", "real/x", "a\0b"] {
        assert_eq!(d.sub(OsStr::new(bad), true).err().map(|e| e.kind()), Some(io::ErrorKind::InvalidInput), "{bad:?}");
    }
    let sub = d.sub(OsStr::new("new"), true).unwrap();
    assert!(t.join("new").is_dir());
    assert_eq!(sub.id().unwrap(), Dir::open(&t.join("new")).unwrap().id().unwrap());
    assert!(d.sub(OsStr::new("new"), true).is_ok(), "an existing directory is fine");
    assert!(d.sub(OsStr::new("real"), false).is_ok());
}

#[test]
fn dir_create_rename_unlink() {
    let t = Tmp::new();
    let d = Dir::open(&t.0).unwrap();
    let m = fs::metadata(&t.0).unwrap();
    assert_eq!(d.id().unwrap(), (m.dev(), m.ino()));
    let mut f = d.create(OsStr::new(".f.tmp"), 0o600).unwrap();
    f.write_all(b"new").unwrap();
    assert_eq!(mode(&t.join(".f.tmp")), 0o600);
    assert_eq!(d.create(OsStr::new(".f.tmp"), 0o600).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    fs::write(t.join("f"), "old").unwrap();
    d.rename(OsStr::new(".f.tmp"), OsStr::new("f")).unwrap();
    assert_eq!(fs::read_to_string(t.join("f")).unwrap(), "new");
    assert!(!t.join(".f.tmp").exists());
    symlink(t.join("elsewhere"), t.join("link")).unwrap();
    assert!(d.create(OsStr::new("link"), 0o600).is_err());
    assert!(!t.join("elsewhere").exists());
    d.unlink(OsStr::new("f"));
    assert!(!t.join("f").exists());
    d.unlink(OsStr::new("f"));
    d.unlink(OsStr::new("../escape"));
}

#[test]
fn kill_esrch_false() {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id() as i32;
    child.wait().unwrap();
    assert!(!kill(pid, 0), "a reaped pid");
    assert!(kill(std::process::id() as i32, 0), "henki itself");
    assert!(!kill(0, 0) && !kill(-1, 0), "never the own group or every process");
    assert!(!reap(pid) && !reap(0) && !reap(-1));
}

#[test]
fn isatty_dev_null_false() {
    let f = File::open("/dev/null").unwrap();
    assert!(!isatty(f.as_raw_fd()));
    assert!(!isatty(-1));
}

#[test]
fn winsize_not_a_tty() {
    let f = File::open("/dev/null").unwrap();
    assert_eq!(winsize(f.as_raw_fd()), None);
    let (r, _w) = io::pipe().unwrap();
    assert_eq!(winsize(r.as_raw_fd()), None);
}

#[test]
fn tty_save_not_a_tty() {
    let f = File::open("/dev/null").unwrap();
    assert!(modes(f.as_raw_fd()).is_none());
    let (r, _w) = io::pipe().unwrap();
    assert!(modes(r.as_raw_fd()).is_none());
    // Saving the modes of a real terminal would change this whole test process, so fd 0 is tried only when it is not one.
    if !isatty(0) {
        assert!(!tty_save());
        assert!(TTY.get().is_none());
    }
}

#[test]
fn flush_input_not_a_tty() {
    // No unit test saves terminal modes, so there is nothing to restore and nothing is flushed.
    assert!(TTY.get().is_none());
    flush_input();
    assert!(TTY.get().is_none());
}

#[test]
fn stamp_missing() {
    let t = Tmp::new();
    assert_eq!(stamp(&t.join("none")), (None, None));
    symlink(t.join("none"), t.join("dangling")).unwrap();
    let (l, s) = stamp(&t.join("dangling"));
    assert!(l.is_some() && s.is_none(), "a dangling link has its own stamp only");
}

#[test]
fn stamp_changes_on_write() {
    let t = Tmp::new();
    let f = t.join("allow");
    fs::write(&f, "a\n").unwrap();
    let before = stamp(&f);
    assert!(before.0.is_some() && before.0 == before.1);
    fs::OpenOptions::new().append(true).open(&f).unwrap().write_all(b"b\n").unwrap();
    let after = stamp(&f);
    assert_ne!(after, before);
    assert_eq!(after.1.unwrap()[2], 4, "the size is part of the stamp");
    // Rewritten to the same size: the change time still differs or the inode does.
    let same = stamp(&f);
    fs::write(t.join("new"), "c\nd\n").unwrap();
    fs::rename(t.join("new"), &f).unwrap();
    assert_ne!(stamp(&f), same);
}

#[test]
fn stamp_changes_on_symlink_retarget() {
    let t = Tmp::new();
    fs::write(t.join("a"), "same").unwrap();
    fs::write(t.join("bb"), "same").unwrap();
    symlink(t.join("a"), t.join("link")).unwrap();
    let before = stamp(&t.join("link"));
    assert_ne!(before.0, before.1, "the link and its target have their own stamps");
    fs::remove_file(t.join("link")).unwrap();
    symlink(t.join("bb"), t.join("link")).unwrap();
    let after = stamp(&t.join("link"));
    assert_ne!(after.0, before.0, "the link itself changed");
    assert_ne!(after.1, before.1, "it leads to another file");
}

#[test]
fn stamp_same_when_untouched() {
    let t = Tmp::new();
    fs::write(t.join("f"), "x").unwrap();
    symlink(t.join("f"), t.join("link")).unwrap();
    for p in [t.join("f"), t.join("link"), t.join("missing")] {
        assert_eq!(stamp(&p), stamp(&p), "{}", p.display());
    }
    // Reading a file is not a change.
    let before = stamp(&t.join("f"));
    let _ = fs::read(t.join("f")).unwrap();
    assert_eq!(stamp(&t.join("f")), before);
}

/// A fresh pty pair (master, slave), close-on-exec, closed when dropped.
fn pty() -> (File, File) {
    let (mut m, mut s) = (-1, -1);
    // SAFETY: two live out pointers; a null name, modes and size are allowed and leave the defaults.
    let rc = unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    assert_eq!(rc, 0, "openpty: {}", io::Error::last_os_error());
    for fd in [m, s] {
        // SAFETY: integer arguments only, on a descriptor openpty just returned.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // SAFETY: openpty returned two new descriptors that nothing else owns.
    unsafe { (File::from_raw_fd(m), File::from_raw_fd(s)) }
}

fn fl(fd: i32) -> i32 {
    // SAFETY: integer arguments only.
    unsafe { libc::fcntl(fd, libc::F_GETFL) }
}

fn set_fl(fd: i32, flags: i32) {
    // SAFETY: integer arguments only.
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) }, 0);
}

/// The bytes waiting to be read on `fd` (FIONREAD).
fn pending(fd: i32) -> i32 {
    let mut n: libc::c_int = 0;
    // SAFETY: the pointer is to a live int, the type FIONREAD writes.
    assert_eq!(unsafe { libc::ioctl(fd, libc::FIONREAD, &mut n) }, 0);
    n
}

#[test]
fn isatty_pty_true() {
    let (_m, s) = pty();
    assert!(isatty(s.as_raw_fd()));
}

#[test]
fn winsize_pty_size() {
    let (m, s) = pty();
    let set = |rows: u16, cols: u16| {
        let w = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: the pointer is to a live winsize, the type TIOCSWINSZ reads.
        assert_eq!(unsafe { libc::ioctl(m.as_raw_fd(), libc::TIOCSWINSZ, &w) }, 0);
    };
    set(30, 100);
    assert_eq!(winsize(s.as_raw_fd()), Some((30, 100)));
    set(30, 0);
    assert_eq!(winsize(s.as_raw_fd()), None, "0 columns is no size");
}

#[test]
fn modes_pty_some() {
    let (_m, s) = pty();
    let t = modes(s.as_raw_fd()).expect("a terminal has modes");
    assert_ne!(t.c_lflag & libc::ICANON, 0, "a new pty is canonical");
}

#[test]
fn blocking_clears_nonblocking() {
    let (_m, s) = pty();
    let (r, _w) = io::pipe().unwrap();
    for fd in [s.as_raw_fd(), r.as_raw_fd()] {
        set_fl(fd, fl(fd) | libc::O_NONBLOCK | libc::O_APPEND);
        blocking(fd);
        assert_eq!(fl(fd) & libc::O_NONBLOCK, 0);
        assert_ne!(fl(fd) & libc::O_APPEND, 0, "the other flags stay");
        blocking(fd);
        assert_eq!(fl(fd) & libc::O_NONBLOCK, 0);
    }
    blocking(-1);
}

/// Set in the child process `flush_input_restores_a_pty` starts.
const PTY_CHILD: &str = "HENKI_TEST_PTY_CHILD";

/// `tty_save` and `flush_input` on a terminal. They act on fd 0, so the test runs this same test again in a child
/// process whose fd 0 is a fresh pty: the test process's own fd 0 is never touched. The parent leaves the pty
/// non-blocking with a line typed ahead; the child checks that `tty_save` clears O_NONBLOCK and keeps the typed line,
/// then that `flush_input` restores the modes it saved, drops what was typed ahead and clears O_NONBLOCK again.
#[test]
fn flush_input_restores_a_pty() {
    if std::env::var_os(PTY_CHILD).is_some() {
        assert!(fl(0) & libc::O_NONBLOCK != 0 && pending(0) > 0, "as the parent left it");
        assert!(tty_save());
        assert_eq!(fl(0) & libc::O_NONBLOCK, 0, "cleared at start");
        assert!(pending(0) > 0, "saving does not flush");
        let saved = modes(0).unwrap();
        // What a command that dies in raw mode leaves behind.
        let mut raw = saved;
        raw.c_lflag &= !(libc::ICANON | libc::ISIG);
        // SAFETY: `raw` is a live termios; tcsetattr only reads it.
        assert_eq!(unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) }, 0);
        set_fl(0, fl(0) | libc::O_NONBLOCK);
        flush_input();
        let now = modes(0).unwrap();
        assert_eq!((now.c_lflag, now.c_iflag, now.c_oflag), (saved.c_lflag, saved.c_iflag, saved.c_oflag));
        assert_eq!(pending(0), 0, "the typed-ahead line is dropped");
        assert_eq!(fl(0) & libc::O_NONBLOCK, 0);
        return;
    }
    let (mut m, s) = pty();
    // No echo, so nothing waits in the pty's output for a reader that never comes.
    let mut t = modes(s.as_raw_fd()).unwrap();
    t.c_lflag &= !libc::ECHO;
    // SAFETY: `t` is a live termios; tcsetattr only reads it.
    assert_eq!(unsafe { libc::tcsetattr(s.as_raw_fd(), libc::TCSANOW, &t) }, 0);
    m.write_all(b"typed ahead\n").unwrap();
    let t0 = std::time::Instant::now();
    while pending(s.as_raw_fd()) == 0 {
        assert!(t0.elapsed() < Duration::from_secs(5), "the typed line never reached the pty");
        std::thread::sleep(Duration::from_millis(5));
    }
    set_fl(s.as_raw_fd(), fl(s.as_raw_fd()) | libc::O_NONBLOCK);
    let me = std::env::current_exe().unwrap();
    let args = ["--exact", "sys::tests::flush_input_restores_a_pty", "--test-threads=1", "--nocapture"];
    // A bounded wait that allows for a loaded machine: a child that hangs fails the test instead of holding it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(std::process::Command::new(me).args(args).env(PTY_CHILD, "1").stdin(s).output()));
    let out = rx.recv_timeout(Duration::from_secs(60)).expect("the child test did not end within 60 s").unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "the child test failed: {text}");
}

/// The `!cmd` child's signal state, read from its own /proc/self/status: no signal blocked, none of the four ignored,
/// even when they were (an earlier hook ignores HUP and blocks INT and TERM, as a henki started under `nohup` could pass
/// on); and it stays in henki's session and process group.
#[cfg(target_os = "linux")]
#[test]
fn bang_setup_signal_defaults() {
    let mut cmd = std::process::Command::new("cat");
    cmd.arg("/proc/self/status");
    let before = || {
        // SAFETY: signal takes integers only; `sigset` fills a local; both are async-signal-safe.
        unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
        // SAFETY: a live, initialized set and a null pointer for the old mask.
        check(unsafe { libc::sigprocmask(libc::SIG_BLOCK, &sigset(&[libc::SIGINT, libc::SIGTERM]), std::ptr::null_mut()) })
    };
    // SAFETY: the hook allocates nothing and calls only async-signal-safe functions. Hooks run in the order they were added.
    unsafe { cmd.pre_exec(before) };
    bang_setup(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let child = String::from_utf8(out.stdout).unwrap();
    let ours = fs::read_to_string("/proc/self/status").unwrap();
    let field = |s: &str, k: &str| s.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string()).unwrap_or_else(|| panic!("no {k} in {s}"));
    assert_eq!(u64::from_str_radix(&field(&child, "SigBlk:"), 16).unwrap(), 0, "an empty mask");
    let four = SIGNALS.iter().fold(0u64, |m, &s| m | 1 << (s - 1));
    assert_eq!(u64::from_str_radix(&field(&child, "SigIgn:"), 16).unwrap() & four, 0, "INT, QUIT, TERM and HUP at their default");
    for k in ["NSsid:", "NSpgid:"] {
        assert_eq!(field(&child, k), field(&ours, k), "{k} unchanged: no new session or process group");
    }
}
