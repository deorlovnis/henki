#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! observe/1: a module's `on/EVENT` programs, run once per event with the event on stdin, the `.wait` ones waited for
//! up to 2 s, slow ones skipped, every run killed at the session end, and nothing they do getting past the tripwire.
use crate::harness::*;
use crate::mock::*;
use crate::pty::Pty;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

const W: Duration = Duration::from_secs(15);

/// The module `name` in the default ext dir with the observers `on` (file name, sh script body).
fn module(r: &Run, name: &str, on: &[(&str, &str)]) -> PathBuf {
    let x = r.config_dir().join("ext").join(name);
    fs::create_dir_all(x.join("on")).unwrap();
    fs::write(x.join("doc.md"), format!("{name}: watches the session\n")).unwrap();
    for (file, body) in on {
        fs::write(x.join("on").join(file), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(x.join("on").join(file), fs::Permissions::from_mode(0o755)).unwrap();
    }
    x
}

/// An observer body that keeps what its run got in `DIR/obs/EVENT.PID`: the event line, then a line of its
/// environment (`EVENT|MODULE|STATE|NEXT|PWD|SESSION`); it writes to stdout and stderr too (the kernel log).
fn keeper(r: &Run) -> String {
    let d = r.dir.join("obs");
    fs::create_dir_all(&d).unwrap();
    let env = r#""$HENKI_EVENT" "$HENKI_MODULE" "$HENKI_STATE" "${HENKI_NEXT-none}" "$(pwd -P)" "$HENKI_SESSION""#;
    format!("f={}/$HENKI_EVENT.$$\n{{ cat; printf '%s|%s|%s|%s|%s|%s\\n' {env}; }} >\"$f.tmp\" && mv \"$f.tmp\" \"$f\"\necho to-the-log; echo err-to-the-log >&2", d.display())
}

/// What the keepers got, in `seq` order: (event line, its environment fields).
fn kept(r: &Run) -> Vec<(Value, Vec<String>)> {
    let mut got: Vec<(Value, Vec<String>)> = fs::read_dir(r.dir.join("obs"))
        .map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().ends_with(".tmp")).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|e| {
            let text = fs::read_to_string(e.path()).unwrap();
            let (line, env) = text.split_once('\n').unwrap();
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(line, v.to_string(), "the event line is not canonical JSON");
            (v, env.trim_end().split('|').map(String::from).collect())
        })
        .collect();
    got.sort_by_key(|g| g.0["seq"].as_u64().unwrap());
    got
}

/// O1: every event in order, with its data, the envelope and the environment; stdout and stderr in the kernel log.
#[test]
fn observe_events_in_order() {
    let m = Mock::start(vec![shell("echo hi"), call("write", json!({"path": "f.txt", "content": "x"})), say("done")]);
    let r = Run::new(&m.url());
    let k = keeper(&r);
    let all = ["session_start", "turn_start.wait", "call_start.wait", "call_end", "turn_end", "question", "session_end", "record"];
    let x = module(&r, "watch", &all.map(|e| (e, k.as_str())));
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let recs = d.journal_all();
    // The events the journal implies: each record's own, then `record`; `call_start` after each approval.
    let mut want: Vec<(String, Value)> = vec![("session_start".into(), recs[0].clone())];
    for rec in &recs[1..] {
        let own = match rec["t"].as_str().unwrap() {
            "user" => Some("turn_start"),
            "tool_result" => Some("call_end"),
            "turn_end" => Some("turn_end"),
            "ask" => Some("question"),
            "result" => continue,
            "close" => {
                want.push(("session_end".into(), rec.clone()));
                continue;
            }
            _ => None,
        };
        want.extend(own.map(|e| (e.to_string(), rec.clone())));
        want.push(("record".into(), rec.clone()));
        if rec["t"] == "approval" {
            let c = recs.iter().filter_map(|r| r["msg"]["tool_calls"].as_array()).flatten().find(|c| c["id"] == rec["id"]).unwrap();
            let target = (c["name"] == "write").then(|| fs::canonicalize(&r.cwd).unwrap().join("f.txt"));
            let data = json!({"call": {"args": c["args"], "id": c["id"], "name": c["name"]}, "rule": null, "target": target, "verdict": "auto"});
            want.push(("call_start".into(), data));
        }
    }
    let got = kept(&r);
    let names = |v: &[(String, Value)]| v.iter().map(|w| w.0.clone()).collect::<Vec<_>>();
    assert_eq!(got.iter().map(|g| g.0["event"].as_str().unwrap().to_string()).collect::<Vec<_>>(), names(&want), "{}", d.diag());
    let state = r.state();
    for (i, ((line, env), (event, data))) in got.iter().zip(&want).enumerate() {
        assert_eq!((&line["data"], line["seq"].as_u64(), &line["session"], &line["v"]), (data, Some(i as u64 + 1), &json!(d.id()), &json!(1)), "{event}");
        let pwd = fs::canonicalize(&r.cwd).unwrap();
        let want_env = [event.as_str(), &x.to_string_lossy(), &state.to_string_lossy(), "none", &pwd.to_string_lossy(), &d.id()];
        assert_eq!(env, &want_env, "{event}");
    }
    let log = fs::read_dir(state.join("spill")).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with(".kernel-")).unwrap().path();
    let log = fs::read_to_string(log).unwrap();
    assert_eq!((log.matches("to-the-log\n").count(), log.matches("err-to-the-log\n").count()), (got.len() * 2, got.len()), "{log}");
    assert!(!fs::read_dir(state.join("spill")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".event-")), "an event file was left");
}

