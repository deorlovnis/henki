#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

const BASH: &str = "/bin/bash";

/// A fresh, empty directory under the temp dir, removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-supervise-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn spill(&self, name: &str) -> Spill {
        Spill { path: self.0.join(name), shown: format!("~/spill/{name}") }
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn env(pairs: &[(&str, &str)]) -> Env {
    Env(pairs.iter().map(|(k, v)| (k.into(), v.into())).collect())
}

fn get<'a>(vars: &'a [(OsString, OsString)], name: &str) -> Option<&'a str> {
    vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.to_str().unwrap())
}

fn names(vars: &[(OsString, OsString)]) -> Vec<&str> {
    vars.iter().map(|(k, _)| k.to_str().unwrap()).collect()
}

fn path_env() -> Vec<(OsString, OsString)> {
    vec![("PATH".into(), "/usr/bin:/bin".into())]
}

/// Starts `cmd` with `tag` in `dir`.
fn start(dir: &Path, cmd: &str, vars: &[(OsString, OsString)], cap: usize, spill: Spill, tag: u32, tx: &Sender<Ev>) -> i32 {
    spawn(Path::new(BASH), cmd, dir, vars, cap, spill, tag, tx).unwrap()
}

/// The one `Ev::Tool` of `tag`, within `within`.
fn tool(rx: &mpsc::Receiver<Ev>, tag: u32, within: Duration) -> ToolOutput {
    match rx.recv_timeout(within).expect("no Ev::Tool in time") {
        Ev::Tool { tag: t, out } if t == tag => out,
        ev => panic!("unexpected {ev:?}"),
    }
}

/// Runs `cmd` to its `Ev::Tool`.
fn run(t: &Tmp, cmd: &str, cap: usize, spill: Spill) -> ToolOutput {
    let (tx, rx) = mpsc::channel();
    start(&t.0, cmd, &path_env(), cap, spill, 7, &tx);
    drop(tx); // a waiter that dies without reporting fails the test at once
    tool(&rx, 7, Duration::from_secs(30))
}

/// A bounded poll on content, never a blind sleep.
fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux tests use it
fn alive(pid: i32) -> bool {
    sys::procs().iter().any(|p| p.pid == pid && !p.zombie)
}

#[test]
fn env_allowlist_exact() {
    let allowed: Vec<&str> = "HOME USER LOGNAME SHELL TERM LANG LC_ALL LC_CTYPE LC_MESSAGES TZ TMPDIR NO_COLOR SSH_AUTH_SOCK SSL_CERT_FILE SSL_CERT_DIR \
        XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR http_proxy HTTP_PROXY https_proxy HTTPS_PROXY no_proxy NO_PROXY"
        .split_whitespace()
        .collect();
    let mut pairs: Vec<(&str, &str)> = vec![("PATH", "/inherited"), ("FOO", "1"), ("OPENAI_API_KEY", "sk-secret"), ("LD_PRELOAD", "/x.so"), ("LC_NUMERIC", "C")];
    pairs.extend(allowed.iter().map(|&k| (k, "v")));
    pairs.extend([("HENKI_STATE_DIR", "/s"), ("BASH_ENV", "/b"), ("ENV", "/e"), ("SHELLOPTS", "xtrace"), ("home", "lower")]);
    let out = child_env(&env(&pairs), &[], "/usr/bin:/bin", "/usr/bin:/bin", "20261105T171203Z-3f9a");
    let mut want = allowed.clone();
    want.extend(["PATH", "HENKI_USER_PATH", "HENKI_SESSION", "HENKI_VERSION", "HENKI_DEPTH"]);
    assert_eq!(names(&out), want);
    assert!(out.iter().all(|(_, v)| v != "sk-secret"));
    assert!(allowed.iter().all(|k| get(&out, k) == Some("v")));
}

#[test]
fn env_proxy_userinfo_dropped() {
    let e = env(&[
        ("http_proxy", "http://user:pass@proxy:3128"),
        ("HTTP_PROXY", "user@proxy:3128"),
        ("https_proxy", "http://proxy:3128"),
        ("HTTPS_PROXY", "http://127.0.0.1:3128/"),
        ("no_proxy", "localhost,127.0.0.1"),
        ("ALL_PROXY", "socks5://a:b@proxy:1080"),
    ]);
    let out = child_env(&e, &["ALL_PROXY".into()], "/bin", "/bin", "s");
    assert_eq!(get(&out, "http_proxy"), None);
    assert_eq!(get(&out, "HTTP_PROXY"), None);
    assert_eq!(get(&out, "ALL_PROXY"), None);
    assert_eq!(get(&out, "https_proxy"), Some("http://proxy:3128"));
    assert_eq!(get(&out, "HTTPS_PROXY"), Some("http://127.0.0.1:3128/"));
    assert_eq!(get(&out, "no_proxy"), Some("localhost,127.0.0.1"));
}

