//! The OS calls henki needs beyond std, each behind a small safe function. The only file with `unsafe`.
#![allow(unsafe_code)]
use crate::types::Ev;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::{fd::AsRawFd, fd::FromRawFd, unix::ffi::OsStrExt, unix::process::CommandExt};
use std::{ffi::CStr, ffi::CString, ffi::OsStr, fs, fs::File, io, io::Read, path::Path, sync::OnceLock, sync::mpsc::Sender, time::SystemTime, time::UNIX_EPOCH};

/// The signals that end a run; blocked in every thread and taken by `sigwait`.
const SIGNALS: [i32; 4] = [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP];

fn sigset(sigs: &[i32]) -> libc::sigset_t {
    // SAFETY: an all-zero sigset_t is a valid value to start from; sigemptyset and sigaddset write only into this local.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for &s in sigs {
            libc::sigaddset(&mut set, s);
        }
        set
    }
}

fn check(rc: libc::c_int) -> io::Result<()> {
    if rc < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Blocks INT, QUIT, TERM and HUP in the calling thread (so in every thread started later) and starts the thread that
/// `sigwait`s for them and sends `Ev::Signal`. One that henki started with ignored (`nohup`, a background job of a
/// script) stays ignored: it is neither blocked nor waited for. Must be the first call in `main`, before any other
/// thread exists.
pub fn signals(tx: Sender<Ev>) -> io::Result<()> {
    let set = sigset(&SIGNALS.into_iter().filter(|&s| !ignored(s)).collect::<Vec<_>>());
    // SAFETY: `set` is an initialized sigset_t and a null pointer for the old mask is allowed.
    let rc = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(rc));
    }
    std::thread::Builder::new().name("signals".into()).spawn(move || {
        let mut sig = 0;
        // SAFETY: `set` lives in this closure and `sig` is a live out pointer; sigwait reads the one and writes the other.
        while unsafe { libc::sigwait(&set, &mut sig) } == 0 && tx.send(Ev::Signal(sig)).is_ok() {}
    })?;
    Ok(())
}

/// Whether `sig` is set to SIG_IGN; reads the disposition without changing it.
fn ignored(sig: i32) -> bool {
    // SAFETY: an all-zero sigaction is a valid value to overwrite; with a null new action, sigaction only writes the old.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN
    }
}

/// RLIMIT_CORE 0. Linux: PR_SET_DUMPABLE 0 and PR_SET_CHILD_SUBREAPER. Errors are returned, never swallowed.
pub fn harden() -> io::Result<()> {
    let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: the pointer is to a live rlimit.
    check(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) })?;
    #[cfg(target_os = "linux")]
    {
        // SAFETY: both options take integer arguments only, passed as the unsigned longs prctl reads.
        check(unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) })?;
        // SAFETY: as above.
        check(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) })?;
    }
    Ok(())
}

/// Installs the tool child's `pre_exec` hook on `cmd` (so the `unsafe` call stays in this file): `setsid()`, INT, QUIT,
/// TERM and HUP back to their default action (henki may have started with one ignored), an empty signal mask, then on
/// Linux PR_SET_PDEATHSIG(SIGKILL) and `_exit(1)` if `getppid()` is no longer `henki`. Async-signal-safe calls only;
/// the hook allocates nothing.
pub fn child_setup(cmd: &mut std::process::Command, henki: u32) {
    #[cfg(not(target_os = "linux"))]
    let _ = henki;
    let hook = move || -> io::Result<()> {
        // SAFETY: setsid takes no arguments and is async-signal-safe.
        check(unsafe { libc::setsid() })?;
        defaults()?;
        #[cfg(target_os = "linux")]
        {
            // SAFETY: integer arguments only, passed as the unsigned longs prctl reads.
            check(unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) })?;
            // SAFETY: getppid and _exit take no pointers and are async-signal-safe. A parent that died before the prctl
            // above would never send the signal, so the child leaves at once.
            unsafe {
                if libc::getppid() as u32 != henki {
                    libc::_exit(1);
                }
            }
        }
        Ok(())
    };
    // SAFETY: the hook allocates nothing and calls only async-signal-safe functions, as `pre_exec` requires.
    unsafe { cmd.pre_exec(hook) };
}

/// The `!cmd` child's pre_exec: INT, QUIT, TERM, HUP back to SIG_DFL, an empty signal mask; no new session and no
/// parent-death signal (the command runs in henki's foreground process group, on the terminal). Async-signal-safe.
pub fn bang_setup(cmd: &mut std::process::Command) {
    // SAFETY: the hook allocates nothing and calls only async-signal-safe functions, as `pre_exec` requires.
    unsafe { cmd.pre_exec(defaults) };
}

