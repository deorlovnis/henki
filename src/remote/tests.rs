#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh directory holding one binding script, removed when dropped.
struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A binding in bash: it answers hello (caps fork) and bye itself (with a file `stay` in its dir: never, ignoring TERM),
/// and runs `body` for every other request with `$id`,
/// `$m` (the method) and `$line` set; `ok JSON` answers with a result.
fn binding(body: &str) -> (Tmp, Remote) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let t = Tmp(std::env::temp_dir().join(format!("henki-remote-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))));
    let _ = fs::remove_dir_all(&t.0);
    fs::create_dir_all(&t.0).unwrap();
    let script = t.0.join("binding");
    let text = format!(
        "#!/bin/bash\nok() {{ printf '{{\"id\":%s,\"result\":%s}}\\n' \"$id\" \"$1\"; }}\nwhile IFS= read -r line; do\n\
         id=$(printf '%s' \"$line\" | sed -n 's/^{{\"id\":\\([0-9]*\\),.*/\\1/p')\n\
         m=$(printf '%s' \"$line\" | sed -n 's/^{{\"id\":[0-9]*,\"method\":\"\\([^\"]*\\)\".*/\\1/p')\n\
         case \"$m\" in hello) ok '{{\"caps\":[\"fork\"],\"impl\":\"1.0\",\"name\":\"t\",\"protocol\":1,\"version\":1}}'; continue ;; bye) [ -f stay ] && trap '' TERM && sleep 30; ok null; exit 0 ;; esac\n{body}\ndone\n"
    );
    fs::write(&script, text).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let env = vec![("PATH".into(), "/usr/bin:/bin".into())];
    let mut r = Remote::start("history", &script, ("t 1".into(), "fix: X".into()), &t.0, &env, Stdio::null()).unwrap();
    r.hello(&t.0, &t.0, &[1]).unwrap();
    (t, r)
}

#[test]
fn remote_hello_and_answers() {
    let (_t, mut r) = binding(r#"case "$m" in create) ok '{"session":"s-1"}' ;; *) printf '{"error":{"code":"x-odd","message":"no \u202e"},"id":%s}\n' "$id" ;; esac"#);
    assert_eq!((r.said.as_str(), r.caps.clone()), ("t 1.0", vec!["fork".to_string()]));
    assert_eq!(r.create(Path::new("/"), None).unwrap(), "s-1");
    // An error answer is one failed call: an unknown code arrives as `other`, the message escaped and named; it lives on.
    let e = r.call("x-none", json!({})).unwrap_err();
    assert_eq!(e, err("other", "history binding t 1: x-odd: no \\u{202e}; fix: X"));
    assert_eq!(r.create(Path::new("/"), None).unwrap(), "s-1");
}

#[test]
fn remote_hello_version() {
    let (t, mut r) = binding("");
    let e = r.hello(&t.0, &t.0, &[2]).unwrap_err();
    assert_eq!(e.code, "version");
    assert!(e.message.starts_with("history binding t 1: version: hello answered {\"caps\":[\"fork\"]"), "{}", e.message);
    // The binding is dead for the session: every later call gives the same error at once.
    assert_eq!(r.call("create", json!({})).unwrap_err(), e);
}

#[test]
fn remote_skips_stale_ids_and_notifications() {
    let (_t, mut r) = binding(r#"printf '{"note":1}\n{"id":null,"x":1}\n{"id":999,"result":1}\n'; ok '{"session":"s-2"}'"#);
    assert_eq!(r.open(Path::new("/"), None).unwrap(), "s-2");
}

#[test]
fn remote_broken_lines_kill_it() {
    for (body, why) in [
        ("echo 'not json'", "protocol: a line that is not a JSON object"),
        ("printf '[1]\\n'", "protocol: a line that is not a JSON object"),
        (r#"printf '{"error":{"code":"io","message":"m"},"id":%s,"result":1}\n' "$id""#, "protocol: an answer holds one of result and error"),
        (r#"printf '{"id":%s}\n' "$id""#, "protocol: an answer holds one of result and error"),
        ("head -c 16777300 /dev/zero | tr '\\0' x", "protocol: a line over 16,777,216 bytes, or cut"),
        ("printf '{\"id\":'; exit 0", "protocol: a line over 16,777,216 bytes, or cut"),
        ("exit 0", "crashed: it closed its stdout or exited"),
        (r#"ok '{"session":"../x"}'"#, "protocol: no valid session id in the answer"),
    ] {
        let (_t, mut r) = binding(body);
        let e = r.create(Path::new("/"), None).unwrap_err();
        assert_eq!(e.message, format!("history binding t 1: {why}; fix: X"), "{body}");
        assert!(r.child.is_none() && r.dead.is_some(), "{body}: not ended");
    }
}

#[test]
fn remote_open_must_answer_the_id_asked() {
    let (_t, mut r) = binding(r#"ok '{"session":"other"}'"#);
    assert_eq!(r.open(Path::new("/"), Some("s-3")).unwrap_err().code, "protocol");
}

#[test]
fn remote_timeout_and_deadline() {
    let (_t, mut r) = binding("sleep 30");
    // A side-effect-free wait ends at the deadline within a slice, and the binding lives on (a late answer is stale).
    r.deadline = Some(Instant::now() + Duration::from_millis(100));
    let t0 = Instant::now();
    assert_eq!(r.read("s").err().unwrap().code, "budget");
    assert!(t0.elapsed() < Duration::from_millis(250), "{:?}", t0.elapsed());
    assert!(r.dead.is_none());
    // `append` changes state: it waits its whole timeout, then the binding is dead.
    let t0 = Instant::now();
    let e = r.append("s", json!({"t": "user"})).unwrap_err();
    assert_eq!(e.message, "history binding t 1: timeout: no answer to append in 2.0 s; fix: X");
    assert!(t0.elapsed() >= Duration::from_secs(2) && t0.elapsed() < Duration::from_millis(2_200), "{:?}", t0.elapsed());
    assert!(r.child.is_none());
}

#[test]
fn remote_paged_read() {
    let (_t, mut r) =
        binding(r#"case "$line" in *'"from":0'*) ok '{"next":2,"notices":["a\u202e"],"records":[{"t":"user"},{"t":"assistant"}]}' ;; *) ok '{"records":[{"t":"close"}]}' ;; esac"#);
    let t = r.read("s-4").unwrap();
    assert_eq!(t.records, vec![json!({"t": "user"}), json!({"t": "assistant"}), json!({"t": "close"})]);
    assert_eq!((t.notices, t.place.as_str()), (vec!["a\\u{202e}".to_string()], "history binding t 1: session s-4"));
    for page in [r#"ok '{"next":1,"records":[{"t":"user"},{"t":"user"}]}'"#, r#"ok '{"records":[{"x":1}]}'"#] {
        let (_t, mut r) = binding(page);
        assert_eq!(r.read("s").err().unwrap().code, "protocol", "{page}");
    }
}

#[test]
fn remote_line_over_the_cap_is_not_sent() {
    let (_t, mut r) = binding("ok null");
    let e = r.append("s", json!({"t": "user", "c": "x".repeat(LINE_MAX)})).unwrap_err();
    assert_eq!(e.code, "invalid");
    assert!(r.dead.is_none() && r.append("s", json!({"t": "user"})).is_ok());
}

#[test]
fn remote_drop_says_bye_then_kills() {
    // One answers bye, one never does (and ignores TERM): either is gone with its child after the drop, which waits at
    // most the 300 ms bye allows.
    for stay in [false, true] {
        let (t, mut r) = binding("sleep 3133.7 & echo $! >child; ok null");
        r.call("x", json!({})).unwrap();
        if stay {
            fs::write(t.0.join("stay"), "").unwrap();
        }
        let (pid, child) = (r.child.as_ref().unwrap().id() as i32, fs::read_to_string(t.0.join("child")).unwrap().trim().parse::<i32>().unwrap());
        let t0 = Instant::now();
        drop(r);
        let took = t0.elapsed();
        assert!(took < Duration::from_millis(450) && (!stay || took >= Duration::from_millis(300)), "stay {stay}: {took:?}");
        if cfg!(target_os = "linux") {
            // The binding is reaped by the drop; its child, SIGKILLed with it but not henki's to reap, may take a moment
            // to die on a loaded machine (kill(2) returns before the signal is acted on).
            let gone = |p: i32| !sys::procs().iter().any(|q| q.pid == p && !q.zombie);
            let until = Instant::now() + Duration::from_secs(2);
            while !gone(child) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(gone(pid) && gone(child), "stay {stay}: the binding or its child outlived the drop");
        }
    }
}

/// examples/port_toy, which `cargo test` builds next to this test binary's dir.
fn port_toy() -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("examples/port_toy")
}

/// The p50 and p95 of `ms`.
fn p50_p95(mut ms: Vec<f64>) -> (f64, f64) {
    ms.sort_by(f64::total_cmp);
    (ms[ms.len() / 2], ms[ms.len() * 95 / 100])
}

/// The cost of history over port/1: 1,000 appends of a typical record through `Remote` to port_toy (which syncs each
/// one, as std does) against the std binding, on the same disk. Over std: p50 under 1 ms, p95 under 5 ms.
#[test]
#[ignore = "release gate"]
fn remote_append_overhead() {
    let t = Tmp(std::env::temp_dir().join(format!("henki-remote-gate-{}", std::process::id())));
    let _ = fs::remove_dir_all(&t.0);
    fs::create_dir_all(t.0.join("toy")).unwrap();
    let rec = json!({"t": "tool_result", "id": "h1", "name": "shell", "content": "x".repeat(2_000), "is_error": false, "ms": 3, "bytes": 2_000});
    let time = |h: &mut dyn History| -> Vec<f64> {
        let sid = h.create(&t.0, None).unwrap();
        (0..1_000).map(|_| (Instant::now(), h.append(&sid, rec.clone()).unwrap()).0.elapsed().as_secs_f64() * 1e3).collect()
    };
    let env = vec![("PATH".into(), "/usr/bin:/bin".into())];
    let mut r = Remote::start("history", &port_toy(), ("toy".into(), String::new()), &t.0, &env, Stdio::null()).unwrap();
    r.hello(&t.0, &t.0.join("toy"), &[1]).unwrap();
    crate::journal::state_dir(&t.0.join("state")).unwrap();
    let (std, remote) = (p50_p95(time(&mut crate::journal::Journal::new(&t.0.join("state")))), p50_p95(time(&mut r)));
    eprintln!("append ms p50/p95: std {:.3}/{:.3}, port_toy over port/1 {:.3}/{:.3}", std.0, std.1, remote.0, remote.1);
    assert!(remote.0 - std.0 < 1.0 && remote.1 - std.1 < 5.0, "over std: p50 {:.3} ms, p95 {:.3} ms", remote.0 - std.0, remote.1 - std.1);
}
