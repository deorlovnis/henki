#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Context sources (ext/2 `ctx/SOURCE`): run once at each start as a tool call runs, one after another within 2 s;
//! the texts that count go into the system prompt after the global AGENTS.md and before the extensions item; each is a
//! notice and, when it counts, an entry of `session.sources`; one that fails, runs long or prints too much or what a
//! terminal does not show is skipped, never cut; and nothing a source starts outlives the session.
use crate::harness::*;
use crate::mock::*;
use crate::pty::Pty;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const W: Duration = Duration::from_secs(15);

/// The module `name` in the default ext dir with the context sources `ctx` (file name, sh script body).
fn module(r: &Run, name: &str, ctx: &[(&str, &str)]) -> PathBuf {
    let x = r.config_dir().join("ext").join(name);
    fs::create_dir_all(x.join("ctx")).unwrap();
    fs::write(x.join("doc.md"), format!("{name}: context for the model\n")).unwrap();
    for (file, body) in ctx {
        fs::write(x.join("ctx").join(file), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(x.join("ctx").join(file), fs::Permissions::from_mode(0o755)).unwrap();
    }
    x
}

/// FNV-1a 64, as 16 lowercase hex digits (journal/1's hashes).
fn fnv(b: &[u8]) -> String {
    format!("{:016x}", b.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &c| (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3)))
}

fn session(d: &Done) -> Value {
    d.of("session").first().cloned().unwrap_or_else(|| panic!("no session record\n{}", d.diag()))
}

/// The texts in discovery order after AGENTS.md and before the extensions item, with nothing added; trailing
/// newlines trimmed; the tool environment without a session id, HENKI_CONFIG, the session cwd, stdin /dev/null; a
/// notice each and `session.sources`; a module of sources only adds no doc line.
#[test]
fn ctx_sources_in_the_system_prompt() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    fs::create_dir_all(r.config_dir()).unwrap();
    fs::write(r.config_dir().join("AGENTS.md"), "Be brief.\n").unwrap();
    let env = r#"printf 'config=%s session=[%s] cwd=%s stdin=[%s]\n\n\n' "$HENKI_CONFIG" "$HENKI_SESSION" "$(pwd -P)" "$(cat)""#;
    module(&r, "notes", &[("b", "printf 'second\\n'"), ("a.env", env)]);
    let x = r.config_dir().join("ext/tools");
    fs::create_dir_all(x.join("bin")).unwrap();
    fs::write(x.join("doc.md"), "tools: a CLI\n").unwrap();
    fs::write(x.join("bin/xtool"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(x.join("bin/xtool"), fs::Permissions::from_mode(0o755)).unwrap();
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let a = format!("config={} session=[] cwd={} stdin=[]", r.config_dir().display(), r.root());
    let exts = "Extensions are commands on PATH; read the doc in brackets before first use.\n- tools: tools: a CLI [~/.config/henki/ext/tools/doc.md]";
    let system = m.log()[0].system();
    assert!(system.ends_with(&format!("\n\nBe brief.\n\n{a}\n\nsecond\n\n{exts}")), "{system}");
    let n = d.notices();
    assert!(n.contains(&format!("context notes/a.env: {} bytes", a.len())) && n.contains(&"context notes/b: 6 bytes".to_string()), "{n:?}");
    let want = json!([{"bytes": a.len(), "hash": fnv(a.as_bytes()), "name": "notes/a.env"}, {"bytes": 6, "hash": fnv(b"second"), "name": "notes/b"}]);
    assert_eq!(session(&d)["sources"], want);
}

/// No module with sources: no notice, no `sources` field, the prompt as before.
#[test]
fn ctx_none_no_field() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("hi");
    d.finish();
    assert!(session(&d).get("sources").is_none() && !d.notices().iter().any(|n| n.starts_with("context ")), "{}", d.diag());
}

