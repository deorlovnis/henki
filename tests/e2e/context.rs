#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Context over port/1 (context/1): a binding linked at `CONFIG/port/context` shapes what each model request holds of
//! the history, henki checks every answer, and the std elision takes over for any answer henki does not take, for a
//! binding that dies, and for one that never starts. The journal always keeps the whole history. examples/port_toy's
//! context modes are the bindings; each carries the run's marker, so `Done::finish` proves none outlives its session.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

const EXCEEDED: &str = r#"{"error":{"message":"the request exceeds the available context size","type":"invalid_request_error"}}"#;

/// examples/port_toy, built by `cargo test` next to the henki under test.
fn port_toy() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_henki")).parent().unwrap().join("examples/port_toy")
}

/// `CONFIG/port/context` linked to a wrapper script `RUN/bindings/context` (in a 0700 dir) whose body is `body`;
/// returns the script's path as henki names it.
fn bind(r: &Run, body: &str) -> String {
    let w = r.dir.join("bindings/context");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&w, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let _ = fs::remove_file(port.join("context"));
    symlink(&w, port.join("context")).unwrap();
    w.display().to_string()
}

/// port_toy as the context binding in `mode` (with faults after it), saying the run's marker as its name.
fn toy(r: &Run, mode: &str) -> String {
    bind(r, &format!("exec {} context {mode} --name {}", port_toy().display(), r.marker))
}

/// The params of every `fit` the binding got, in order.
fn fits(r: &Run) -> Vec<Value> {
    lines(&fs::read_to_string(r.state().join("port/context/fit.log")).unwrap_or_default())
}

/// The users' texts of a request, by the start of each (the task line after henki's head lines).
fn users(l: &Logged) -> Vec<String> {
    l.contents("user").iter().map(|c| c.rsplit("\n\n").next().unwrap().to_string()).collect()
}

/// A request body with each shell call's time (`, 0.0 s]`) as `, T s]`, which differs from run to run.
fn untimed(body: &str) -> String {
    let mut out = String::new();
    for (i, part) in body.split(" s]").enumerate() {
        let cut = part.rfind(", ").filter(|&k| part[k + 2..].bytes().all(|b| b.is_ascii_digit() || b == b'.')).map_or(part, |k| &part[..k]);
        out += &format!("{}{}", if i > 0 { " s]" } else { "" }, if cut.len() < part.len() { format!("{cut}, T") } else { part.to_string() });
    }
    out
}