/// A child's signal state before exec: the four signals back to their default action (henki may have started with one
/// ignored), then an empty mask. Async-signal-safe calls only (`sigset` too), no allocation.
fn defaults() -> io::Result<()> {
    for s in SIGNALS {
        // SAFETY: signal takes integers only and is async-signal-safe; SIG_DFL needs no handler.
        unsafe { libc::signal(s, libc::SIG_DFL) };
    }
    // SAFETY: the empty set is an initialized sigset_t; a null pointer for the old mask is allowed.
    check(unsafe { libc::sigprocmask(libc::SIG_SETMASK, &sigset(&[]), std::ptr::null_mut()) })
}

/// isatty(3).
pub fn isatty(fd: i32) -> bool {
    // SAFETY: isatty takes an integer only.
    unsafe { libc::isatty(fd) == 1 }
}

/// (rows, cols) of the terminal on `fd` (TIOCGWINSZ); None when it is not a terminal or reports 0 columns.
pub fn winsize(fd: i32) -> Option<(u16, u16)> {
    let mut w = libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: the pointer is to a live winsize, the type TIOCGWINSZ writes.
    (unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut w) } == 0 && w.ws_col > 0).then_some((w.ws_row, w.ws_col))
}

/// fd 0's terminal modes from the first `tty_save` that found a terminal.
static TTY: OnceLock<libc::termios> = OnceLock::new();

/// tcgetattr(0): keeps fd 0's terminal modes for `flush_input`; false when fd 0 is not a terminal. The REPL calls it
/// once at start; it is also the REPL's "stdin is a terminal" check. On a terminal it also clears O_NONBLOCK.
pub fn tty_save() -> bool {
    modes(0).map(|t| TTY.get_or_init(|| t)).inspect(|_| blocking(0)).is_some()
}

/// tcgetattr(fd); None when `fd` is not a terminal.
fn modes(fd: i32) -> Option<libc::termios> {
    // SAFETY: an all-zero termios is a valid value to overwrite.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is to a live termios, which tcgetattr fills.
    (unsafe { libc::tcgetattr(fd, &mut t) } == 0).then_some(t)
}

/// tcsetattr(0, TCSAFLUSH, the saved modes): restores the modes `tty_save` kept and drops what was typed ahead, so a
/// command that left the terminal raw or without echo cannot leave the line reader with it; then clears O_NONBLOCK.
/// Nothing when no modes were saved (`run`, or fd 0 not a terminal).
pub fn flush_input() {
    // SAFETY: `t` is a termios tcgetattr filled, and tcsetattr only reads it.
    let _ = TTY.get().map(|t| unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, t) }).inspect(|_| blocking(0));
}

/// Clears O_NONBLOCK on `fd`. A command that shares the terminal may set it on the open file and leave it set; reads
/// from the terminal, henki's and the next command's, would then fail at once instead of waiting for input.
fn blocking(fd: i32) {
    // SAFETY: fcntl with F_GETFL and F_SETFL takes and returns integers only.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags >= 0 && flags & libc::O_NONBLOCK != 0 {
        // SAFETY: as above; only O_NONBLOCK changes.
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    }
}

/// (dev, ino, size, mtime_ns, ctime_ns) of one path; None when it does not exist.
pub type Stamp = Option<[i64; 5]>;

/// lstat, then stat (following symlinks): retargeting a link and changing what it points at both show.
pub fn stamp(path: &Path) -> (Stamp, Stamp) {
    let ns = |s: i64, n: i64| s.wrapping_mul(1_000_000_000).wrapping_add(n);
    let of = |m: fs::Metadata| [m.dev() as i64, m.ino() as i64, m.size() as i64, ns(m.mtime(), m.mtime_nsec()), ns(m.ctime(), m.ctime_nsec())];
    (fs::symlink_metadata(path).ok().map(of), fs::metadata(path).ok().map(of))
}

/// kill(2); a negative pid is a process group; sig 0 probes. False only on ESRCH. Never pid 0 or -1 (henki's own group,
/// every process): those give false.
pub fn kill(pid: i32, sig: i32) -> bool {
    // SAFETY: kill takes integers only.
    pid != 0 && pid != -1 && (unsafe { libc::kill(pid, sig) } == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH))
}

/// waitpid(pid, WNOHANG) for one pid; true when it was reaped. Never waitpid(-1).
pub fn reap(pid: i32) -> bool {
    let mut status = 0;
    // SAFETY: `status` is a live out pointer; pid > 0 names one child.
    pid > 0 && unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == pid
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub sid: i32,
    pub zombie: bool,
}

