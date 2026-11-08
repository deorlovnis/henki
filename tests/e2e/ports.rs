#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! History over port/1: a session kept by an outside binding (the bash toy of the conformance suite, and
//! examples/port_toy with its faults), the binding notice and `session.ports`, and what henki does when a binding hangs,
//! lies, breaks its lines, dies, floods its stderr or leaves a process behind. Every binding here carries the run's
//! marker in its command line, so `Done::finish` also proves that none outlives its session.
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

/// examples/port_toy, built by `cargo test` next to the henki under test.
fn port_toy() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_henki")).parent().unwrap().join("examples/port_toy")
}

/// `CONFIG/port/history` linked to a wrapper script `at` (default `RUN/bindings/history`, in 0700 dirs) running `cmd`.
fn bind_at(r: &Run, at: Option<&Path>, cmd: &str) -> PathBuf {
    let w = at.map_or_else(|| r.dir.join("bindings/history"), Path::to_path_buf);
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&w, format!("#!/bin/sh\n{cmd}\n")).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    let port = r.config_dir().join("port");
    fs::create_dir_all(&port).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    let _ = fs::remove_file(port.join("history"));
    symlink(&w, port.join("history")).unwrap();
    w
}

/// port_toy as the history binding, with `faults`; it says the run's marker as its name.
fn toy(r: &Run, faults: &str) -> PathBuf {
    bind_at(r, None, &format!("exec {} history --name {} {faults}", port_toy().display(), r.marker))
}

/// The std journals of the run (none while history is bound).
fn std_journals(r: &Run) -> Vec<PathBuf> {
    fs::read_dir(r.state().join("sessions")).into_iter().flatten().flatten().flat_map(|d| fs::read_dir(d.path()).into_iter().flatten().flatten()).map(|e| e.path()).collect()
}

/// The binding's store: the state dir hello names.
fn store(r: &Run) -> PathBuf {
    r.state().join("port/history")
}