/// A source that prints more than 16 KiB, a code point henki escapes (U+202E, CR), or exits non-zero is
/// skipped with a notice that says why; the others still count. Invalid UTF-8 becomes U+FFFD and counts.
#[test]
fn ctx_skip_never_cut() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    let sources = [
        ("big", "head -c 20000 /dev/zero | tr '\\0' x"),
        ("bidi", "printf 'a \\342\\200\\256 b'"),
        ("cr", "printf 'line\\r\\n'"),
        ("fail", "echo partial; exit 1"),
        ("full", "head -c 16384 /dev/zero | tr '\\0' y"),
        ("latin1", "printf 'caf\\351'"),
    ];
    module(&r, "kit", &sources);
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let n = d.notices();
    let rule = "a source exits 0 with at most 16,384 bytes a terminal shows";
    for (name, why) in [("big", "exit 0, "), ("bidi", "exit 0, "), ("cr", "exit 0, "), ("fail", "exit 1, ")] {
        assert!(n.iter().any(|l| l.starts_with(&format!("context kit/{name}: skipped: {why}")) && l.ends_with(rule)), "{name}: {n:?}");
    }
    assert!(n.iter().any(|l| l.contains("kit/big: skipped") && l.contains("20,000 bytes")), "{n:?}");
    assert!(n.contains(&"context kit/full: 16,384 bytes".to_string()) && n.contains(&"context kit/latin1: 6 bytes".to_string()), "{n:?}");
    let system = m.log()[0].system();
    assert!(
        system.contains(&format!("\n\n{}\n\ncaf\u{fffd}", "y".repeat(16384))) && !system.contains("partial") && !system.contains("xxxx"),
        "{}",
        &system[system.len().saturating_sub(300)..]
    );
    let s = session(&d);
    let names: Vec<&str> = s["sources"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["kit/full", "kit/latin1"]);
}

/// A source still running at 2 s is killed (its whole group) and skipped; the start goes on, about 2 s late; a
/// source after it shares the same 2 s.
#[test]
fn ctx_slow_source_killed() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("a", &format!("echo early; sleep {}; :", r.marker)), ("b", "echo late")]);
    let t0 = Instant::now();
    let d = r.json("hi");
    let took = t0.elapsed();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(took >= Duration::from_secs(2) && took < Duration::from_millis(4500), "{took:?}");
    let n = d.notices();
    assert!(n.contains(&"context kit/a: skipped: still running after 2 s; stopped".to_string()), "{n:?}");
    assert!(n.iter().any(|l| l.starts_with("context kit/b: ")), "{n:?}");
    assert!(!m.log()[0].system().contains("early"));
    assert!(marked(&r.marker).is_empty(), "the source's sleep survived");
}

/// A source that leaves a process behind: the process is a background process of the session (always is not
/// offered while it runs), killed at the session end. A clean source leaves always offered.
#[test]
fn ctx_leftover_swept() {
    let m = Mock::start(vec![shell("echo hi"), say("ok")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("a", &format!("sleep {} >/dev/null 2>&1 &\necho started", r.marker))]);
    let d = r.json("hi");
    d.finish(); // no process with the marker survives
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.notices().iter().any(|n| n.contains("background processes")), "{}", d.diag());
}

#[test]
fn ctx_always_offered_after_clean_source() {
    for (body, offered) in [("echo clean", true), ("sleep 30 >/dev/null 2>&1 &\necho dirty", false)] {
        let m = Mock::start(vec![shell("echo hi"), say("ran")]);
        let mut r = Run::new(&m.url());
        r.set("approve", "\"ask\"");
        let body = body.replace("30", &r.marker);
        module(&r, "kit", &[("a", &body)]);
        let p = Pty::spawn(&r, &[], 24, 80);
        p.prompt(W);
        p.line("go");
        p.expect(if offered { "run? [y/N/always] " } else { "always is off until they end" }, W);
        if !offered {
            p.expect("run? [y/N] ", W);
        }
        p.line("y");
        p.expect("ran", W);
        p.prompt(W);
        p.line("/exit");
        let d = p.wait(W);
        d.finish();
        assert_eq!(d.code, Some(0), "{}", d.diag());
    }
}

