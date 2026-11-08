#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Policy over port/1: an outside binding (examples/port_toy in its policy modes, and the bash binding of the
//! conformance suite) decides who may run a call, and henki's floor holds whatever it says: protected writes refused
//! before it is asked, its allows of henki's own files asked, every failure asked, `approve = auto` refused. Every
//! binding carries the run's marker in its command line, so `Done::finish` also proves that none outlives its session.
use crate::harness::*;
use crate::mock::*;
use crate::pty::Pty;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const W: Duration = Duration::from_secs(10);
const FIX: &str = "fix: henki --port policy=std for this session, or henki mods swap policy std; every call is asked";
const FLOOR: &str = "asked: the call touches henki's own files";

/// examples/port_toy, built by `cargo test` next to the henki under test.
fn port_toy() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_henki")).parent().unwrap().join("examples/port_toy")
}

/// `CONFIG/port/policy` linked to the wrapper `RUN/bindings/policy` (in 0700 dirs) running `cmd`; the run asks.
fn bind(r: &mut Run, cmd: &str) -> PathBuf {
    r.set("approve", "\"ask\"");
    let w = r.dir.join("bindings/policy");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&w, format!("#!/bin/sh\n{cmd}\n")).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let _ = fs::remove_file(port.join("policy"));
    symlink(&w, port.join("policy")).unwrap();
    w
}

/// port_toy as the policy binding in `mode`, with `faults`; it says the run's marker as its name.
fn toy(r: &mut Run, mode: &str, faults: &str) -> PathBuf {
    let cmd = format!("exec {} policy {mode} --name {} {faults}", port_toy().display(), r.marker);
    bind(r, &cmd)
}

/// The `decide` params the toy got, one per line.
fn decided(r: &Run) -> Vec<Value> {
    lines(&fs::read_to_string(r.state().join("port/policy/decide.log")).unwrap_or_default())
}

fn verdicts(d: &Done) -> Vec<Value> {
    d.of("approval").iter().map(|a| json!([a["verdict"], a.get("by"), a.get("reason")])).collect()
}