#[test]
fn env_pass_env_added() {
    let e = env(&[("FOO", "1"), ("BAR", "2"), ("CARGO_HOME", "/c")]);
    let out = child_env(&e, &["FOO".into(), "CARGO_HOME".into(), "MISSING".into()], "/bin", "/bin", "s");
    assert_eq!(names(&out), ["FOO", "CARGO_HOME", "PATH", "HENKI_USER_PATH", "HENKI_SESSION", "HENKI_VERSION", "HENKI_DEPTH"]);
    assert_eq!(get(&out, "FOO"), Some("1"));
    assert_eq!(get(&out, "CARGO_HOME"), Some("/c"));
}

#[test]
fn env_henki_vars() {
    let e = env(&[("PATH", "./bin:/inherited"), ("HENKI_SESSION", "old"), ("HENKI_VERSION", "0.0.0")]);
    let out = child_env(&e, &[], "/opt/homebrew/bin:/usr/bin:/bin", "/opt/homebrew/bin:/usr/bin:/bin", "20261105T171203Z-3f9a");
    assert_eq!(get(&out, "PATH"), Some("/opt/homebrew/bin:/usr/bin:/bin"));
    assert_eq!(get(&out, "HENKI_USER_PATH"), Some("/opt/homebrew/bin:/usr/bin:/bin"));
    assert_eq!(get(&out, "HENKI_SESSION"), Some("20261105T171203Z-3f9a"));
    assert_eq!(get(&out, "HENKI_VERSION"), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(out.len(), 5, "{out:?}");
}

#[test]
fn env_depth_plus_one() {
    let depth = |v: Option<&str>| {
        let e = v.map_or_else(Env::default, |v| env(&[("HENKI_DEPTH", v)]));
        get(&child_env(&e, &[], "/bin", "/bin", "s"), "HENKI_DEPTH").unwrap().to_string()
    };
    assert_eq!(depth(None), "1");
    assert_eq!(depth(Some("0")), "1");
    assert_eq!(depth(Some("1")), "2");
    assert_eq!(depth(Some("41")), "42");
    assert_eq!(depth(Some("x")), "1");
}

#[test]
fn env_path_param() {
    let e = env(&[("PATH", "/inherited:/usr/bin"), ("HENKI_USER_PATH", "/old")]);
    let out = child_env(&e, &[], "/home/u/.config/henki/ext/notes/bin:/usr/bin:/bin", "/usr/bin:/bin", "s");
    assert_eq!(get(&out, "PATH"), Some("/home/u/.config/henki/ext/notes/bin:/usr/bin:/bin"));
    assert_eq!(get(&out, "HENKI_USER_PATH"), Some("/usr/bin:/bin"));
    assert_eq!(names(&out), ["PATH", "HENKI_USER_PATH", "HENKI_SESSION", "HENKI_VERSION", "HENKI_DEPTH"]);
}

#[test]
fn bang_env_inherits_and_depth() {
    // Everything henki inherited passes, in order, the variables a tool child never gets included.
    let pairs = [("PATH", "./bin:/usr/bin"), ("OPENAI_API_KEY", "sk-1"), ("HENKI_DEPTH", "2"), ("BASH_ENV", "/b"), ("HENKI_SESSION", "outer"), ("http_proxy", "http://u:p@x:1")];
    let out = bang_env(&env(&pairs));
    let want: Vec<(OsString, OsString)> =
        pairs.iter().filter(|p| p.0 != "HENKI_DEPTH").map(|(k, v)| ((*k).into(), (*v).into())).chain([("HENKI_DEPTH".into(), "3".into())]).collect();
    assert_eq!(out, want);
    // Unset or not a number: depth 1.
    assert_eq!(bang_env(&Env::default()), [("HENKI_DEPTH".into(), "1".into())]);
    assert_eq!(get(&bang_env(&env(&[("HENKI_DEPTH", "x"), ("HOME", "/h")])), "HENKI_DEPTH"), Some("1"));
}

#[test]
fn env_bash_env_never_passed() {
    let t = Tmp::new();
    let rc = t.0.join("rc");
    fs::write(&rc, "echo sourced\n").unwrap();
    let rc = rc.to_str().unwrap();
    let e = env(&[("BASH_ENV", rc), ("ENV", rc), ("HOME", t.0.to_str().unwrap())]);
    let vars = child_env(&e, &["BASH_ENV".into(), "ENV".into()], "/usr/bin:/bin", "/usr/bin:/bin", "s");
    assert_eq!(get(&vars, "BASH_ENV"), None);
    assert_eq!(get(&vars, "ENV"), None);
    let (tx, rx) = mpsc::channel();
    start(&t.0, "echo ran", &vars, 1024, t.spill("s-1.log"), 1, &tx);
    assert_eq!(tool(&rx, 1, Duration::from_secs(10)).text, "ran\n");
}

#[test]
fn cap_spill_math() {
    let t = Tmp::new();
    let cap = |n: usize| Cap::new(n, t.spill(&format!("c-{n}.log")));
    // Even and odd caps: head cap/2, tail the rest, whatever the chunk sizes.
    for (n, head, tail) in [(10, "abcde", "uvwxy"), (11, "abcde", "tuvwxy")] {
        let mut c = cap(n);
        for part in ["abc", "defghij", "klmnopqrstuvwxy"] {
            c.push(part.as_bytes());
        }
        assert_eq!((c.head.as_slice(), c.tail.make_contiguous() as &[u8], c.total), (head.as_bytes(), tail.as_bytes(), 25));
    }
    // Within the cap nothing is cut, even when head and tail split a code point.
    let mut c = cap(4);
    c.push(b"a\xC3");
    c.push(b"\xA9b");
    assert_eq!(c.text(), "a\u{e9}b");
    assert!(!t.0.join("c-4.log").exists());
    // A detached capture keeps nothing more.
    c.push(b"more");
    assert_eq!(c.total, 4);
    // Cut points: the head drops a code point cut short at its end, the tail the bytes that continue one.
    for (b, end) in [(&b"ab\xC3"[..], 2), (b"ab\xC3\xA9", 4), (b"a\xF0\x9F\x98", 1), (b"\xF0\x9F\x98\x80", 4), (b"\xE2\x82", 0), (b"abc", 3), (b"", 0)] {
        assert_eq!(head_end(b), end, "{b:?}");
    }
    for (b, start) in [(&b"\xA9cd"[..], 1), (b"\x9F\x98\x80z", 3), (b"\x80\x80\x80\x80", 3), (b"abc", 0), (b"\xC3\xA9", 0), (b"", 0)] {
        assert_eq!(tail_start(b), start, "{b:?}");
    }
}

#[test]
fn spill_written_past_cap() {
    let t = Tmp::new();
    // 1 + 8,192 x 2 + 1 bytes: both cuts of a 1,024-byte cap fall inside a two-byte code point.
    let cmd = "s=$'\\xc3\\xa9'; for i in {1..13}; do s=$s$s; done; printf a; printf %s \"$s\"; printf z";
    let out = run(&t, cmd, 1024, t.spill("s-1.log"));
    let e = "\u{e9}";
    assert_eq!(out.bytes, 16_386);
    assert_eq!(out.end, End::Exit(0));
    assert_eq!(out.text, format!("a{}\n[... 15,364 of 16,386 bytes cut; full output: ~/spill/s-1.log ...]\n{}z", e.repeat(255), e.repeat(255)));
    assert_eq!(fs::read(t.0.join("s-1.log")).unwrap(), format!("a{}z", e.repeat(8192)).into_bytes());
}

#[test]
fn spill_cap_8mib_marker() {
    let t = Tmp::new();
    let out = run(&t, "head -c 9000000 /dev/zero", 1024, t.spill("s-2.log"));
    assert_eq!(out.bytes, 9_000_000);
    assert!(out.text.contains("\n[... 8,998,976 of 9,000,000 bytes cut; full output: ~/spill/s-2.log ...]\n"), "{}", &out.text[512..]);
    let spill = fs::read(t.0.join("s-2.log")).unwrap();
    assert!(spill.len() as u64 <= SPILL_MAX, "{}", spill.len());
    let kept = (SPILL_MAX - ROOM) as usize;
    assert!(spill[..kept].iter().all(|&b| b == 0));
    assert_eq!(&spill[kept..], b"\n[henki: 611,456 more bytes dropped]\n");
}

#[test]
fn spill_eexist_not_kept() {
    let t = Tmp::new();
    fs::write(t.0.join("s-3.log"), "planted").unwrap();
    let out = run(&t, "head -c 3000 /dev/zero | tr '\\0' x", 1024, t.spill("s-3.log"));
    assert_eq!(out.text, format!("{}\n[... 1,976 of 3,000 bytes cut; full output not kept (name in use) ...]\n{}", "x".repeat(512), "x".repeat(512)));
    assert_eq!(fs::read_to_string(t.0.join("s-3.log")).unwrap(), "planted");
}

#[test]
fn capture_merged_order() {
    let t = Tmp::new();
    let out = run(&t, "echo one; echo two >&2; echo three; echo four >&2; exit 3", 1024, t.spill("s-4.log"));
    assert_eq!((out.text.as_str(), out.bytes, out.end), ("one\ntwo\nthree\nfour\n", 19, End::Exit(3)));
    assert_eq!(run(&t, "kill -9 $$", 1024, t.spill("s-5.log")).end, End::Signal(9));
    assert_eq!(run(&t, "true", 1024, t.spill("s-6.log")).text, "");
}

#[test]
fn capture_pipe_over_64k() {
    let t = Tmp::new();
    let out = run(&t, "head -c 300000 /dev/zero | tr '\\0' x; echo done >&2", 400_000, t.spill("s-7.log"));
    assert_eq!(out.bytes, 300_005);
    assert_eq!(out.text, format!("{}done\n", "x".repeat(300_000)));
    assert!(!t.0.join("s-7.log").exists());
}

#[test]
fn capture_background_holder_returns() {
    let t = Tmp::new();
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    let pgid = start(&t.0, "sleep 30 & echo started", &path_env(), 1024, t.spill("s-8.log"), 3, &tx);
    let out = tool(&rx, 3, Duration::from_secs(1));
    assert!(t0.elapsed() < Duration::from_secs(1) && t0.elapsed() >= GRACE, "{:?}", t0.elapsed());
    assert_eq!((out.text.as_str(), out.end), ("started\n", End::Exit(0)));
    kill_call(pgid);
    until("the background sleep to die", || !sys::procs().iter().any(|p| p.sid == pgid && !p.zombie));
}

#[cfg(target_os = "linux")]
#[test]
fn kill_call_session_members() {
    let t = Tmp::new();
    let (tx, rx) = mpsc::channel();
    // Each call starts a child in a session of its own, records its pid and waits for it.
    let call = |name: &str, tag: u32| {
        let file = t.0.join(name);
        let pgid = start(&t.0, &format!("setsid sleep 3133 & echo $! > {name}; wait"), &path_env(), 1024, t.spill(&format!("{name}.log")), tag, &tx);
        let pid = || fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<i32>().ok());
        until("the setsid child", || pid().is_some_and(|p| sys::procs().iter().any(|q| q.pid == p && q.sid == p && q.ppid == pgid)));
        (pgid, pid().unwrap())
    };
    let (a, a_child) = call("a", 1);
    let (b, b_child) = call("b", 2);
    kill_call(a);
    assert_eq!(tool(&rx, 1, Duration::from_secs(5)).end, End::Signal(libc::SIGKILL));
    until("the first call's setsid child to die", || !alive(a_child));
    assert!(alive(b) && alive(b_child), "kill_call touched another call");
    kill_call(b);
    assert_eq!(tool(&rx, 2, Duration::from_secs(5)).end, End::Signal(libc::SIGKILL));
    until("the second call's setsid child to die", || !alive(b_child));
}