/// Linux: every /proc/[0-9]*/stat, fields parsed after the last ')'. macOS: empty.
pub fn procs() -> Vec<Proc> {
    #[cfg(target_os = "linux")]
    {
        let Ok(dir) = fs::read_dir("/proc") else { return Vec::new() };
        dir.filter_map(|e| {
            let e = e.ok()?;
            let pid = e.file_name().to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()?;
            let mut s = String::new();
            File::open(e.path().join("stat")).ok()?.read_to_string(&mut s).ok()?;
            stat(pid, &s)
        })
        .collect()
    }
    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

/// `PID (COMM) STATE PPID PGRP SESSION ...`; COMM may hold anything, so the fields start after the last ')'.
#[cfg(target_os = "linux")]
fn stat(pid: i32, s: &str) -> Option<Proc> {
    let mut f = s.get(s.rfind(')')? + 1..)?.split_ascii_whitespace();
    let zombie = f.next()? == "Z";
    let ppid = f.next()?.parse().ok()?;
    let sid = f.nth(1)?.parse().ok()?;
    Some(Proc { pid, ppid, sid, zombie })
}

/// The live (non-zombie) processes matching `root`, plus all their ppid-descendants; never henki itself.
pub fn tree(ps: &[Proc], root: impl Fn(&Proc) -> bool) -> Vec<i32> {
    let me = std::process::id() as i32;
    let live = || ps.iter().filter(|p| !p.zombie && p.pid != me);
    let mut out: Vec<i32> = live().filter(|p| root(p)).map(|p| p.pid).collect();
    let mut i = 0;
    while let Some(&parent) = out.get(i) {
        let kids: Vec<i32> = live().filter(|p| p.ppid == parent && !out.contains(&p.pid)).map(|p| p.pid).collect();
        out.extend(kids);
        i += 1;
    }
    out
}

/// O_RDONLY|O_NONBLOCK|O_NOCTTY|O_CLOEXEC, then fstat. Never blocks on a FIFO or a device.
pub fn open_read(path: &Path) -> io::Result<(File, fs::Metadata)> {
    let f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path)?;
    f.metadata().map(|m| (f, m))
}

/// `open_read`, refusing anything but a regular file: Err("not a regular file: PATH (KIND)"); Ok carries the length.
pub fn open_regular(path: &Path) -> Result<(File, u64), String> {
    let (f, m) = open_read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.is_file() { Ok((f, m.len())) } else { Err(not_regular(path.display(), m.file_type())) }
}

/// `not a regular file: PATH (fifo)`; the kind is one of directory, fifo, socket, char device, block device.
pub fn not_regular(path: impl std::fmt::Display, t: fs::FileType) -> String {
    let kinds = [(t.is_dir(), "directory"), (t.is_fifo(), "fifo"), (t.is_socket(), "socket"), (t.is_char_device(), "char device"), (t.is_block_device(), "block device")];
    format!("not a regular file: {path} ({})", kinds.iter().find(|k| k.0).map_or("unknown kind", |k| k.1))
}

/// O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC (| O_APPEND when `append`) with an explicit mode: the only way henki
/// creates a file by path. The journal passes `append = true`, spill files `false`.
pub fn create_excl(path: &Path, mode: u32, append: bool) -> io::Result<File> {
    fs::OpenOptions::new().write(true).append(append).create_new(true).mode(mode).custom_flags(libc::O_NOFOLLOW).open(path)
}