/// The records of every session in the toy's store, by session id.
fn kept(r: &Run) -> Vec<(String, Vec<Value>)> {
    let mut out: Vec<(String, Vec<Value>)> = fs::read_dir(store(r))
        .unwrap()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(".jsonl")?.to_string();
            Some((name, lines(&fs::read_to_string(e.path()).unwrap())))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A request's messages without call ids (the mock makes its own) and with the run's cwd as CWD.
fn bodies(m: &Mock, cwd: &Path) -> Vec<String> {
    fn strip(v: &mut Value) {
        if let Some(o) = v.as_object_mut() {
            o.remove("id");
            o.remove("tool_call_id");
            o.values_mut().for_each(strip);
        } else if let Some(a) = v.as_array_mut() {
            a.iter_mut().for_each(strip);
        }
    }
    m.log()
        .iter()
        .map(|l| {
            let mut v = Value::from(l.messages());
            strip(&mut v);
            v.to_string().replace(&cwd.display().to_string(), "CWD")
        })
        .collect()
}

/// Three `run`s of one session (two with a call, each resuming the last), on std or on a history binding: the model sees
/// the same requests, and a bound session leaves no std journal; its store holds the three segments.
#[test]
fn e2e_port_history_like_std() {
    let script = || vec![shell("echo one"), say("one done"), shell("echo two"), say("two done"), say("three")];
    let mut seen = Vec::new();
    for bound in [false, true] {
        let m = Mock::start(script());
        let r = Run::new(&m.url());
        if bound {
            bind_at(&r, None, &format!("exec bash {}/tests/fixtures/ports/toy-history {}", env!("CARGO_MANIFEST_DIR"), r.marker));
        }
        for args in [&["--json", "first"][..], &["--json", "--resume", "second"], &["--json", "--resume", "third"]] {
            let d = r.cmd(args).go();
            d.finish();
            assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{}", d.diag());
            if bound {
                let w = r.dir.join("bindings/history").display().to_string();
                let notice = format!("history: {w} (port/1, toy-history 0.1.0); henki --port history=std runs a session without it");
                let out = d.records();
                d.check(out.iter().any(|x| x["t"] == "notice" && x["text"] == notice.as_str()), "no binding notice");
                d.check(out.iter().find(|x| x["t"] == "session").unwrap()["ports"] == json!({"history": w}), "no session.ports");
            }
        }
        if bound {
            assert!(std_journals(&r).is_empty(), "a std journal");
            let k = kept(&r);
            assert_eq!(k.len(), 1, "{k:?}");
            let kinds: Vec<&str> = k[0].1.iter().filter_map(|x| x["t"].as_str()).filter(|t| *t == "session" || *t == "close").collect();
            assert_eq!(kinds, ["session", "close", "session", "close", "session", "close"]);
            assert_eq!(k[0].1.iter().filter(|x| x["t"] == "session").map(|x| x["resumed"].clone()).collect::<Vec<_>>(), [Value::Null, json!({}), json!({})]);
        }
        seen.push(bodies(&m, &r.cwd));
    }
    assert_eq!(seen[0].len(), 5);
    assert_eq!(seen[0], seen[1], "the model saw other requests with history bound");
}

/// The REPL on a bound history, as on std: three turns with calls, `/exit`, `henki --resume` and a fourth; the model
/// sees the same requests, the banner shows the binding, its store keeps the session, and a REPL that got no message
/// leaves no session there.
#[test]
fn pty_port_history() {
    let mut seen = Vec::new();
    for bound in [false, true] {
        let m = Mock::start(vec![shell("echo one"), say("one done"), shell("echo two"), say("two done"), say("three done"), say("four done")]);
        let r = Run::new(&m.url());
        if bound {
            toy(&r, "");
        }
        for (args, lines) in [(&[][..], &["one", "two", "three"][..]), (&["--resume"][..], &["four"][..])] {
            let p = Pty::spawn(&r, args, 24, 80);
            if bound && args.is_empty() {
                p.expect(&format!("henki: history: {} (port/1, {} 0.1.0)", r.dir.join("bindings/history").display(), r.marker), W);
            }
            p.prompt(W);
            for l in lines {
                p.line(l);
                p.expect(&format!("{l} done"), W);
                p.prompt(W);
            }
            p.line("/exit");
            let d = p.wait(W);
            d.finish();
            assert_eq!(d.code, Some(0), "{}", d.diag());
        }
        seen.push(bodies(&m, &r.cwd));
        if bound {
            assert!(std_journals(&r).is_empty(), "a std journal");
            let k = kept(&r);
            assert_eq!(k.len(), 1);
            assert_eq!(k[0].1.iter().filter(|x| x["t"] == "user").count(), 4, "{k:?}");
            let p = Pty::spawn(&r, &[], 24, 80);
            p.prompt(W);
            p.line("/exit");
            p.wait(W).finish();
            assert_eq!(kept(&r).len(), 1, "the empty session was not discarded");
        }
    }
    assert_eq!(seen[0].len(), 6);
    assert_eq!(seen[0], seen[1], "the model saw other requests with history bound");
}

/// A binding that fails to start, to greet, or to give a session stops the start: exit 2 (`config` before the session,
/// `journal` at it), the reason named after the binding, the kernel log and the repair; nothing of it is left running,
/// not even (Linux) a daemon it started in a session of its own, which only the sweep after the refusal takes.
#[test]
fn e2e_port_refused_starts() {
    let cases = [
        ("--exit-at hello", "crashed: it closed its stdout or exited"),
        ("--chatty", "protocol: a line that is not a JSON object"),
        ("--bad-id", "protocol: no valid session id in the answer"),
        ("--huge-at create", "protocol: a line over 16,777,216 bytes, or cut"),
        ("--error-at create", "io: a fault"),
        ("--no-read", "timeout: no answer to create in 2.0 s"),
        ("--hang-at hello", "timeout: no answer to hello in 5.0 s"),
    ];
    for (faults, why) in cases {
        let m = Mock::start(vec![say("never")]);
        let r = Run::new(&m.url());
        let daemon = if cfg!(target_os = "linux") { format!("(setsid sleep {} </dev/null >/dev/null 2>&1 &)\n", r.marker) } else { String::new() };
        let w = bind_at(&r, None, &format!("{daemon}exec {} history --name {} {faults}", port_toy().display(), r.marker));
        let d = r.json("hi");
        d.finish();
        let e = &d.result()["error"];
        let msg = e["message"].as_str().unwrap_or_default();
        let fix = "; fix: henki --port history=std for this session, or henki mods swap history std";
        let log = r.state().join("spill/.kernel-");
        d.check(d.code == Some(2) && (e["kind"] == "config" || e["kind"] == "journal"), faults);
        d.check(msg.starts_with(&format!("history binding {}: {why}; log {}", w.display(), log.display())) && msg.ends_with(fix), faults);
        assert!(m.log().is_empty() && std_journals(&r).is_empty(), "{faults}: a request or a std journal");
    }
}

/// JSON holds UTF-8 only: a history binding in a cwd whose name is not refuses the start, naming the binding and
/// the cwd, before any record; std runs there.
#[test]
fn e2e_port_non_utf8_cwd() {
    let m = Mock::start(vec![say("on std")]);
    let r = Run::new(&m.url());
    let w = toy(&r, "");
    let dir = r.cwd.join(std::ffi::OsStr::from_bytes(b"w\xff"));
    fs::create_dir(&dir).unwrap();
    let d = r.cmd(&["--json", "hi"]).cwd(&dir).go();
    d.finish();
    let says = format!("history binding {}: {} is not UTF-8, which port/1 cannot send; log ", w.display(), dir.display());
    assert_eq!((d.code, &d.result()["error"]["kind"]), (Some(2), &json!("config")), "{}", d.diag());
    d.check(d.result()["error"]["message"].as_str().unwrap().starts_with(&says), "the refusal");
    assert!(m.log().is_empty() && fs::read_dir(store(&r)).unwrap().next().is_none(), "a request, or a session kept");
    let d = r.cmd(&["--json", "--port", "history=std", "hi"]).cwd(&dir).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
}

/// What the start checks before it runs anything: an entry for a port this henki cannot bind, a binding another user
/// may change, a link to nothing; `--port history=std` runs past a broken binding, on std.
#[test]
fn e2e_port_links_checked() {
    let m = Mock::start(vec![say("on std")]);
    let r = Run::new(&m.url());
    let w = toy(&r, "");
    let port = r.config_dir().join("port");
    fs::set_permissions(&w, fs::Permissions::from_mode(0o720)).unwrap();
    let d = r.json("hi");
    d.finish();
    d.check(d.result()["error"]["message"].as_str().unwrap().starts_with(&format!("{} is group- or world-writable", w.display())), "a writable binding ran");
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&w, port.join("context")).unwrap();
    let d = r.json("hi");
    d.finish();
    let says = format!("{}: this henki cannot bind that port; fix: henki mods swap context std, or upgrade henki", port.join("context").display());
    d.check(d.result()["error"]["message"] == says.as_str(), "an unknown port entry");
    fs::remove_file(port.join("context")).unwrap();
    fs::write(port.join(".history.123"), "").unwrap(); // the manager's temporary names are skipped
    fs::remove_file(&w).unwrap();
    let d = r.json("hi");
    d.finish();
    d.check(d.result()["error"]["message"].as_str().unwrap().contains("not a link to a program"), "a link to nothing");
    let d = r.cmd(&["--json", "--port", "history=std", "hi"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.journal.is_some() && !d.of("session")[0].as_object().unwrap().contains_key("ports"), "{}", d.diag());
}

/// A binding that hangs, stops or dies mid-turn: the turn ends `error journal` (exit 2) within the append timeout,
/// with the repair, and the binding is killed.
#[test]
fn e2e_port_dead_mid_turn() {
    let pid = |r: &Run| fs::read_to_string(store(r).join("pid")).unwrap();
    for how in ["hang", "STOP", "KILL"] {
        let mut r = Run::new("");
        let shot = format!("kill -{how} $(cat {})", store(&r).join("pid").display());
        let m = Mock::start(vec![if how == "hang" { shell("echo one") } else { shell(&shot) }, say("never")]);
        r.at(&m.url());
        // Records: session, notice, user, assistant, approval, then the call's tool_result: the 6th append.
        toy(&r, if how == "hang" { "--hang-at append#6" } else { "" });
        let t0 = Instant::now();
        let d = r.json("go");
        let took = t0.elapsed();
        // Not `finish`: the store lacks what the dead binding never took, so its pairing cannot hold. Nothing survives.
        assert!(marked(&r.marker).is_empty(), "{how}: a process with the marker survived");
        let e = &d.result()["error"];
        let msg = e["message"].as_str().unwrap_or_default();
        d.check(d.code == Some(2) && e["kind"] == "journal" && msg.ends_with("fix: henki --port history=std for this session, or henki mods swap history std"), how);
        let why = if how == "KILL" { "crashed" } else { "timeout: no answer to append in 2.0 s" };
        d.check(msg.contains(why), how);
        d.check(took < Duration::from_millis(if how == "KILL" { 1_500 } else { 3_500 }), &format!("{how}: {took:?}"));
        let gone = !Path::new(&format!("/proc/{}", pid(&r).trim())).exists();
        assert!(!cfg!(target_os = "linux") || gone, "{how}: the binding still runs");
    }
}

/// A binding's processes and output stay its own: a daemon it leaves dies with the session, its stderr goes to the
/// kernel log, and it never sees the API key, a token or the user's shell tokens.
#[test]
fn e2e_port_daemon_and_stderr() {
    let m = Mock::start(vec![say("fine")]);
    let mut r = Run::new(&m.url());
    r.env("SOME_TOKEN", "t").env("HENKI_DEPTH", "0");
    toy(&r, &format!("--daemon {} --stderr-flood", r.marker));
    let d = r.cmd(&["--json", "--nested", "hi"]).go();
    d.finish(); // no process with the marker survives: the daemon was killed with the binding's session
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let logs: Vec<_> = fs::read_dir(r.state().join("spill")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".kernel-")).collect();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].metadata().unwrap().len() >= 8 << 20 && logs[0].metadata().unwrap().permissions().mode() & 0o777 == 0o600);
}