/// O2, O4: an observer that never reads stdin and never ends misses events once it has 4 runs going, with one notice;
/// the turn is not held up, also by a call output it never reads; the session end kills the runs.
#[test]
fn observe_slow_observer_misses_events() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell("head -c 3000000 /dev/zero | tr '\\0' x"), shell(":"), shell(":"), say("done")]);
    r.at(&m.url());
    module(&r, "slow", &[("record", &r.sleep())]);
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let slow = "observer slow/on/record is slow: 4 runs still going; it misses events until one ends";
    assert_eq!(d.notices().iter().filter(|n| *n == slow).count(), 1, "{}", d.diag());
    assert!(d.elapsed < Duration::from_secs(3), "the run took {:?}", d.elapsed);
}

/// O3: a `.wait` observer that does not end holds the first call 2 s, gets one notice, and is not waited for again.
#[test]
fn observe_wait_runs_out_once() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(":"), shell(":"), shell(":"), say("done")]);
    r.at(&m.url());
    module(&r, "slow", &[("call_start.wait", &r.sleep())]);
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let late = "observer slow/on/call_start.wait took over 2 s; it is no longer waited for";
    assert_eq!(d.notices().iter().filter(|n| *n == late).count(), 1, "{}", d.diag());
    let log = m.log();
    let gap = |i: usize| log[i + 1].at - log[i].done.unwrap();
    assert!(gap(0) >= Duration::from_secs(2) && gap(0) < Duration::from_millis(2600), "the first call waited {:?}", gap(0));
    assert!(gap(1) < Duration::from_millis(500) && gap(2) < Duration::from_millis(500), "later calls waited {:?} and {:?}", gap(1), gap(2));
}

/// A `.wait` observer that ends is waited for, every time, before its call runs.
#[test]
fn observe_wait_run_lands_before_the_call() {
    let m = Mock::start(vec![shell("cat stamp"), shell("cat stamp"), say("done")]);
    let r = Run::new(&m.url());
    module(&r, "stamp", &[("call_start.wait", "sleep 0.3; n=$(cat stamp 2>/dev/null || echo 0); echo $((n + 1)) >stamp"), ("turn_start.wait", "echo 0 >stamp")]);
    let d = r.json("hi");
    d.finish();
    let out: Vec<_> = d.results().iter().map(|r| r["content"].as_str().unwrap().to_string()).collect();
    assert!(out[0].starts_with("1\n") && out[1].starts_with("2\n"), "{out:?}");
}

/// O5: Ctrl-C while a `.wait` observer is waited for ends the turn at once; the call never ran.
#[test]
fn observe_signal_ends_the_wait() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell("echo ran >ran.txt"), say("never")]);
    r.at(&m.url());
    module(&r, "slow", &[("call_start.wait", &r.sleep())]);
    let mut l = r.cmd(&["--json", "wait"]).spawn();
    l.ready();
    l.wait_marked(5 * SEC);
    let t = std::time::Instant::now();
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(130), "interrupted"), "{}", d.diag());
    // The session end's grace for the run still going is part of the time to exit.
    assert!(d.exited - t < Duration::from_millis(800), "exit {:?} after the signal", d.exited - t);
    assert_eq!(d.results()[0]["content"], "not run: interrupted");
    assert!(!r.cwd.join("ran.txt").exists());
}

/// O7: a `.wait` observer that changes a trust file at call_start: the tripwire after it stops the call.
#[test]
fn observe_trust_change_stops_the_call() {
    let m = Mock::start(vec![shell("echo ran >ran.txt"), say("never")]);
    let r = Run::new(&m.url());
    module(&r, "meddle", &[("call_start.wait", &format!("echo '# changed' >>{}", r.config_path.display()))]);
    let d = r.json("hi");
    d.finish();
    assert_eq!((d.status().as_str(), &d.result()["error"]["kind"]), ("error", &json!("changed")), "{}", d.diag());
    assert_eq!(d.results()[0]["content"], "not run: trust file changed");
    assert!(!r.cwd.join("ran.txt").exists(), "the call ran");
}