/// An allow runs the call with the binding's line, journaled as `policy` with `by`; the binding is a notice and in
/// `session.ports`; a deny is the call's result, journaled with its reason, and three of them end the turn stuck.
#[test]
fn e2e_policy_allow_and_deny() {
    let m = Mock::start(vec![shell("echo allowed"), say("done")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "allow", "").display().to_string();
    let d = r.json("run it");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let notice = format!("policy: {w} (port/1, {} 0.1.0); henki --port policy=std runs a session without it", r.marker);
    d.check(d.notices().contains(&notice), "no binding notice");
    d.check(d.of("session")[0]["ports"] == json!({"policy": w}), "no session.ports");
    assert_eq!(verdicts(&d), [json!(["policy", w, null])]);
    d.check(d.results()[0]["content"].as_str().unwrap().starts_with("allowed\n"), "not run");
    let view = &decided(&r)[0];
    assert_eq!((&view["call"]["name"], &view["target"], &view["session"]), (&json!("shell"), &Value::Null, &json!(d.id())));
    assert_eq!((view["root"].as_str().unwrap(), view["cwd"].as_str().unwrap()), (r.root().as_str(), r.root().as_str()));
    // A deny every time: the same failing call three times is stuck.
    let m = Mock::start(vec![shell("echo no"), shell("echo no"), shell("echo no"), say("never")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "deny", "").display().to_string();
    let d = r.json("try");
    d.finish();
    let why = format!("denied by policy {w}: port_toy denies every call");
    assert_eq!((d.status(), d.result()["reason"].as_str()), ("gave_up".into(), Some("stuck")), "{}", d.diag());
    assert_eq!(verdicts(&d), vec![json!(["deny", null, why]); 3]);
    d.check(d.results().iter().all(|x| x["content"] == why.as_str() && x["is_error"] == true), "the deny is not the result");
}

/// `approve = auto` is the std policy's mode: with a policy binding the start is refused, from the config or the flag;
/// `--port policy=std` runs the session on std; no allow rule is read or shown while a binding decides.
#[test]
fn e2e_policy_auto_refused() {
    let m = Mock::start(vec![say("on std")]);
    let mut r = Run::new(&m.url());
    toy(&mut r, "allow", "");
    r.write_allow(&format!("{}\techo ruled\n", r.root()));
    let d = r.cmd(&["--json", "--approve", "auto", "hi"]).go();
    d.finish();
    let says = "--approve: approve = auto with a policy binding; fix: set approve = \"ask\", or start with --port policy=std";
    assert_eq!((d.code, d.result()["error"]["message"].as_str()), (Some(2), Some(says)), "{}", d.diag());
    r.set("approve", "\"auto\"");
    let d = r.json("hi");
    d.finish();
    d.check(
        d.result()["error"]["message"]
            .as_str()
            .unwrap()
            .ends_with("config.toml:3: approve = auto with a policy binding; fix: set approve = \"ask\", or start with --port policy=std"),
        "config auto",
    );
    assert!(m.log().is_empty() && decided(&r).is_empty(), "a request or a decide");
    r.set("approve", "\"ask\"");
    let d = r.cmd(&["--json", "--port", "policy=std", "hi"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    d.check(d.notices().iter().any(|n| n.starts_with("allow: 1 rules apply")), "std reads the rules");
    // Bound and asking: the rules are not read (no notice, no hash), the binding decides.
    let m = Mock::start(vec![shell("echo ruled"), say("done")]);
    r.at(&m.url());
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    d.check(!d.notices().iter().any(|n| n.starts_with("allow:")) && d.of("session")[0]["allow_hash"].is_null(), "rules read under a binding");
    assert_eq!(d.of("approval")[0]["verdict"], "policy");
}

/// The floor under an allow-everything binding: writes to henki's own files are refused before it is asked (its log
/// shows none); a read of one, or a shell command naming one, is asked (`run`: needs_approval, exit 3) with the note;
/// a project file and this session's spill file are not.
#[test]
fn e2e_policy_floor() {
    let mut r = Run::new("");
    let (cfg, home) = (r.config_dir(), r.home.display().to_string());
    let state = r.state();
    let protected = [
        cfg.join("config.toml"),
        cfg.join("AGENTS.md"),
        cfg.join("allow"),
        cfg.join("ext/x/doc.md"),
        state.join("x.txt"),
        r.dir.join("xdg/data/henki/x.txt"),
        cfg.join("port/policy"),
    ];
    let writes: Vec<Step> = protected.iter().map(|p| call("write", json!({"path": p, "content": "x"}))).chain([say("refused")]).collect();
    let m = Mock::start(writes);
    r.at(&m.url());
    toy(&mut r, "allow", "");
    let d = r.cmd(&["--json", "--max-steps", "20", "write them"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(d.results().len(), 7);
    d.check(d.results().iter().all(|x| x["content"].as_str().unwrap().starts_with("protected: ")), "a protected write ran");
    assert!(decided(&r).is_empty(), "the binding was asked about a protected write: {:?}", decided(&r));
    for (step, n) in [
        (call("read", json!({"path": "~/.config/henki/config.toml"})), 1),
        (shell("cat ~/.config/henki/config.toml"), 2),
        (shell(&format!("cat {home}/.config/henki/config.toml")), 3),
        (call("read", json!({"path": state.join("port/policy/decide.log")})), 4),
    ] {
        let m = Mock::start(vec![step, say("never")]);
        r.at(&m.url());
        let d = r.json("read it");
        d.finish();
        assert_eq!((d.code, d.notices().iter().filter(|x| *x == FLOOR).count()), (Some(3), 1), "{n}: {}", d.diag());
        assert_eq!(decided(&r).len(), n, "{n}: the binding is asked first");
    }
    // Not henki's own: a project file; and the spill file henki itself points the model at.
    fs::write(r.path("mine.txt"), "mine").unwrap();
    let m = Mock::start(vec![shell("head -c 100000 /dev/zero | tr '\\0' z"), say("big")]);
    r.at(&m.url());
    let d = r.cmd(&["--json", "--port", "policy=std", "--approve", "auto", "big"]).go();
    d.finish();
    let spill = spill_of(d.results()[0]["content"].as_str().unwrap(), &r.home);
    let m = Mock::start(vec![call("read", json!({"path": "mine.txt"})), call("read", json!({"path": spill})), say("read")]);
    r.at(&m.url());
    let d = r.cmd(&["--json", "--resume", "read them"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(d.of("approval").iter().map(|a| a["verdict"].clone()).collect::<Vec<_>>(), ["policy", "policy"]);
}

/// A binding that hangs, dies, breaks its line or answers what no contract has: the call is asked (`run`:
/// needs_approval, exit 3) with one notice naming it and the repair; it is killed, and nothing of it is left running.
#[test]
fn e2e_policy_failures_ask() {
    let cases = [
        ("allow", "--hang-at decide", "timeout: no answer to decide in 5.0 s", 5_600),
        ("allow", "--exit-at decide", "crashed: it closed its stdout or exited", 1_500),
        ("allow", "--garbage-at decide", "protocol: a line that is not a JSON object", 1_500),
        ("maybe", "", "protocol: decide: the decision is not allow, ask or deny", 1_500),
    ];
    for (mode, faults, why, ms) in cases {
        let m = Mock::start(vec![shell("echo hi"), say("never")]);
        let mut r = Run::new(&m.url());
        let w = toy(&mut r, mode, faults);
        let d = r.json("go");
        d.finish();
        let log = r.state().join("spill/.kernel-");
        let said = format!("policy binding {}: {why}; log {}", w.display(), log.display());
        assert_eq!(d.code, Some(3), "{faults}: {}", d.diag());
        d.check(d.notices().iter().filter(|n| n.starts_with(&said) && n.ends_with(FIX)).count() == 1, faults);
        d.check(d.elapsed < Duration::from_millis(ms), &format!("{faults}: {:?}", d.elapsed));
    }
}

/// A signal or the session deadline ends a hung `decide` at once: Ctrl-C ends the turn and exits 130 once the binding
/// had its 300 ms for `bye`; `--max-seconds` exits 124.
#[test]
fn e2e_policy_signals() {
    let m = Mock::start(vec![shell("echo hi"), say("never")]);
    let mut r = Run::new(&m.url());
    toy(&mut r, "allow", "--hang-at decide");
    let mut l = r.cmd(&["--json", "go"]).spawn();
    l.wait_record(W, |x| x["t"] == "assistant");
    std::thread::sleep(Duration::from_millis(300));
    let t0 = Instant::now();
    l.signal(libc::SIGINT);
    l.wait_record(W, |x| x["t"] == "turn_end");
    let ended = t0.elapsed();
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert!(ended < Duration::from_millis(100) && d.exited - t0 < Duration::from_millis(450), "{ended:?} {:?}", d.exited - t0);
    assert!(d.of("ask").is_empty() && d.notices().iter().all(|n| !n.contains("every call is asked")), "{}", d.diag());
    let m = Mock::start(vec![shell("echo hi"), say("never")]);
    r.at(&m.url());
    let d = r.cmd(&["--json", "--max-seconds", "1", "go"]).go();
    d.finish();
    assert_eq!(d.code, Some(124), "{}", d.diag());
    assert!(d.elapsed < Duration::from_millis(1_700), "{:?}", d.elapsed);
}

/// The REPL: a dead binding's call is asked with the note once, and every later call is asked at once without it; an
/// `always` the binding offers (cap record) is kept by it after the confirmation; a write is never offered `always`.
#[test]
fn pty_policy_asks() {
    let m = Mock::start(vec![shell("echo one"), shell("echo two"), say("both")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "allow", "--exit-at decide");
    let p = Pty::spawn(&r, &[], 24, 80);
    p.prompt(W);
    p.line("go");
    p.expect(&format!("policy binding {}: crashed", w.display()), W);
    p.expect("run? [y/N] ", W);
    p.line("y");
    p.expect("run? [y/N] ", W);
    let t0 = Instant::now();
    p.line("y");
    p.expect("both", W);
    p.prompt(W);
    assert!(t0.elapsed() < Duration::from_secs(2));
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.notices().iter().filter(|n| n.contains("every call is asked")).count(), 1, "{}", d.diag());
    assert_eq!(verdicts(&d), vec![json!(["yes", null, null]); 2]);
    let m = Mock::start(vec![shell("echo kept"), call("write", json!({"path": "w.txt", "content": "x"})), say("done")]);
    let mut r = Run::new(&m.url());
    toy(&mut r, "ask-always", "");
    let p = Pty::spawn(&r, &[], 24, 80);
    p.prompt(W);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect("? [y/N] ", W);
    p.line("y");
    p.expect("kept by port_toy\r\n", W);
    p.expect("run? [y/N] ", W);
    p.line("n");
    p.expect("done", W);
    p.prompt(W);
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.of("approval").iter().map(|a| a["verdict"].clone()).collect::<Vec<_>>(), ["always", "no"]);
    let kept = lines(&fs::read_to_string(r.state().join("port/policy/record.log")).unwrap());
    assert_eq!((kept.len(), &kept[0]["call"]["args"]["command"], &kept[0]["verdict"]), (1, &json!("echo kept"), &json!("always")));
    // An allow shows the binding's line in place of the call's head; the banner names the binding.
    let m = Mock::start(vec![shell("echo allowed"), say("done")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "allow", "").display().to_string();
    let p = Pty::spawn(&r, &[], 24, 200);
    p.expect(&format!("henki: policy: {w} (port/1, {} 0.1.0)", r.marker), W);
    p.prompt(W);
    p.line("go");
    p.expect(&format!("$ echo allowed  (allowed by policy {w}: port_toy allows every call)\r\n"), W);
    p.expect("done", W);
    p.prompt(W);
    p.line("/exit");
    p.wait(W).finish();
}

/// An error answer to `decide` is one failed call: that call is asked with a notice saying so, the binding lives on and
/// decides the next one; its death later is still said, once, and every call after it is asked.
#[test]
fn pty_policy_error_answer_asks_that_call() {
    let m = Mock::start(vec![shell("echo one"), shell("echo two"), shell("echo three"), shell("echo four"), say("done")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "allow", "--error-at decide#1 --exit-at decide#3").display().to_string();
    let p = Pty::spawn(&r, &[], 24, 200);
    p.prompt(W);
    p.line("go");
    p.expect(&format!("henki: policy binding {w}: io: a fault; log "), W);
    p.expect("or henki mods swap policy std; this call is asked\r\n", W);
    p.expect("run? [y/N] ", W);
    p.line("y");
    p.expect(&format!("$ echo two  (allowed by policy {w}: port_toy allows every call)\r\n"), W);
    p.expect(&format!("henki: policy binding {w}: crashed: it closed its stdout or exited; log "), W);
    p.expect(&format!("{FIX}\r\n"), W);
    p.expect("run? [y/N] ", W);
    p.line("y");
    p.expect("run? [y/N] ", W);
    p.line("y");
    p.expect("done", W);
    p.prompt(W);
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    let said = |end: &str| d.notices().iter().filter(|n| n.ends_with(end)).count();
    assert_eq!((said("; this call is asked"), said("; every call is asked")), (1, 1), "{}", d.diag());
    assert_eq!(d.of("approval").iter().map(|a| a["verdict"].clone()).collect::<Vec<_>>(), ["yes", "policy", "yes", "yes"]);
}

/// A signal during `decide` ends the turn even when other events are queued (a `record` observer's ended runs).
#[test]
fn e2e_policy_signal_with_observer() {
    let m = Mock::start(vec![shell("echo hi"), say("never")]);
    let mut r = Run::new(&m.url());
    toy(&mut r, "allow", "--hang-at decide");
    let on = r.config_dir().join("ext/obs/on");
    fs::create_dir_all(&on).unwrap();
    fs::write(on.parent().unwrap().join("doc.md"), "obs: watches\n").unwrap();
    fs::write(on.join("record"), "#!/bin/sh\nsleep 0.2\n").unwrap();
    fs::set_permissions(on.join("record"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut l = r.cmd(&["--json", "go"]).spawn();
    l.wait_record(W, |x| x["t"] == "assistant");
    std::thread::sleep(Duration::from_millis(600));
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!((d.code, d.status()), (Some(130), "interrupted".to_string()), "{}", d.diag());
    assert!(d.of("ask").is_empty(), "{}", d.diag());
}

/// JSON holds UTF-8 only: a cwd whose name is not refuses the start with the binding named (std runs there), and a
/// target whose real path is not is asked without asking the binding.
#[test]
fn e2e_policy_non_utf8_paths() {
    let m = Mock::start(vec![call("read", json!({"path": "a.txt"})), say("std read it")]);
    let mut r = Run::new(&m.url());
    let w = toy(&mut r, "allow", "");
    let dir = r.cwd.join(std::ffi::OsStr::from_bytes(b"w\xff"));
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    let d = r.cmd(&["--json", "read a.txt"]).cwd(&dir).go();
    d.finish();
    let says = format!("policy binding {}: {} is not UTF-8, which port/1 cannot send; log ", w.display(), dir.display());
    let e = &d.result()["error"];
    assert_eq!((d.code, &e["kind"]), (Some(2), &json!("config")), "{}", d.diag());
    d.check(e["message"].as_str().unwrap().starts_with(&says), "the refusal");
    let d = r.cmd(&["--json", "--port", "policy=std", "--approve", "auto", "read a.txt"]).cwd(&dir).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let m = Mock::start(vec![call("read", json!({"path": "link.txt"})), say("never")]);
    r.at(&m.url());
    symlink(dir.join("a.txt"), r.path("link.txt")).unwrap();
    let d = r.json("read the link");
    d.finish();
    assert_eq!(d.code, Some(3), "{}", d.diag());
    let says = format!("policy binding {}: the target ", w.display());
    d.check(d.notices().iter().any(|n| n.starts_with(&says) && n.ends_with(" is not UTF-8, which port/1 cannot send; this call is asked")), "the note");
    assert!(decided(&r).is_empty(), "the binding was asked: {:?}", decided(&r));
}
