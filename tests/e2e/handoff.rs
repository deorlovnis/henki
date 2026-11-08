#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! handoff/1 over a pseudo-terminal: a module's `cmd/NAME` run as `/NAME`, its environment, and what it hands back in
//! `HENKI_NEXT` (nothing, `send`, `resume`, `new`, `restart`, or something henki refuses), under the tripwire's rules.
use crate::harness::*;
use crate::mock::*;
use crate::pty::Pty;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const W: Duration = Duration::from_secs(15);
const REFUSED: &str = "henki: a trust file changed; restart henki to go on (henki --resume continues this session)\r\n";
const BEFORE: &str = "henki: a trust file changed before; restart henki yourself (henki --resume continues this session)\r\n";

/// The module `m` in the default ext dir with the commands `cmds` (NAME, a sh script body), and doc `doc`.
fn module(r: &Run, m: &str, cmds: &[(&str, &str)]) -> PathBuf {
    let x = r.config_dir().join("ext").join(m);
    fs::create_dir_all(x.join("cmd")).unwrap();
    fs::write(x.join("doc.md"), format!("{m}: commands for the user\n")).unwrap();
    for (name, body) in cmds {
        fs::write(x.join("cmd").join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(x.join("cmd").join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
    x
}

/// The REPL with `args` at its first prompt.
fn start<'a>(r: &'a Run, args: &[&str]) -> Pty<'a> {
    let p = Pty::spawn(r, args, 24, 80);
    p.prompt(W);
    p
}

/// `/exit`, exit 0, the postconditions, and no hand-off file left behind.
fn exit(r: &Run, p: Pty<'_>) -> Done {
    p.line("/exit");
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let left: Vec<_> = fs::read_dir(r.state().join("spill")).map(|d| d.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    assert!(!left.iter().any(|n| n.to_string_lossy().starts_with(".next-")), "{left:?}");
    d
}

/// A message, then its answer and the prompt.
fn message(p: &Pty, text: &str, answer: &str) {
    p.line(text);
    p.expect(answer, W);
    p.prompt(W);
}

/// HN1: an empty or removed HENKI_NEXT hands back nothing; a failing command is noted; the session goes on.
#[test]
fn handoff_nothing() {
    let m = Mock::start(vec![say("still here")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("quiet", "echo ran quietly"), ("gone", "rm \"$HENKI_NEXT\""), ("fail", "exit 3")]);
    let p = start(&r, &[]);
    p.line("/quiet");
    p.expect("ran quietly\r\n", W);
    p.prompt(W);
    p.line("/gone");
    p.prompt(W);
    p.line("/fail");
    p.expect("henki: /fail exited 3\r\n", W);
    p.prompt(W);
    message(&p, "hello", "still here");
    let d = exit(&r, p);
    assert!(!d.out().contains("HENKI_NEXT is not"), "{}", d.out());
    assert_eq!(m.log()[0].contents("user").len(), 1, "commands never reach the model");
}

/// HN2, HN3: more than 64 KiB, not UTF-8, a FIFO, a link or another file in its place: one notice, nothing done, no hang.
#[test]
fn handoff_hostile_file_ignored() {
    let m = Mock::start(vec![say("still here")]);
    let r = Run::new(&m.url());
    let elsewhere = r.dir.join("elsewhere");
    fs::write(&elsewhere, "send\nfrom elsewhere\n").unwrap();
    let link = format!("rm \"$HENKI_NEXT\"; ln -s {} \"$HENKI_NEXT\"", elsewhere.display());
    let cmds = [
        ("big", "{ echo send; head -c 5242880 /dev/zero | tr '\\0' a; } >\"$HENKI_NEXT\""),
        ("binary", "printf 'send\\n\\377\\376' >\"$HENKI_NEXT\""),
        ("fifo", "rm \"$HENKI_NEXT\"; mkfifo \"$HENKI_NEXT\""),
        ("link", &link),
        ("swapped", "rm \"$HENKI_NEXT\"; printf 'send\\nswapped\\n' >\"$HENKI_NEXT\""),
    ];
    module(&r, "bad", &cmds);
    let p = start(&r, &[]);
    for (name, _) in cmds {
        p.line(&format!("/{name}"));
        p.expect(&format!("henki: /{name}: HENKI_NEXT is not henki's file, over 64 KiB or not UTF-8; ignored\r\n"), W);
        p.prompt(W);
    }
    message(&p, "hello", "still here");
    let d = exit(&r, p);
    assert!(!d.out().contains("send? ["), "{}", d.out());
    assert_eq!(m.log().len(), 1);
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "send\nfrom elsewhere\n", "a link's target is left alone");
}

/// HN4, HN8: `send` shows the whole text and asks; n, Ctrl-C and a paste drop it, Enter sends it as the next message
/// (with a pending `!cmd` note); invisible characters are refused.
#[test]
fn handoff_send() {
    let m = Mock::start(vec![say("got it")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("ask", "printf 'send\\nline one\\nline two: %s\\n' \"$1\" >\"$HENKI_NEXT\""), ("rlo", "printf 'send\\nok \\342\\200\\256 kO\\n' >\"$HENKI_NEXT\"")]);
    let p = start(&r, &[]);
    p.line("/rlo");
    p.expect("henki: /rlo handed back invisible characters; nothing is sent\r\n", W);
    p.prompt(W);
    p.line("/ask  the  rest ");
    p.expect("[from /ask]\r\nline one\r\nline two: the  rest\r\n", W);
    p.expect("send? [Y/n] ", W);
    p.line("n");
    p.prompt(W);
    p.line("/ask x");
    p.expect("send? [Y/n] ", W);
    p.send(b"y\ny\n");
    p.expect("henki: a paste is not an answer; taken as no\r\n", W);
    p.prompt(W);
    p.line("!echo noted");
    p.prompt(W);
    p.line("/ask");
    p.expect("send? [Y/n] ", W);
    p.line("");
    p.expect("got it", W);
    p.prompt(W);
    exit(&r, p);
    let log = m.log();
    assert_eq!(log.len(), 1, "only the last one went out");
    let user = log[0].contents("user");
    assert!(user[0].ends_with("[the user ran in their terminal: echo noted (exit 0)]\nline one\nline two: \n"), "{user:?}");
}

/// HN5, HN9, HN10: a trust file changed by the command: listed; `send` is refused; `restart` asks and n leaves the
/// session stopped; once stopped there is no one-key restart.
#[test]
fn handoff_trust_changes() {
    let m = Mock::start(vec![say("never")]);
    let r = Run::new(&m.url());
    let edit = "echo '# more' >>\"$HENKI_CONFIG/AGENTS.md\"";
    module(
        &r,
        "kit",
        &[
            ("say", &format!("{edit}; printf 'send\\nhi\\n' >\"$HENKI_NEXT\"")),
            ("again", &format!("{edit}; echo restart >\"$HENKI_NEXT\"")),
            ("plain", "echo restart >\"$HENKI_NEXT\""),
        ],
    );
    let p = start(&r, &[]);
    p.line("/say");
    p.expect("henki: changed while /say ran: ~/.config/henki/AGENTS.md\r\nhenki: /say: a trust file changed; nothing is sent\r\n", W);
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    p.line("/plain");
    p.expect(BEFORE, W);
    p.prompt(W);
    exit(&r, p);
    let p = start(&r, &[]);
    p.line("/again");
    p.expect("henki: changed while /again ran: ~/.config/henki/AGENTS.md\r\nrestart on these changes? [y/N] ", W);
    p.line("n");
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    p.line("/again");
    p.expect(BEFORE, W);
    p.prompt(W);
    let d = exit(&r, p);
    assert!(m.log().is_empty());
    assert_eq!(d.out().matches("[y/N]").count(), 1, "{}", d.out());
}

/// HN6: ids a resume refuses (a path, a flag, a fork), and a verb henki does not know: a notice, nothing else.
#[test]
fn handoff_resume_refused() {
    let m = Mock::start(vec![say("still here")]);
    let r = Run::new(&m.url());
    let cmds: Vec<(String, String)> = ["../x", "-x", "a@3", ""].iter().enumerate().map(|(i, id)| (format!("r{i}"), format!("echo 'resume {id}' >\"$HENKI_NEXT\""))).collect();
    let mut cmds: Vec<(&str, &str)> = cmds.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    cmds.push(("odd", "echo 'fork me' >\"$HENKI_NEXT\""));
    module(&r, "kit", &cmds);
    let p = start(&r, &[]);
    for (i, id) in ["../x", "-x", "a@3", ""].iter().enumerate() {
        p.line(&format!("/r{i}"));
        p.expect(&format!("henki: /r{i}: cannot resume {id:?}\r\n"), W);
        p.prompt(W);
    }
    p.line("/odd");
    p.expect("henki: /odd handed back \"fork\", which henki does not know\r\n", W);
    p.prompt(W);
    message(&p, "hello", "still here");
    let d = exit(&r, p);
    assert!(!d.out().contains("switch to session"), "{}", d.out());
}

/// HN7: the command's environment: the tool environment (no secrets, every module's bin/ first on PATH), the
/// HENKI_* names, REST as one argument (none when empty), the session's cwd; the model's calls never get HENKI_NEXT.
#[test]
fn handoff_environment() {
    let m = Mock::start(vec![shell("env | grep -c HENKI_NEXT; env | grep -c HENKI_MODULE; true"), say("done")]);
    let mut r = Run::new(&m.url());
    r.env("SECRET_TOKEN", "t");
    let x = module(&r, "kit", &[("env", "echo \"args $# [$1]\"; pwd; env | grep -E '^(HENKI_|SECRET|PATH=)' | sort")]);
    fs::create_dir_all(x.join("bin")).unwrap();
    fs::write(x.join("bin/kit-tool"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(x.join("bin/kit-tool"), fs::Permissions::from_mode(0o755)).unwrap();
    let p = start(&r, &[]);
    p.line("/env  a  b ");
    p.expect("args 1 [a  b]\r\n", W);
    p.prompt(W);
    p.line("/env");
    p.expect("args 0 []\r\n", W);
    p.prompt(W);
    message(&p, "go", "done");
    let d = exit(&r, p);
    let out = d.out();
    let state = r.state();
    for want in [
        format!("{}\n", r.cwd.display()),
        format!("HENKI_BIN={}\n", Path::new(&bin()).display()),
        format!("HENKI_CONFIG={}\n", r.config_dir().display()),
        "HENKI_DEPTH=1\n".into(),
        format!("HENKI_MODULE={}\n", x.display()),
        format!("HENKI_NEXT={}/spill/.next-", state.display()),
        format!("HENKI_STATE={}\n", state.display()),
        format!("PATH={}:/usr/local/bin:/usr/bin:/bin\n", x.join("bin").display()),
    ] {
        assert!(out.contains(&want), "{want:?} not in:\n{out}");
    }
    assert!(!out.contains("SECRET_TOKEN"), "{out}");
    assert!(m.log()[1].contents("tool")[0].starts_with("0\n0\n[exit 0"), "a call sees neither");
}

/// `new`, `restart` and `resume ID`: the session closes, and henki starts again in its place with its arguments
/// (minus `--resume`), a new session for `new`, the same one for `restart`, the asked one for `resume`.
#[test]
fn handoff_new_restart_resume() {
    let m = Mock::start(vec![say("one"), say("two"), say("three")]);
    let r = Run::new(&m.url());
    let show = "echo \"session=$HENKI_SESSION\"; ps -ww -o args= -p $PPID";
    let resume = "echo \"resume $(cat \"$HENKI_STATE/first\")\" >\"$HENKI_NEXT\"";
    let keep = "echo \"$HENKI_SESSION\" >\"$HENKI_STATE/first\"";
    module(&r, "kit", &[("show", show), ("new", "echo new >\"$HENKI_NEXT\""), ("again", "echo restart >\"$HENKI_NEXT\""), ("back", resume), ("keep", keep)]);
    let mut p = start(&r, &["--model", "mock-model", "--port", "history=std"]);
    p.line("/again");
    p.expect("approval: AUTO", W); // no message yet: a restart is a new henki without --resume
    p.prompt(W);
    message(&p, "first", "one");
    p.line("/keep");
    p.prompt(W);
    p.line("/again");
    p.expect("approval: AUTO", W);
    p.prompt(W);
    p.line("/show");
    let first = fs::read_to_string(r.state().join("first")).unwrap().trim().to_string();
    p.expect(&format!("session={first}\r\n"), W);
    p.expect(&format!("{} --model mock-model --port history=std --resume={first}\r\n", Path::new(&bin()).display()), W);
    p.prompt(W);
    p.line("/new");
    p.expect("approval: AUTO", W);
    p.prompt(W);
    message(&p, "second", "two");
    p.line("/back");
    p.expect(&format!("switch to session {first}? [y/N] "), W);
    p.line("y");
    p.expect("approval: AUTO", W);
    p.prompt(W);
    p.renew(&r);
    message(&p, "third", "three");
    exit(&r, p);
    let log = m.log();
    assert_eq!(log[2].contents("user").len(), 2, "the resumed session holds first and third");
    assert_eq!(r.journals().len(), 2, "first (resumed) and second");
}