#[test]
fn spill_symlink_not_followed() {
    let t = Tmp::new();
    fs::write(t.0.join("target"), "kept").unwrap();
    std::os::unix::fs::symlink(t.0.join("target"), t.0.join("s-9.log")).unwrap();
    std::os::unix::fs::symlink(t.0.join("absent"), t.0.join("s-10.log")).unwrap();
    for name in ["s-9.log", "s-10.log"] {
        let mut c = Cap::new(4, t.spill(name));
        c.push(b"abcdef");
        assert!(matches!(c.sink, Sink::Lost("name in use")), "{name}");
    }
    assert_eq!(fs::read_to_string(t.0.join("target")).unwrap(), "kept");
    assert!(!t.0.join("absent").exists());
}

#[test]
fn spill_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let t = Tmp::new();
    let mut c = Cap::new(4, t.spill("s-11.log"));
    c.push(b"abcdef");
    let m = fs::symlink_metadata(t.0.join("s-11.log")).unwrap();
    assert!(m.is_file());
    assert_eq!(m.permissions().mode() & 0o7777, 0o600);
    assert_eq!(fs::read(t.0.join("s-11.log")).unwrap(), b"abcdef");
}

#[test]
fn spill_create_error_marker() {
    let t = Tmp::new();
    let mut c = Cap::new(10, Spill { path: t.0.join("missing/s-12.log"), shown: "~/missing/s-12.log".into() });
    c.push(b"abcdefghijk");
    assert_eq!(c.text(), "abcde\n[... 1 of 11 bytes cut; full output not kept (could not write) ...]\nghijk");
}