/// The binding shapes every request (`last 1`: only the last turn), while the journal keeps all of it; the binding gets
/// the whole history and the budget each time; `--port context=std` and a resume replay the whole history.
#[test]
fn e2e_context_shapes_requests() {
    let m = Mock::start(vec![shell("echo one"), say("one done"), shell("echo two"), say("two done"), shell("echo three"), say("three done"), say("four done")]);
    let r = Run::new(&m.url());
    let w = toy(&r, "last 1");
    for (i, args) in [&["--json", "first"][..], &["--json", "--resume", "second"], &["--json", "--resume", "third"]].into_iter().enumerate() {
        let d = r.cmd(args).go();
        d.finish();
        assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{}", d.diag());
        let notice = format!("context: {w} (port/1, {} 0.1.0); henki --port context=std runs a session without it", r.marker);
        d.check(d.notices().contains(&notice), "no binding notice");
        d.check(d.segment()[0]["ports"] == json!({"context": w}), "no session.ports.context");
        d.check(!d.notices().iter().any(|n| n.contains("std elide")), "a refused answer");
        if i == 2 {
            let all = d.journal_all();
            assert_eq!(all.iter().filter(|x| x["t"] == "user").count(), 3, "the journal keeps every turn");
            assert_eq!(all.iter().filter(|x| x["t"] == "tool_result").count(), 3);
        }
    }
    let log = m.log();
    let seen: Vec<Vec<String>> = log.iter().map(users).collect();
    assert_eq!(seen, [["first"], ["first"], ["second"], ["second"], ["third"], ["third"]], "each request holds only its own turn");
    assert_eq!(log[5].messages().iter().filter(|x| x["role"] == "tool").count(), 1);
    // The binding saw the whole history before every request, with the budget.
    let f = fits(&r);
    assert_eq!(f.len(), 6, "{f:?}");
    let sizes: Vec<usize> = f.iter().map(|p| p["records"].as_array().unwrap().len()).collect();
    assert_eq!(sizes, [1, 3, 5, 7, 9, 11]);
    let b = &f[5]["budget"];
    assert_eq!((b["window"].as_u64(), b["limit"].as_u64(), f[5]["overflowed"].as_bool()), (Some(32_768), Some(104_857), Some(false)), "{b}");
    assert!(b["fixed"].as_u64().unwrap() > 1_000 && b["max_tokens"].as_u64().unwrap() > 0, "{b}");
    assert!(f[5]["records"].as_array().unwrap().iter().all(|x| ["user", "assistant", "tool_result"].contains(&x["t"].as_str().unwrap())));
    // On std the resumed session's next request holds every turn.
    let d = r.cmd(&["--json", "--port", "context=std", "--resume", "fourth"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(users(&m.log()[6]), ["first", "second", "third", "fourth"]);
    assert_eq!(fits(&r).len(), 6, "the binding was not started");
    d.check(d.segment()[0]["ports"].is_null(), "session.ports names no binding");
}

/// An answer henki does not take (unpaired, over the limit, another last record, an invented call, an edited result)
/// is a notice naming the binding and why; the std elision shapes that request, and the binding is asked again for
/// the next one.
#[test]
fn e2e_context_refused_answers() {
    let cases = [
        ("unpaired", "has no result", 1),
        ("huge", "bytes, over the limit of 104857", 2),
        ("wrong-last", "it does not end with the history's last record", 2),
        ("invent-call", "a call or result that is not the history's", 2),
        ("edit-result", "a call or result that is not the history's", 1),
    ];
    for (mode, why, n) in cases {
        let script = vec![shell("echo hi"), say("done")];
        let (m, std) = (Mock::start(script.clone()), Mock::start(script));
        let mut r = Run::new(&m.url());
        let w = toy(&r, mode);
        let d = r.json("one");
        d.finish();
        assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{mode}: {}", d.diag());
        let refused: Vec<String> = d.notices().into_iter().filter(|x| x.starts_with(&format!("context binding {w}: fit: "))).collect();
        assert_eq!(refused.len(), n, "{mode}: {refused:?}");
        assert!(refused.iter().all(|x| x.contains(why) && x.ends_with("; std elide shapes this request") && x.contains("fix: henki --port context=std")), "{mode}: {refused:?}");
        assert_eq!(fits(&r).len(), 2, "{mode}: asked before both requests");
        // Each request is the one std sends.
        r.at(&std.url());
        let s = r.cmd(&["--json", "--port", "context=std", "one"]).go();
        s.finish();
        let (got, want): (Vec<String>, Vec<String>) = (m.log().iter().map(|l| untimed(l.text())).collect(), std.log().iter().map(|l| untimed(l.text())).collect());
        assert_eq!(got, want, "{mode}");
    }
}

/// A binding that breaks its lines or exits is dead for the session: one notice, then std for every request, and its
/// process is gone. An error answer fails one request; the binding lives on.
#[test]
fn e2e_context_dead_or_failing() {
    for (fault, why, n, asked) in [
        ("--garbage-at fit", "protocol: a line that is not a JSON object", 1, 1),
        ("--exit-at fit", "crashed: it closed its stdout or exited", 1, 1),
        ("--error-at fit", "io: a fault", 2, 2),
    ] {
        let m = Mock::start(vec![shell("echo hi"), say("done")]);
        let r = Run::new(&m.url());
        let w = toy(&r, &format!("keep {fault}"));
        let d = r.json("one");
        d.finish();
        assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{fault}: {}", d.diag());
        let said: Vec<String> = d.notices().into_iter().filter(|x| x.starts_with(&format!("context binding {w}: "))).collect();
        let end = if n == 1 { "; std elide shapes the rest of the session" } else { "; std elide shapes this request" };
        assert_eq!(said.len(), n, "{fault}: {said:?}");
        assert!(said.iter().all(|x| x.contains(why) && x.ends_with(end)), "{fault}: {said:?}");
        assert_eq!((fits(&r).len(), m.log().len()), (asked, 2), "{fault}");
    }
}

/// A binding that never answers `fit`: after its 10 s the turn goes on on std, and later requests do not wait for it.
#[test]
fn e2e_context_hung() {
    let m = Mock::start(vec![shell("echo hi"), say("done")]);
    let r = Run::new(&m.url());
    let w = toy(&r, "keep --hang-at fit");
    let d = r.json("one");
    d.finish();
    assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{}", d.diag());
    let said: Vec<String> = d.notices().into_iter().filter(|x| x.starts_with(&format!("context binding {w}: "))).collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("timeout: no answer to fit in 10.0 s") && said[0].ends_with("; std elide shapes the rest of the session"), "{said:?}");
    assert!(d.elapsed >= Duration::from_secs(10) && d.elapsed < Duration::from_secs(14), "{:?}", d.elapsed);
    assert_eq!(m.log().len(), 2);
}

/// A server overflow on the binding's answer asks the binding once more with `overflowed`, then std decides.
#[test]
fn e2e_context_overflow_asks_again() {
    let m = Mock::start(vec![error(400, EXCEEDED, 2), say("fitted")]);
    let r = Run::new(&m.url());
    toy(&r, "last 1");
    let d = r.json("one");
    d.finish();
    assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{}", d.diag());
    let asked: Vec<Value> = fits(&r).iter().map(|p| p["overflowed"].clone()).collect();
    assert_eq!(asked, [json!(false), json!(true)], "asked again with overflowed, then not again");
    assert_eq!(m.log().len(), 3, "the binding's answer, its second answer, then std's");
    assert!(d.of("elide").is_empty(), "nothing to elide");
}

/// With the anthropic dialect the binding is not started, and the notice says so.
#[test]
fn e2e_context_anthropic_not_started() {
    let m = Mock::start_api(Api::Anthropic, vec![say("hello")]);
    let mut r = Run::new("");
    r.at(&m.url()).set("dialect", "\"anthropic\"");
    let w = toy(&r, "keep");
    let d = r.json("one");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    d.check(d.notices().contains(&format!("context: {w} is not used with the anthropic dialect")), "no notice");
    d.check(d.segment()[0]["ports"].is_null(), "session.ports names it");
    assert!(!r.state().join("port/context/pid").exists(), "the binding started");
}

/// A binding that does not start or answer `hello` is one notice, and the session runs on std; a link that names
/// nothing runnable stops the start (port/1's link rule).
#[test]
fn e2e_context_start_failure() {
    for (body, why) in [("exit 3", "crashed: it closed its stdout or exited"), ("exec sleep MARK", "timeout: no answer to hello in 5.0 s")] {
        let m = Mock::start(vec![say("hello")]);
        let r = Run::new(&m.url());
        let w = bind(&r, &body.replace("MARK", &r.marker));
        let d = r.json("one");
        d.finish();
        assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{body}: {}", d.diag());
        let said: Vec<String> = d.notices().into_iter().filter(|x| x.starts_with(&format!("context binding {w}: "))).collect();
        assert!(said.len() == 1 && said[0].contains(why) && said[0].ends_with("; std elide shapes this session"), "{body}: {said:?}");
        d.check(d.segment()[0]["ports"].is_null(), "session.ports names it");
        assert_eq!(m.log().len(), 1);
    }
    let r = Run::new("http://127.0.0.1:9/v1");
    fs::create_dir_all(r.config_dir().join("port")).unwrap();
    symlink(r.dir.join("nothing"), r.config_dir().join("port/context")).unwrap();
    let d = r.json("one");
    d.finish();
    assert_eq!(d.code, Some(2), "{}", d.diag());
    assert!(d.result()["error"]["message"].as_str().unwrap().contains("port/context: not a link to a program"), "{}", d.diag());
}

/// `keep` leaves every request to std: byte for byte what an unbound session sends.
#[test]
fn e2e_context_keep_is_std() {
    let script = vec![shell("echo one"), say("one done"), say("two done")];
    let mut sent = Vec::new();
    for bound in [false, true] {
        let m = Mock::start(script.clone());
        let r = Run::new(&m.url());
        if bound {
            toy(&r, "keep");
        }
        for args in [&["--json", "first"][..], &["--json", "--resume", "second"]] {
            let d = r.cmd(args).go();
            d.finish();
            assert_eq!(d.code, Some(0), "{}", d.diag());
        }
        assert_eq!(fits(&r).len(), if bound { 3 } else { 0 });
        sent.push(m.log().iter().map(|l| untimed(l.text()).replace(&r.dir.display().to_string(), "RUN")).collect::<Vec<_>>());
    }
    assert_eq!(sent[0].len(), 3);
    assert_eq!(sent[0], sent[1]);
}

/// A signal while the binding works on `fit` ends the turn at once (interrupted), and kills the binding at the end.
#[test]
fn e2e_context_signal_during_fit() {
    let m = Mock::start(vec![say("never")]);
    let r = Run::new(&m.url());
    toy(&r, "keep --hang-at fit");
    let mut l = r.cmd(&["--json", "one"]).spawn();
    l.until("the binding got fit", 10 * SEC, |_| !fits(&r).is_empty());
    let t = std::time::Instant::now();
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(130), "interrupted"), "{}", d.diag());
    assert!(d.exited - t < Duration::from_millis(1000), "exit {:?} after the signal", d.exited - t);
    assert!(m.log().is_empty());
}
