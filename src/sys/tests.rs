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
