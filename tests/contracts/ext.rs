#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! ext/1: the layouts henki loads and the ones it refuses at start, the prompt bytes an extension adds, and what an
//! extension command sees when a call runs it.
use crate::harness::*;
use crate::mock::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const HEADER: &str = "Extensions are commands on PATH; read the doc in brackets before first use.";

/// `~/.config/henki/ext/`, made.
fn ext_dir(r: &Run) -> PathBuf {
    let d = r.config_dir().join("ext");
    fs::create_dir_all(&d).unwrap();
    d
}

/// A file at `rel` under `dir` with `text` and `mode`, its parent dirs made.
fn file(dir: &Path, rel: &str, text: &str, mode: u32) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
}

/// Everything ext/1 allows in one extension, next to a dot directory the ext path ignores: the run starts, the
/// prompt gets exactly the documented bytes, and the command runs first on PATH with the documented environment.
#[test]
fn contracts_ext_accepts() {
    let m = Mock::start(vec![shell("demo-cmd"), say("ok")]);
    let r = Run::new(&m.url());
    let x = ext_dir(&r);
    let demo = x.join("demo");
    file(&demo, "doc.md", "# demo: shows the layout\nThe body is read on demand.\n", 0o644);
    file(&demo, "bin/demo-cmd", "#!/bin/sh\necho \"demo-ran|$HENKI_USER_PATH|$HENKI_SESSION|$HENKI_VERSION|$HENKI_DEPTH\"\n", 0o755);
    file(&demo, "bin/.DS_Store", "", 0o644);
    for (rel, text) in
        [("README.md", "readme"), ("LICENSE", "license"), ("CHANGELOG.md", "changes"), (".henki/module.toml", "henki_module = 1\n"), (".henki/card.md", "card"), (".gitignore", "")]
    {
        file(&demo, rel, text, 0o644);
    }
    let long = "z".repeat(32);
    file(&x.join(&long), "doc.md", &"y".repeat(160), 0o644);
    file(&x, ".hidden/anything.txt", "ignored", 0o644);
    let d = r.json("use the extension");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(d.of("session")[0]["exts"], json!(["demo", long]));
    let item = format!("{HEADER}\n- demo: demo: shows the layout [~/.config/henki/ext/demo/doc.md]\n- {long}: {} [~/.config/henki/ext/{long}/doc.md]", "y".repeat(160));
    let system = m.log()[0].system();
    assert!(system.ends_with(&format!("\n\n{item}")), "the prompt bytes changed:\n{system}");
    let out = d.results()[0]["content"].as_str().unwrap().to_string();
    let want = format!("demo-ran|/usr/local/bin:/usr/bin:/bin|{}|{}|1", d.id(), env!("CARGO_PKG_VERSION"));
    assert!(out.starts_with(&want), "{out}");
}

