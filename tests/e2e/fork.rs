#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Forks: `--resume=ID@N` on std and on a history binding (with and without cap `fork`), what the model sees after
//! one, a refused fork that leaves nothing behind, and `resume ID@N` handed back by a command.
use crate::harness::*;
use crate::mock::*;
use crate::pty::Pty;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

const W: Duration = Duration::from_secs(15);

/// The 1-based line of the `k`th (from 0) `user` record of `all`.
fn user_line(all: &[Value], k: usize) -> usize {
    all.iter().enumerate().filter(|(_, x)| x["t"] == "user").nth(k).unwrap().0 + 1
}

/// The session id of a journal or store file.
fn stem(p: &Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

/// `CONFIG/port/history` linked to a wrapper running `cmd` (in 0700 dirs).
fn bind(r: &Run, cmd: &str) {
    let w = r.dir.join("bindings/history");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&w, format!("#!/bin/sh\n{cmd}\n")).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(r.config_dir().join("port")).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&w, r.config_dir().join("port/history")).unwrap();
}

/// Two runs of one session (a call, then a resume), then a fork at the second `user` record with a new task: the
/// notice, `session.parent`, the fork's lines 1 to N-1 byte for byte the parent's, the parent unchanged, and the model
/// sees the second request's history with the new task in place of the second one. `--resume=FORK` then goes on with it.
#[test]
fn e2e_fork_std() {
    let m = Mock::start(vec![shell("echo one"), say("first done"), say("second done"), say("third done"), say("fourth done")]);
    let r = Run::new(&m.url());
    for args in [&["--json", "first"][..], &["--json", "--resume", "second"]] {
        let d = r.cmd(args).go();
        d.finish();
        assert_eq!(d.code, Some(0), "{}", d.diag());
    }
    let parent = r.journals()[0].clone();
    let before = fs::read(&parent).unwrap();
    let (id, n) = (stem(&parent), user_line(&lines(&String::from_utf8_lossy(&before)), 1));
    let d = r.cmd(&["--json", &format!("--resume={id}@{n}"), "third"]).go();
    d.finish();
    assert_eq!((d.code, d.status()), (Some(0), "ok".into()), "{}", d.diag());
    let fork = d.journal.clone().unwrap();
    assert_ne!(fork, parent);
    assert_eq!(fork.parent(), parent.parent(), "the cwd's scope");
    assert_eq!(fs::read(&parent).unwrap(), before, "the parent changed");
    let notice = format!("forked from session {id} at line {n}");
    d.check(d.records().iter().any(|x| x["t"] == "notice" && x["text"] == notice.as_str()), "no fork notice");
    let s = &d.of("session")[0];
    assert_eq!((&s["parent"], s["resumed"].is_object()), (&json!({"id": id, "line": n}), true), "{}", d.diag());
    let head: usize = before.split_inclusive(|&b| b == b'\n').take(n - 1).map(<[u8]>::len).sum();
    let child = fs::read(&fork).unwrap();
    assert_eq!(child[..head], before[..head]);
    assert_eq!(lines(&String::from_utf8_lossy(&child[head..]))[0]["t"], "close");
    let log = m.log();
    let (second, third) = (log[2].messages(), log[3].messages());
    assert_eq!(second.len(), third.len());
    assert_eq!(second[..second.len() - 1], third[..third.len() - 1], "the history before the task");
    assert_eq!(third.last().unwrap()["content"].as_str(), Some("third"));
    let d = r.cmd(&["--json", &format!("--resume={}", stem(&fork)), "fourth"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&fork)), "{}", d.diag());
    assert_eq!(m.log()[4].contents("user").len(), 3, "first, third and fourth");
}