/// The binding's name comes from the store path it is in, never from its hello; its environment is the tool one,
/// without the extensions, a depth deeper, no session.
#[test]
fn e2e_port_name_and_env() {
    let m = Mock::start(vec![say("fine")]);
    let mut r = Run::new(&m.url());
    r.env("SOME_TOKEN", "t");
    let at = r.dir.join("xdg/data/henki/mods/tree/0a1b2c3d4e5f/port/history");
    bind_at(&r, Some(&at), &format!("env >{}/env\nexec {} history --name std", r.dir.display(), port_toy().display()));
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let notice = "history: tree 0a1b2c3d4e5f (port/1, std 0.1.0); henki --port history=std runs a session without it";
    assert!(d.records().iter().any(|x| x["text"] == notice), "{}", d.diag());
    assert_eq!(d.records().iter().find(|x| x["t"] == "session").unwrap()["ports"], json!({"history": "tree 0a1b2c3d4e5f"}));
    let env = fs::read_to_string(r.dir.join("env")).unwrap();
    let get = |k: &str| env.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(String::from));
    assert_eq!((get("SOME_TOKEN"), get("HENKI_SESSION"), get("HENKI_DEPTH")), (None, Some(String::new()), Some("1".into())), "{env}");
    assert_eq!(get("PATH"), get("HENKI_USER_PATH"), "{env}");
}