/// A run started at session_end gets the grace, then is killed with its session; nothing with the marker is left.
#[test]
fn observe_session_end_run_killed() {
    let mut r = Run::new("");
    let m = Mock::start(vec![say("done")]);
    r.at(&m.url());
    module(&r, "late", &[("session_end", &r.sleep())]);
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.elapsed < Duration::from_secs(2), "the run took {:?}", d.elapsed);
}

/// The REPL: no event before the first message (none at all without one); `session_start` once, `turn_start` per
/// message; an observer that cannot start is a notice, journaled.
#[test]
fn observe_repl_events() {
    let m = Mock::start(vec![say("one"), say("two")]);
    let r = Run::new(&m.url());
    let k = keeper(&r);
    module(&r, "watch", &[("session_start", &k), ("turn_start", &k), ("session_end", &k)]);
    let p = Pty::spawn(&r, &[], 24, 80);
    p.prompt(W);
    p.line("/exit");
    p.wait(W).finish();
    assert!(kept(&r).is_empty(), "events without a message");
    let x = module(&r, "broken", &[]);
    symlink("turn_end", x.join("on/question")).unwrap();
    fs::write(x.join("on/turn_end"), "#!/nonexistent/sh\n").unwrap();
    fs::set_permissions(x.join("on/turn_end"), fs::Permissions::from_mode(0o755)).unwrap();
    let p = Pty::spawn(&r, &[], 24, 80);
    p.prompt(W);
    p.line("first");
    p.expect("one", W);
    p.prompt(W);
    p.line("second");
    p.expect("two", W);
    p.prompt(W);
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    let names: Vec<_> = kept(&r).iter().map(|g| g.0["event"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["session_start", "turn_start", "turn_start", "session_end"]);
    let failed = d.notices().iter().filter(|n| n.starts_with("observer broken/on/turn_end did not start: ")).count();
    assert_eq!(failed, 2, "{}", d.diag());
}

/// O6: the official checkpoints mod; before each of 20 write and edit calls, the snapshot holds the files as
/// they were, and the work tree, the index and HEAD are not touched by it.
#[test]
fn observe_checkpoints_snapshot_before_each_edit() {
    let mut steps = Vec::new();
    let mut state: Vec<Option<String>> = vec![None; 3];
    let mut before = Vec::new();
    for i in 0..20 {
        let k = i % 3;
        before.push(state.clone());
        let path = format!("f{k}.txt");
        match &state[k] {
            Some(cur) if i % 2 == 1 => {
                steps.push(call("edit", json!({"path": path, "old_text": cur.trim(), "new_text": format!("e{i}")})));
                state[k] = Some(format!("e{i}\n"));
            }
            _ => {
                steps.push(call("write", json!({"path": path, "content": format!("c{i}\n")})));
                state[k] = Some(format!("c{i}\n"));
            }
        }
    }
    steps.push(say("done"));
    let m = Mock::start(steps);
    let r = Run::new(&m.url());
    copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("mods/checkpoints"), &r.config_dir().join("ext/checkpoints"));
    let d = r.json("edit");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.results().iter().all(|r| r["is_error"] == false), "{}", d.diag());
    assert!(d.notices().is_empty(), "{:?}", d.notices());
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&r.cwd).output().unwrap();
    for (i, files) in before.iter().enumerate() {
        for (k, want) in files.iter().enumerate() {
            let o = git(&["show", &format!("refs/henki/checkpoints/{}/{}:f{k}.txt", d.id(), i + 1)]);
            let got = o.status.success().then(|| String::from_utf8(o.stdout).unwrap());
            assert_eq!(&got, want, "snapshot {} of f{k}.txt", i + 1);
        }
    }
    assert!(!git(&["rev-parse", "-q", "--verify", "HEAD"]).status.success(), "HEAD moved");
    assert!(String::from_utf8(git(&["ls-files"]).stdout).unwrap().is_empty(), "the index changed");
}

/// `src` copied to `dst` as it is: files with their modes, links as links.
fn copy(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap().flatten() {
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let t = e.file_type().unwrap();
        if t.is_symlink() {
            symlink(fs::read_link(&from).unwrap(), &to).unwrap();
        } else if t.is_dir() {
            copy(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
            let mode = fs::metadata(&from).unwrap().permissions().mode() & 0o755;
            fs::set_permissions(&to, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}