/// On a history binding: one without cap `fork` refuses `--resume=ID@N` with the fix and keeps one session; one with
/// it (port_toy --fork, the bash toy) makes the fork in its own store.
#[test]
fn e2e_fork_bound() {
    let toy = Path::new(env!("CARGO_BIN_EXE_henki")).parent().unwrap().join("examples/port_toy");
    let bash = format!("exec bash {}/tests/fixtures/ports/toy-history", env!("CARGO_MANIFEST_DIR"));
    for (cmd, forks) in [(format!("exec {} history --name", toy.display()), false), (format!("exec {} history --fork --name", toy.display()), true), (bash, true)] {
        let m = Mock::start(vec![say("one"), say("two")]);
        let r = Run::new(&m.url());
        bind(&r, &format!("{cmd} {}", r.marker));
        let d = r.json("first");
        d.finish();
        assert_eq!(d.code, Some(0), "{cmd}: {}", d.diag());
        let (id, parent) = (stem(d.journal.as_ref().unwrap()), d.journal_all());
        let n = user_line(&parent, 0);
        let d = r.cmd(&["--json", &format!("--resume={id}@{n}"), "again"]).go();
        d.finish();
        let store: Vec<PathBuf> =
            fs::read_dir(r.state().join("port/history")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
        if !forks {
            assert_eq!((d.code, d.status()), (Some(2), "error".into()), "{}", d.diag());
            let fix = format!("cannot fork (its hello lists no fork); fix: henki --resume={id} goes on with the whole session, or bind a history that can fork");
            d.check(d.records().last().unwrap()["error"]["message"].as_str().unwrap().contains(&fix), "no fix");
            assert_eq!((store.len(), m.log().len()), (1, 1), "{cmd}");
            continue;
        }
        assert_eq!(d.code, Some(0), "{cmd}: {}", d.diag());
        assert_eq!((store.len(), m.log().len()), (2, 2), "{cmd}");
        assert_eq!(d.of("session")[0]["parent"], json!({"id": id, "line": n}), "{cmd}");
        let kept = lines(&fs::read_to_string(d.journal.as_ref().unwrap()).unwrap());
        assert_eq!((&kept[..n - 1], &kept[n - 1]["t"], &kept[n]["t"]), (&parent[..n - 1], &json!("close"), &json!("session")), "{cmd}");
        assert_eq!(m.log()[1].contents("user").len(), 1, "{cmd}: the fork at the first task holds none before it");
    }
}

/// A fork that fails its checks (a session of another dialect, a line that is no user record) creates nothing and asks
/// the model nothing; a plain `--resume` then still opens the previous session.
#[test]
fn e2e_fork_refused() {
    let m = Mock::start_api(Api::Anthropic, vec![say("one"), say("two")]);
    let mut r = Run::new(&m.url());
    r.set("dialect", "\"anthropic\"");
    let d = r.json("first");
    d.finish();
    let (parent, n) = (d.journal.clone().unwrap(), user_line(&d.journal_all(), 0));
    let id = stem(&parent);
    for (at, dialect, want) in [
        (format!("{id}@{n}"), "openai", format!("{}: the session's dialect is anthropic, not openai", parent.display())),
        (format!("{id}@{}", n + 1), "anthropic", format!("--resume={id}@{}: line {} is not a user record", n + 1, n + 1)),
    ] {
        r.set("dialect", &format!("\"{dialect}\""));
        let d = r.cmd(&["--json", &format!("--resume={at}"), "again"]).go();
        d.finish();
        assert_eq!((d.code, d.records().last().unwrap()["error"]["message"].as_str()), (Some(2), Some(want.as_str())), "{}", d.diag());
        assert_eq!(r.journals(), std::slice::from_ref(&parent));
    }
    assert_eq!(m.log().len(), 1);
    let d = r.cmd(&["--json", "--resume", "again"]).go();
    d.finish();
    assert_eq!((d.code, d.journal.as_ref()), (Some(0), Some(&parent)), "{}", d.diag());
}

/// A command hands back `resume ID@N`: henki asks `fork session ID at line N?`, and on y starts again on the fork, whose
/// first request holds the parent's history before line N, then the new message.
#[test]
fn pty_fork_handoff() {
    let m = Mock::start(vec![say("one"), say("two"), say("forked")]);
    let r = Run::new(&m.url());
    for args in [&["--json", "first"][..], &["--json", "--resume", "second"]] {
        r.cmd(args).go().finish();
    }
    let parent = r.journals()[0].clone();
    let (id, n) = (stem(&parent), user_line(&lines(&fs::read_to_string(&parent).unwrap()), 1));
    let x = r.config_dir().join("ext/kit");
    fs::create_dir_all(x.join("cmd")).unwrap();
    fs::write(x.join("doc.md"), "kit: commands for the user\n").unwrap();
    fs::write(x.join("cmd/branch"), format!("#!/bin/sh\necho 'resume {id}@{n}' >\"$HENKI_NEXT\"\n")).unwrap();
    fs::set_permissions(x.join("cmd/branch"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut p = Pty::spawn(&r, &[], 24, 80);
    p.prompt(W);
    p.line("/branch");
    p.expect(&format!("fork session {id} at line {n}? [y/N] "), W);
    p.line("y");
    p.expect(&format!("forked from session {id} at line {n}"), W);
    p.prompt(W);
    p.renew(&r);
    p.line("instead");
    p.expect("forked", W);
    p.prompt(W);
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(m.log()[2].contents("user").len(), 2, "first, then instead");
    assert_eq!(r.journals().len(), 2, "the parent and the fork; the REPL that handed back left none");
}