/// O_RDWR|O_APPEND|O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK, then fstat; a non-regular file is an error. The resumed journal:
/// read through it, `set_len` to cut a torn tail, then every write lands at the end.
pub fn open_append(path: &Path) -> io::Result<File> {
    let f = fs::OpenOptions::new().read(true).append(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    if f.metadata()?.is_file() { Ok(f) } else { Err(io::Error::other("not a regular file")) }
}

/// Creates missing ancestors 0700; refuses a symlink or a directory owned by another uid; tightens a wider mode to 0700.
/// The ownership rule is a private pure function of (owner uid, euid), so a unit test can feed it a foreign uid.
pub fn private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?,
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() => return Err(io::Error::other("a symlink, not a directory")),
        Ok(_) => {}
    }
    // O_NOFOLLOW: a symlink planted after the check above fails here.
    let d = Dir::open(path)?.0;
    let m = d.metadata()?;
    owner(m.uid(), euid())?;
    if m.mode() & 0o077 != 0 {
        d.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn owner(uid: u32, euid: u32) -> io::Result<()> {
    if uid == euid { Ok(()) } else { Err(io::Error::other(format!("owned by uid {uid}, not by uid {euid}"))) }
}

/// A directory opened O_DIRECTORY|O_NOFOLLOW; the write path's *at calls go through it.
pub struct Dir(File);

impl Dir {
    pub fn open(path: &Path) -> io::Result<Dir> {
        fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path).map(Dir)
    }

    /// (dev, ino)
    pub fn id(&self) -> io::Result<(u64, u64)> {
        self.0.metadata().map(|m| (m.dev(), m.ino()))
    }

    /// mkdirat 0o777 when `create` (EEXIST ok), then openat, no symlink follow.
    pub fn sub(&self, name: &OsStr, create: bool) -> io::Result<Dir> {
        let c = component(name)?;
        // SAFETY: the fd is open for as long as `self` lives and `c` is NUL-terminated.
        let made = !create || check(unsafe { libc::mkdirat(self.0.as_raw_fd(), c.as_ptr(), 0o777) }).is_ok();
        if !made && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
            return Err(io::Error::last_os_error());
        }
        self.openat(&c, libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW, 0).map(Dir)
    }

    /// openat O_CREAT|O_EXCL|O_NOFOLLOW|O_WRONLY|O_CLOEXEC.
    pub fn create(&self, name: &OsStr, mode: u32) -> io::Result<File> {
        self.openat(&component(name)?, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW, mode)
    }

    fn openat(&self, name: &CStr, flags: libc::c_int, mode: u32) -> io::Result<File> {
        // SAFETY: the fd is open for as long as `self` lives, `name` is NUL-terminated, and the mode goes as the
        // unsigned int openat reads from its variadic argument.
        let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC, mode as libc::c_uint) };
        check(fd)?;
        // SAFETY: `fd` is a new descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// renameat within this directory.
    pub fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        let (from, to, fd) = (component(from)?, component(to)?, self.0.as_raw_fd());
        // SAFETY: the fd is open for as long as `self` lives and both names are NUL-terminated.
        check(unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) })
    }

    /// unlinkat; errors ignored.
    pub fn unlink(&self, name: &OsStr) {
        if let Ok(c) = component(name) {
            // SAFETY: the fd is open for as long as `self` lives and `c` is NUL-terminated; the result is not needed.
            unsafe { libc::unlinkat(self.0.as_raw_fd(), c.as_ptr(), 0) };
        }
    }
}

/// One path component: not empty, `.` or `..`, and without `/` or NUL, so an *at call never walks out of its directory.
fn component(name: &OsStr) -> io::Result<CString> {
    let b = name.as_bytes();
    if b.is_empty() || b == b"." || b == b".." || b.contains(&b'/') {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    CString::new(b).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

pub fn euid() -> u32 {
    // SAFETY: geteuid cannot fail and touches no memory.
    unsafe { libc::geteuid() }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The broken-down time of `secs`, local or UTC; all zero if the C library cannot represent it.
fn tm(secs: i64, local: bool) -> libc::tm {
    let t = secs as _; // the C library's time_t, inferred from the calls below
    // SAFETY: an all-zero tm is a valid value (its zone pointer, where present, is only written).
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are to live locals; the reentrant forms write only into `tm`.
    unsafe { if local { libc::localtime_r(&t, &mut tm) } else { libc::gmtime_r(&t, &mut tm) } };
    tm
}

/// UTC now as ("2026-11-05T17:12:03Z", "20261105T171203Z"); through `gmtime_r` (as `local_date` uses `localtime_r`),
/// with a private `utc_at(secs)` the test feeds fixed epochs. No crate.
pub fn utc() -> (String, String) {
    utc_at(now())
}

fn utc_at(secs: i64) -> (String, String) {
    let t = tm(secs, false);
    let (y, mo, d, h, mi, s) = (t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec);
    (format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z"), format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z"))
}

/// The local date, "2026-11-05" (localtime_r).
pub fn local_date() -> String {
    let t = tm(now(), true);
    format!("{:04}-{:02}-{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday)
}

/// FNV-1a 64 as 16 lowercase hex digits.
pub fn fnv(b: &[u8]) -> String {
    let h = b.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &c| (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

/// 4 hex digits from /dev/urandom (the session id suffix).
pub fn random4() -> io::Result<String> {
    let mut b = [0u8; 2];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(format!("{:02x}{:02x}", b[0], b[1]))
}

#[cfg(test)]
mod tests;
