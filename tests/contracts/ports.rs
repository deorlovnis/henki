#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! port/1 and ext/2 read from CONTRACTS.md and held to what henki does: the six port names `--port NAME=std` takes, the
//! ports a link in `CONFIG/port/` and `henki port` bind, the command names a module may not take, and the cap on
//! observer files. The names and numbers come from the document, so a change to either side fails here.
use crate::doc::{section, text};
use crate::harness::*;
use crate::mock::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn ticked(s: &str) -> Vec<String> {
    s.split('`').skip(1).step_by(2).map(String::from).collect()
}

/// A section with its lines joined, so a phrase may span a line break.
fn flat(name: &str) -> String {
    section(&text(), name).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn file(p: &Path, text: &str, mode: u32) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

/// A start that henki refuses: exit 2, error config, and `says` in the message.
fn refused(d: &Done, says: &str, what: &str) {
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, e["kind"].as_str()), (Some(2), Some("config")), "{what}\n{}", d.diag());
    assert!(e["message"].as_str().unwrap().contains(says), "{what}: {}", e["message"]);
}

/// The six ports: `--port NAME=std` takes each (and no other name); only the ones port/1 says this henki binds may be
/// linked in `CONFIG/port/`, and of those only the ones that speak port/1 run through `henki port`.
#[test]
fn contracts_port_names() {
    let d = flat("port/1");
    let six = ticked(d.split(". Each").next().unwrap());
    let bound = ticked(d.split("This henki binds ").nth(1).unwrap().split(" that way").next().unwrap());
    let own = ticked(d.split("links say. ").nth(1).unwrap().split(" speaks no protocol").next().unwrap());
    assert_eq!((six.len(), bound.len(), own.as_slice()), (6, 3, &["exec".to_string()][..]), "{six:?} {bound:?} {own:?}");
    assert!(flat("exec/1").contains("`henki port` does not take `exec`"));
    for p in &six {
        let m = Mock::start(vec![say("ok")]);
        let d = Run::new(&m.url()).cmd(&["--json", "--port", &format!("{p}=std"), "x"]).go();
        d.finish();
        assert_eq!(d.code, Some(0), "--port {p}=std\n{}", d.diag());
    }
    refused(&Run::new("http://127.0.0.1:9/v1").cmd(&["--json", "--port", "tools=std", "x"]).go(), "--port takes PORT=std", "--port tools=std");
    for p in &six {
        // A binding that exits at once: a port/1 port henki binds fails at its hello, exec runs (no call starts it), any
        // other fails at the link rule.
        let m = Mock::start(vec![say("ok")]);
        let r = Run::new(&m.url());
        let prog = r.config_dir().join("bindings/exits");
        file(&prog, "#!/bin/sh\nexit 0\n", 0o700);
        fs::set_permissions(prog.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(r.config_dir().join("port")).unwrap();
        std::os::unix::fs::symlink(&prog, r.config_dir().join("port").join(p)).unwrap();
        let d = r.json("x");
        if own.contains(p) {
            d.finish();
            assert_eq!(d.code, Some(0), "a link for {p}\n{}", d.diag());
        } else {
            let says = if bound.contains(p) { format!("{p} binding") } else { "this henki cannot bind that port".into() };
            refused(&d, &says, &format!("a link for {p}"));
        }
        let out =
            Command::new(env!("CARGO_BIN_EXE_henki")).args(["port", p, "--", "/bin/true"]).env_clear().env("HOME", "/nonexistent").env("PATH", "/usr/bin:/bin").output().unwrap();
        let line = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(line.contains("henki port takes"), !bound.contains(p) || own.contains(p), "henki port {p}: {line}");
    }
}

/// ext/2: the command names henki keeps for itself, and the most observer files over all modules.
#[test]
fn contracts_ext2_names_and_observers() {
    let d = flat("ext/2");
    let kept = d.split("CMD as for bin/, not ").nth(1).unwrap().split(" NAME/").next().unwrap().replace(" or ", ", ");
    let kept: Vec<&str> = kept.split(", ").collect();
    assert_eq!(kept.len(), 4, "{kept:?}");
    for name in kept.iter().copied().chain(["review"]) {
        let m = Mock::start(vec![say("ok")]);
        let r = Run::new(&m.url());
        let x = r.config_dir().join("ext/demo");
        file(&x.join("doc.md"), "a summary\n", 0o644);
        file(&x.join("cmd").join(name), "#!/bin/sh\n", 0o755);
        let d = r.json("x");
        if name == "review" {
            d.finish();
            assert_eq!(d.code, Some(0), "cmd/review\n{}", d.diag());
        } else {
            refused(&d, "a name henki keeps for its own command", &format!("cmd/{name}"));
        }
    }
    let max: usize = d.split("At most ").nth(1).unwrap().split(" `on/` files over all modules").next().unwrap().parse().unwrap();
    // Every name on/ takes in one module, the rest in a second: max files load, one more stops the start.
    let all = ["session_start", "turn_start", "turn_start.wait", "call_start", "call_start.wait", "call_end", "question", "turn_end", "session_end", "record"];
    for n in [max, max + 1] {
        let m = Mock::start(vec![say("ok")]);
        let r = Run::new(&m.url());
        for (i, e) in all.iter().chain(all.iter()).take(n).enumerate() {
            let x = r.config_dir().join(if i < all.len() { "ext/one" } else { "ext/two" });
            file(&x.join("doc.md"), "a summary\n", 0o644);
            file(&x.join("on").join(e), "#!/bin/sh\nexit 0\n", 0o755);
        }
        let d = r.json("x");
        if n == max {
            d.finish();
            assert_eq!(d.code, Some(0), "{n} observers\n{}", d.diag());
        } else {
            refused(&d, &format!("more than {max} observers"), &format!("{n} observers"));
        }
    }
}

/// exec/1: the argv henki runs a shell call with (BASH by its absolute path, the first `bash` on the user PATH), and
/// the start notice, as the section gives them.
#[test]
fn contracts_exec_argv() {
    let sec = section(&text(), "exec/1");
    let shape: Vec<&str> = sec.lines().find(|l| l.trim_start().starts_with("PROGRAM BASH")).unwrap().split_whitespace().collect();
    let notice = flat("exec/1").split("a notice at start (`").nth(1).unwrap().split("`)").next().unwrap().to_string();
    let m = Mock::start(vec![shell("echo pinned"), say("ok")]);
    let r = Run::new(&m.url());
    let (prog, log) = (r.dir.join("bindings/exec"), r.dir.join("argv"));
    file(&prog, &format!("#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" >{}\nexec \"$@\"\n", log.display()), 0o700);
    fs::set_permissions(prog.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir_all(r.config_dir().join("port")).unwrap();
    std::os::unix::fs::symlink(&prog, r.config_dir().join("port/exec")).unwrap();
    let d = r.json("x");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let bash = ["/usr/local/bin/bash", "/usr/bin/bash", "/bin/bash"].into_iter().find(|b| Path::new(b).is_file()).unwrap();
    let p = prog.display().to_string();
    let want: Vec<&str> = shape
        .iter()
        .map(|w| match *w {
            "PROGRAM" => p.as_str(),
            "BASH" => bash,
            "COMMAND" => "echo pinned",
            w => w,
        })
        .collect();
    assert_eq!(fs::read_to_string(&log).unwrap().lines().collect::<Vec<_>>(), want);
    assert!(d.notices().contains(&notice.replace("NAME ID", &p)), "{notice}\n{}", d.diag());
}