#[cfg(target_os = "linux")]
#[test]
fn spill_write_error_not_kept() {
    let t = Tmp::new();
    let path = t.0.join("s-13.log");
    let mut c = Cap::new(4, t.spill("s-13.log"));
    c.push(b"abcdef");
    assert_eq!(fs::read(&path).unwrap(), b"abcdef");
    // From here every write fails, as on a full disk.
    c.sink = Sink::Open(fs::OpenOptions::new().write(true).open("/dev/full").unwrap(), 6);
    c.push(b"gh");
    assert!(matches!(c.sink, Sink::Lost("could not write")));
    assert!(!path.exists(), "a partial spill file stayed");
    c.push(b"ij");
    assert!(!path.exists());
    assert_eq!(c.text(), "ab\n[... 6 of 10 bytes cut; full output not kept (could not write) ...]\nij");
}

#[test]
fn capture_invalid_utf8_lossy() {
    let t = Tmp::new();
    // Within the cap the whole output is decoded at once: each invalid sequence becomes one U+FFFD.
    let out = run(&t, "printf 'a\\377b\\303(\\342\\202'", 1024, t.spill("s-14.log"));
    assert_eq!((out.text.as_str(), out.bytes), ("a\u{FFFD}b\u{FFFD}(\u{FFFD}", 7));
    // Past the cap, invalid bytes inside head and tail are replaced; a stray lead byte at the head's end and a stray
    // continuation byte at the tail's start are cut and counted.
    let mut c = Cap::new(8, t.spill("s-15.log"));
    c.push(b"\xffabc12xyz\xfe");
    assert_eq!(c.text(), "\u{FFFD}abc\n[... 2 of 10 bytes cut; full output: ~/spill/s-15.log ...]\nxyz\u{FFFD}");
    let mut c = Cap::new(8, t.spill("s-16.log"));
    c.push(b"abc\xff12\x80xyz");
    assert_eq!(c.text(), "abc\n[... 4 of 10 bytes cut; full output: ~/spill/s-16.log ...]\nxyz");
}