/// A swap while the session runs (the link retargeted) trips the wire, as any trust file does.
#[test]
fn e2e_port_link_changed() {
    let mut r = Run::new("");
    let link = r.config_dir().join("port/history");
    let m = Mock::start(vec![shell(&format!("ln -sfn /bin/true {}", link.display())), say("never")]);
    r.at(&m.url());
    toy(&r, "");
    let d = r.json("swap it");
    d.finish();
    let msg = d.result()["error"]["message"].as_str().unwrap_or_default().to_string();
    d.check(d.code == Some(2) && msg.starts_with("trust file changed: ") && msg.contains(&r.config_dir().join("port").display().to_string()), "not tripped");
}

/// Ctrl-C while a slow binding has not said hello yet ends the start at once, exit 130; a SIGKILLed henki takes its
/// binding with it (Linux: the parent-death signal).
#[test]
fn e2e_port_signals() {
    let m = Mock::start(vec![shell("exec sleep 3133"), say("never")]);
    let r = Run::new(&m.url());
    toy(&r, "--slow 3000");
    let mut l = r.cmd(&["--json", "hi"]).spawn();
    l.ready();
    l.wait_marked(5 * SEC);
    std::thread::sleep(Duration::from_millis(200));
    let t0 = Instant::now();
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert!(d.exited - t0 < Duration::from_millis(150), "{:?}", d.exited - t0);
    // The deadline of `run --max-seconds` too: exit 124 at the deadline, not after the slow hello.
    let r = Run::new(&m.url());
    toy(&r, "--slow 3000");
    let d = r.cmd(&["--json", "--max-seconds", "1", "hi"]).go();
    d.finish();
    assert_eq!(d.code, Some(124), "{}", d.diag());
    assert!(d.elapsed < Duration::from_millis(1_400), "{:?}", d.elapsed);
    if cfg!(target_os = "linux") {
        let r = Run::new(&m.url());
        toy(&r, "");
        let mut l = r.cmd(&["--json", "hi"]).spawn();
        l.until("the binding's store", W, |_| store(&r).join("pid").exists());
        l.kill9();
        let d = l.wait();
        d.finish();
    }
}

/// The cost of history over port/1 in a whole session: 20 steps against the mock, std against port_toy, per record.
#[test]
#[ignore = "release gate"]
fn e2e_port_session_overhead() {
    let mut per = Vec::new();
    for bound in [false, true] {
        let steps: Vec<Step> = (0..20).map(|_| shell("true")).chain([say("done")]).collect();
        let m = Mock::start(steps);
        let r = Run::new(&m.url());
        if bound {
            toy(&r, "");
        }
        let d = r.cmd(&["--json", "--max-steps", "30", "go"]).go();
        d.finish();
        assert_eq!(d.code, Some(0), "{}", d.diag());
        let n = d.records().iter().filter(|x| x["t"] != "text" && x["t"] != "reasoning").count();
        per.push((d.elapsed.as_secs_f64() * 1e3, n));
    }
    let extra = (per[1].0 - per[0].0) / per[1].1 as f64;
    eprintln!("20-step session: std {:.1} ms, port_toy {:.1} ms, {} records: {extra:.3} ms more per record", per[0].0, per[1].0, per[1].1);
    assert!(extra <= 5.0, "{extra:.3} ms more per record");
}
