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
    let out = child_env(&env(&pairs), &[], "/usr/bin:/bin", "20261105T171203Z-3f9a");
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
    let out = child_env(&e, &["ALL_PROXY".into()], "/bin", "s");
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
    let out = child_env(&e, &["FOO".into(), "CARGO_HOME".into(), "MISSING".into()], "/bin", "s");
    assert_eq!(names(&out), ["FOO", "CARGO_HOME", "PATH", "HENKI_USER_PATH", "HENKI_SESSION", "HENKI_VERSION", "HENKI_DEPTH"]);
    assert_eq!(get(&out, "FOO"), Some("1"));
    assert_eq!(get(&out, "CARGO_HOME"), Some("/c"));
}

#[test]
fn env_henki_vars() {
    let e = env(&[("PATH", "./bin:/inherited"), ("HENKI_SESSION", "old"), ("HENKI_VERSION", "0.0.0")]);
    let out = child_env(&e, &[], "/opt/homebrew/bin:/usr/bin:/bin", "20261105T171203Z-3f9a");
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
        get(&child_env(&e, &[], "/bin", "s"), "HENKI_DEPTH").unwrap().to_string()
    };
    assert_eq!(depth(None), "1");
    assert_eq!(depth(Some("0")), "1");
    assert_eq!(depth(Some("1")), "2");
    assert_eq!(depth(Some("41")), "42");
    assert_eq!(depth(Some("x")), "1");
}

#[test]
fn env_bash_env_never_passed() {
    let t = Tmp::new();
    let rc = t.0.join("rc");
    fs::write(&rc, "echo sourced\n").unwrap();
    let rc = rc.to_str().unwrap();
    let e = env(&[("BASH_ENV", rc), ("ENV", rc), ("HOME", t.0.to_str().unwrap())]);
    let vars = child_env(&e, &["BASH_ENV".into(), "ENV".into()], "/usr/bin:/bin", "s");
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