#[test]
fn cap_boundary_exact() {
    let t = Tmp::new();
    let mut c = Cap::new(10, t.spill("s-17.log"));
    c.push(b"abcde");
    c.push(b"fghij");
    assert_eq!((c.text(), c.total), ("abcdefghij".to_string(), 10));
    assert!(!t.0.join("s-17.log").exists());
    let mut c = Cap::new(10, t.spill("s-18.log"));
    c.push(b"abcdefghij");
    assert!(!t.0.join("s-18.log").exists());
    c.push(b"k");
    assert_eq!(fs::read(t.0.join("s-18.log")).unwrap(), b"abcdefghijk");
    assert_eq!(c.text(), "abcde\n[... 1 of 11 bytes cut; full output: ~/spill/s-18.log ...]\nghijk");
}

#[test]
fn spawn_failure_leaves_nothing() {
    let t = Tmp::new();
    let (tx, rx) = mpsc::channel();
    let fail = |bash: &str, cmd: &str, cwd: &Path| spawn(Path::new(bash), cmd, cwd, &path_env(), 1024, t.spill("s-19.log"), 9, &tx).unwrap_err().kind();
    assert_eq!(fail("/nonexistent/bash", "echo x", &t.0), io::ErrorKind::NotFound);
    assert_eq!(fail(BASH, "echo x", &t.0.join("gone")), io::ErrorKind::NotFound);
    assert_eq!(fail(BASH, "echo \0x", &t.0), io::ErrorKind::InvalidInput);
    drop(tx);
    // Each failed spawn's waiter held a sender: the channel closes only when all of them ended without a report.
    match rx.recv_timeout(Duration::from_secs(10)) {
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        got => panic!("a failed spawn left a waiter behind: {got:?}"),
    }
    assert!(!t.0.join("s-19.log").exists());
}