/// Every ctx/ entry is a trust file: a change during the session ends it `changed`.
#[test]
fn ctx_entry_is_trusted() {
    let mut r = Run::new("");
    let x = module(&r, "kit", &[("a", "echo text")]);
    let src = x.join("ctx/a");
    let m = Mock::start(vec![shell(&format!("echo 'echo other' >> {}", src.display())), say("never")]);
    r.at(&m.url());
    let d = r.json("change the source");
    d.finish();
    let says = format!("trust file changed: {}", src.display());
    assert_eq!((d.code, d.result()["error"]["message"].as_str()), (Some(2), Some(says.as_str())), "{}", d.diag());
}

/// Ctrl-C while a source runs ends the start at once: exit 130, the source's group killed, no session.
#[test]
fn ctx_interrupt_during_source() {
    let m = Mock::start(vec![say("never")]);
    let r = Run::new(&m.url());
    let flag = r.dir.join("source-started");
    module(&r, "kit", &[("a", &format!("touch {}; sleep {}; :", flag.display(), r.marker))]);
    let mut l = r.cmd(&["--json", "hi"]).spawn();
    l.until("the source", W, |_| flag.exists());
    l.ready();
    let t0 = Instant::now();
    l.signal(libc::SIGINT);
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert!(d.exited - t0 < Duration::from_millis(500), "{:?}", d.exited - t0);
    assert!(d.journal.is_none() && d.result()["session"].is_null() && m.log().is_empty(), "{}", d.diag());
}

/// A source is the user's code and runs on the host: never through a bound exec program.
#[test]
fn ctx_not_through_exec() {
    let m = Mock::start(vec![shell("echo call"), say("ok")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("a", "echo from-the-source")]);
    let log = r.dir.join("toy.log");
    let w = r.dir.join("bindings/exec");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let toys = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/exec");
    fs::write(&w, format!("#!/bin/sh\nHENKI_TOY_LOG={} exec sh {toys}/toy-exec \"$@\"\n", log.display())).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(r.config_dir().join("port")).unwrap();
    fs::set_permissions(r.config_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&w, r.config_dir().join("port/exec")).unwrap();
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(calls.lines().count() == 1 && calls.contains("echo call") && !calls.contains("ctx"), "{calls}");
    assert!(m.log()[0].system().contains("\n\nfrom-the-source"));
}

/// Sources run at every start: a resume runs them again and its `session` record lists them as they are now.
#[test]
fn ctx_rerun_on_resume() {
    let m = Mock::start(vec![say("one"), say("two")]);
    let r = Run::new(&m.url());
    let x = module(&r, "kit", &[("a", "echo first")]);
    let d = r.json("hi");
    d.finish();
    fs::write(x.join("ctx/a"), "#!/bin/sh\necho second\n").unwrap();
    let d2 = r.cmd(&["--json", &format!("--resume={}", d.id()), "again"]).go();
    d2.finish();
    assert_eq!(d2.code, Some(0), "{}", d2.diag());
    assert!(m.log()[1].system().ends_with("\n\nsecond"), "{}", m.log()[1].system());
    assert_eq!(session(&d2)["sources"][0]["hash"], fnv(b"second"));
}

/// The official `project` mod through henki: the project's AGENTS.md with CRLF ends, NBSP and a byte order
/// mark counts (henki skips nothing), under its header and before the git line, after the global AGENTS.md.
#[test]
fn ctx_project_mod() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    crate::observe::copy(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mods/project"), &r.config_dir().join("ext/project"));
    fs::write(r.config_dir().join("AGENTS.md"), "Be brief.\n").unwrap();
    fs::write(r.cwd.join("AGENTS.md"), "\u{feff}Run the tests\u{a0}first.\r\nThen commit.\r\n").unwrap();
    let d = r.json("hi");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let root = r.root();
    let head = String::from_utf8(r.git(&["symbolic-ref", "--short", "-q", "HEAD"]).unwrap().stdout).unwrap();
    let text = format!("# project instructions ({root}/AGENTS.md)\n\nRun the tests first.\nThen commit.\n\ngit: {root} on {}", head.trim());
    let system = m.log()[0].system();
    assert!(system.ends_with(&format!("\n\nBe brief.\n\n{text}")), "{system}");
    assert!(d.notices().contains(&format!("context project/project: {} bytes", text.len())), "{:?}", d.notices());
}