/// Each layout ext/1 refuses stops henki at start with a config error naming the problem, before any request.
#[test]
fn contracts_ext_refuses() {
    type Setup = fn(&Path);
    let cases: Vec<(&str, Setup, &str)> = vec![
        (
            "a ctx/ directory",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                fs::create_dir_all(x.join("demo/ctx")).unwrap();
            },
            "ext/2 modules hold doc.md, bin/, cmd/, port/ and eval/",
        ),
        (
            "a stray file",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                file(&x.join("demo"), "notes.txt", "", 0o644);
            },
            "unknown entry",
        ),
        ("an upper-case name", |x| file(&x.join("Demo"), "doc.md", "a summary\n", 0o644), "a name is 1 to 32 of a-z, 0-9, _ and -"),
        ("a 33-byte name", |x| file(&x.join("a".repeat(33)), "doc.md", "a summary\n", 0o644), "a name is 1 to 32 of a-z, 0-9, _ and -"),
        ("a name starting with -", |x| file(&x.join("-demo"), "doc.md", "a summary\n", 0o644), "a name is 1 to 32 of a-z, 0-9, _ and -"),
        ("no doc.md", |x| file(&x.join("demo"), "README", "", 0o644), "doc.md: No such file or directory"),
        ("a 161-byte line 1", |x| file(&x.join("demo"), "doc.md", &"y".repeat(161), 0o644), "line 1 must be a summary of 1 to 160 bytes"),
        ("an empty line 1", |x| file(&x.join("demo"), "doc.md", "#\nthe summary is not here\n", 0o644), "line 1 must be a summary of 1 to 160 bytes"),
        ("an invisible character", |x| file(&x.join("demo"), "doc.md", "a\u{200b}summary\n", 0o644), "doc.md:1: holds U+200B, which a terminal does not show"),
        ("an invisible character below line 1", |x| file(&x.join("demo"), "doc.md", "a summary\n\n\u{202e}\n", 0o644), "doc.md:3: holds U+202E"),
        ("a doc.md over 16 KiB", |x| file(&x.join("demo"), "doc.md", &format!("a summary\n{}", "y".repeat(16 << 10)), 0o644), "over 16,384 bytes"),
        (
            "a link out of the module",
            |x| {
                file(x.parent().unwrap(), "elsewhere.md", "a summary\n", 0o644);
                fs::create_dir_all(x.join("demo")).unwrap();
                std::os::unix::fs::symlink("../../elsewhere.md", x.join("demo/doc.md")).unwrap();
            },
            "a link that leaves the module",
        ),
        (
            "a reserved command name",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                file(&x.join("demo"), "cmd/mods", "#!/bin/sh\n", 0o755);
            },
            "a name henki keeps for its own command",
        ),
        (
            "a user command in two modules",
            |x| {
                for n in ["one", "two"] {
                    file(&x.join(n), "doc.md", "a summary\n", 0o644);
                    file(&x.join(n), "cmd/go", "#!/bin/sh\n", 0o755);
                }
            },
            "ext two: cmd/go is also in ext one",
        ),
        (
            "a command that cannot run",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                file(&x.join("demo"), "bin/tool", "#!/bin/sh\n", 0o644);
            },
            "a command is a file you can run",
        ),
        (
            "a command name with a space",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                file(&x.join("demo"), "bin/my tool", "#!/bin/sh\n", 0o755);
            },
            "a command is a file you can run",
        ),
        ("a file as an extension", |x| file(x, "plain", "", 0o644), "make it an extension directory"),
        (
            "a command in two extensions",
            |x| {
                for n in ["one", "two"] {
                    file(&x.join(n), "doc.md", "a summary\n", 0o644);
                    file(&x.join(n), "bin/tool", "#!/bin/sh\n", 0o755);
                }
            },
            "ext two: bin/tool is also in ext one",
        ),
        (
            "a command that shadows one on PATH",
            |x| {
                file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
                file(&x.join("demo"), "bin/ls", "#!/bin/sh\n", 0o755);
            },
            "add shadow = [\"demo/ls\"] if that is intended",
        ),
    ];
    for (what, setup, says) in cases {
        let r = Run::new("http://127.0.0.1:9/v1");
        setup(&ext_dir(&r));
        let d = r.json("x");
        d.finish();
        let e = &d.result()["error"];
        assert_eq!((d.code, e["kind"].as_str(), d.journal.as_ref()), (Some(2), Some("config"), None), "{what}\n{}", d.diag());
        let message = e["message"].as_str().unwrap();
        // After the config file's name: `ext PATH: PROBLEM; fix: ...` (or `ext NAME: ...` for a rule across extensions).
        assert!(message.contains(": ext ") && message.contains(says) && message.contains("; fix: "), "{what}: {message}");
    }
}

/// `shadow` lets a command with the name of one on PATH load.
#[test]
fn contracts_ext_shadow() {
    let m = Mock::start(vec![say("ok")]);
    let mut r = Run::new(&m.url());
    r.set("shadow", "[\"demo/ls\"]");
    let x = ext_dir(&r);
    file(&x.join("demo"), "doc.md", "a summary\n", 0o644);
    file(&x.join("demo"), "bin/ls", "#!/bin/sh\n", 0o755);
    let d = r.json("x");
    d.finish();
    assert_eq!((d.code, &d.of("session")[0]["exts"]), (Some(0), &json!(["demo"])), "{}", d.diag());
}