#[test]
fn capture_stdin_null() {
    let t = Tmp::new();
    let out = run(&t, "cat; read x; echo rc=$?; { : </dev/tty; } 2>/dev/null || echo no tty", 1024, t.spill("s-20.log"));
    assert_eq!((out.text.as_str(), out.end), ("rc=1\nno tty\n", End::Exit(0)));
    assert!(out.ms < 5_000, "{}", out.ms);
}

/// In the test process: runs the test `name` again in a new process of this test binary, alone, and returns false. In
/// that process (marked by `HENKI_TEST_ALONE`) it returns true and the test runs its body there. Code that changes the
/// whole process (the subreaper flag, the ledger, the sweep) is tested only this way, so it never meets another test's
/// children.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux tests use it
fn alone(name: &str) -> bool {
    if std::env::var_os("HENKI_TEST_ALONE").is_some() {
        return true;
    }
    let args = ["--exact", &format!("supervise::tests::{name}"), "--test-threads=1", "--nocapture"];
    let out = std::process::Command::new(std::env::current_exe().unwrap()).args(args).env("HENKI_TEST_ALONE", "1").output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains(" 1 passed;"), "{text}");
    false
}

#[cfg(target_os = "linux")]
#[test]
fn signal_groups_ended_background_gone() {
    if !alone("signal_groups_ended_background_gone") {
        return;
    }
    // As in henki: a subreaper, so a background process that ends after its shell is left as henki's zombie.
    sys::harden().unwrap();
    let t = Tmp::new();
    let (tx, rx) = mpsc::channel();
    let pgid = start(&t.0, "sleep 0.2 >/dev/null 2>&1 & echo started", &path_env(), 1024, t.spill("s-21.log"), 1, &tx);
    assert_eq!(tool(&rx, 1, Duration::from_secs(5)).text, "started\n");
    let me = std::process::id() as i32;
    until("the background sleep to end", || sys::procs().iter().any(|p| p.sid == pgid && p.ppid == me && p.zombie));
    assert_eq!(signal_groups(0), 0, "a group with nothing live still counts");
    assert!(!sys::procs().iter().any(|p| p.sid == pgid), "its zombie was not reaped");
    assert_eq!(reap(), 0);
}

/// A kernel child (a port binding) with a background child of its own: neither is in the count `reap` gives nor touched
/// by a sweep, its zombie is left to its owner, and `kernel_end` kills its whole session and reaps it.
#[cfg(target_os = "linux")]
#[test]
fn kernel_child_outside_the_floor_of_calls() {
    if !alone("kernel_child_outside_the_floor_of_calls") {
        return;
    }
    sys::harden().unwrap();
    let t = Tmp::new();
    let script = t.0.join("binding");
    fs::write(&script, "#!/bin/bash\nsleep 3133.5 &\necho $! >bg\nread -r line\n").unwrap();
    fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let io = [Stdio::piped(), Stdio::null(), Stdio::null()];
    let c = kernel_child(&script, &t.0, &path_env(), io).unwrap();
    let pid = c.id() as i32;
    let bg = || fs::read_to_string(t.0.join("bg")).ok().and_then(|s| s.trim().parse::<i32>().ok());
    until("the binding's background sleep", || bg().is_some_and(alive));
    assert_eq!((reap(), sweep_pass(), signal_groups(0)), (0, 0, 0), "a kernel child counted or swept as a call's");
    assert!(alive(pid) && alive(bg().unwrap()) && lock(&KERNEL).contains(&pid));
    // Ended on its own (stdin closed), it stays a zombie for its owner: `pass` never reaps it. Its orphan is henki's
    // child now, so it counts as background, as an orphan of a call's would.
    let mut c = c;
    drop(c.stdin.take());
    until("the binding to end", || sys::procs().iter().any(|p| p.pid == pid && p.zombie));
    assert_eq!(reap(), 1);
    assert!(sys::procs().iter().any(|p| p.pid == pid && p.zombie), "pass reaped a kernel child");
    // The kill still reaches the orphan: it is in the binding's session.
    kernel_end(Some(c));
    assert!(!lock(&KERNEL).contains(&pid) && !sys::procs().iter().any(|p| p.pid == pid));
    until("the binding's background sleep to die", || !alive(bg().unwrap()));
    assert_eq!(reap(), 0);
    kernel_end(None);
}
