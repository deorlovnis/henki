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

/// HN6: ids a resume refuses (a path, a flag, a fork point that is no number), and a verb henki does not know: a notice, nothing else.
#[test]
fn handoff_resume_refused() {
    let m = Mock::start(vec![say("still here")]);
    let r = Run::new(&m.url());
    let cmds: Vec<(String, String)> = ["../x", "-x", "a@x", "a@", ""].iter().enumerate().map(|(i, id)| (format!("r{i}"), format!("echo 'resume {id}' >\"$HENKI_NEXT\""))).collect();
    let mut cmds: Vec<(&str, &str)> = cmds.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    cmds.push(("odd", "echo 'fork me' >\"$HENKI_NEXT\""));
    module(&r, "kit", &cmds);
    let p = start(&r, &[]);
    for (i, id) in ["../x", "-x", "a@x", "a@", ""].iter().enumerate() {
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

/// The official sessions, prompts and clip modules from this checkout, linked as the manager links them: only clip's
/// doc is in the prompt, /help lists the commands, /sessions lists this folder's sessions, /resume N switches, and a
/// prompt goes out with its argument once Enter says so.
#[test]
fn handoff_official_mods() {
    let m = Mock::start(vec![say("one"), say("two"), say("reviewed")]);
    let mut r = Run::new(&m.url());
    let ext = r.dir.join("ext");
    fs::create_dir_all(&ext).unwrap();
    for n in ["clip", "prompts", "sessions"] {
        std::os::unix::fs::symlink(Path::new(env!("CARGO_MANIFEST_DIR")).join("mods").join(n), ext.join(n)).unwrap();
    }
    r.set("ext", &format!("[\"{}\"]", ext.display()));
    // sessions reads them through `henki mods history`: the checkout's manager on PATH.
    r.env("PATH", &format!("{}/manager:/usr/local/bin:/usr/bin:/bin", env!("CARGO_MANIFEST_DIR")));
    let mut p = Pty::spawn(&r, &[], 24, 80);
    p.expect("ext clip: clip-get, clip-put\r\next prompts: no tools\r\next sessions: no tools\r\n", W);
    p.prompt(W);
    p.line("/help");
    p.expect("this terminal; the model is told you ran it", W);
    p.expect("\r\n/commit-msg  (prompts)\r\n/explain  (prompts)\r\n/fix-test  (prompts)\r\n/review  (prompts)\r\n/fork  (sessions)\r\n/new  (sessions)\r\n", W);
    p.prompt(W);
    message(&p, "first task", "one");
    p.line("/new");
    p.expect("approval: AUTO", W);
    p.prompt(W);
    message(&p, "second task", "two");
    p.line("/sessions");
    p.expect(" 2   ", W);
    p.expect("  first task\r\n", W);
    p.prompt(W);
    // Two sessions started in one second sort by their random part: the first is the one holding the first task.
    let first = r.journals().into_iter().find(|j| fs::read_to_string(j).unwrap().contains("first task")).unwrap();
    let first = first.file_stem().unwrap().to_string_lossy().into_owned();
    p.line("/resume 2");
    p.expect(&format!("switch to session {first}? [y/N] "), W);
    p.line("y");
    p.expect("approval: AUTO", W);
    p.prompt(W);
    p.renew(&r);
    p.line("/review the parser");
    p.expect("[from /review]\r\nReview the uncommitted changes", W);
    p.expect("\r\nthe parser\r\n", W);
    p.expect("send? [Y/n] ", W);
    p.line("y");
    p.expect("reviewed", W);
    p.prompt(W);
    exit(&r, p);
    let log = m.log();
    let (system, user) = (log[2].system(), log[2].contents("user"));
    assert!(system.contains("\n- clip: clipboard: clip-get prints it") && !system.contains("- prompts:") && !system.contains("- sessions:"), "{system}");
    assert_eq!(user.len(), 2, "the first session, resumed: {user:?}");
    assert!(user[1].starts_with("Review the uncommitted changes") && user[1].ends_with("\n\nthe parser\n"), "{user:?}");
}

/// An Agent Skill the manager imported (`henki-mods add --skill`) is a template: `/NAME TEXT` shows the skill's
/// instructions, TEXT after them, and sends them on a yes. It costs no prompt bytes, and its files stay data.
#[test]
fn handoff_skill_import() {
    let m = Mock::start(vec![say("checked")]);
    let r = Run::new(&m.url());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new("bash")
        .arg(root.join("manager/henki-mods"))
        .args(["add", "--skill"])
        .arg(root.join("manager/testdata/skills/plain-skill"))
        .arg("--yes")
        .env_clear()
        .env("HOME", &r.home)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("XDG_DATA_HOME", r.dir.join("xdg/data"))
        .env("TMPDIR", &r.dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let p = start(&r, &[]);
    p.line("/plain-skill focus on 0.3");
    p.expect("[from /plain-skill]\r\n(This skill's files are in ", W);
    p.expect("/.henki/skill; read them when the text names them.)\r\n# Changelog check\r\n", W);
    p.expect("3. Say which commits have no entry.\r\n\r\nfocus on 0.3\r\n", W);
    p.expect("send? [Y/n] ", W);
    p.line("y");
    p.expect("checked", W);
    p.prompt(W);
    exit(&r, p);
    let log = m.log();
    let (system, user) = (log[0].system(), log[0].contents("user"));
    assert!(!system.contains("plain-skill"), "{system}");
    assert!(user[0].contains("\n\n(This skill's files are in ") && user[0].contains("2. Read `references/STYLE.md`") && user[0].ends_with("\n\nfocus on 0.3\n"), "{user:?}");
}

/// A `restart` after the history link changed (as `/mods swap history ...` does) starts a new session on the new
/// binding: a resume there would be refused, since the session stays in the store it was kept in, which a note names.
#[test]
fn handoff_restart_after_history_swap() {
    let m = Mock::start(vec![say("one"), say("two")]);
    let r = Run::new(&m.url());
    let w = r.dir.join("bindings/history");
    fs::create_dir_all(w.parent().unwrap()).unwrap();
    fs::set_permissions(w.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let toy = Path::new(env!("CARGO_BIN_EXE_henki")).parent().unwrap().join("examples/port_toy");
    fs::write(&w, format!("#!/bin/sh\nexec {} history --name {}\n", toy.display(), r.marker)).unwrap();
    fs::set_permissions(&w, fs::Permissions::from_mode(0o700)).unwrap();
    let swap = format!("mkdir -p \"$HENKI_CONFIG/port\" && ln -s {} \"$HENKI_CONFIG/port/history\" && echo restart >\"$HENKI_NEXT\"", w.display());
    module(&r, "kit", &[("swap", &swap)]);
    let mut p = start(&r, &[]);
    message(&p, "first", "one");
    let first = r.journals()[0].file_stem().unwrap().to_string_lossy().into_owned();
    p.line("/swap");
    p.expect("henki: changed while /swap ran: ~/.config/henki/port/history\r\nrestart on these changes? [y/N] ", W);
    p.line("y");
    p.expect(&format!("henki: session {first} stays in henki's own history; resume it with that bound (std: henki --port history=std --resume={first})\r\n"), W);
    p.expect(&format!("henki: history: {} (port/1, {} 0.1.0)", w.display(), r.marker), W);
    p.prompt(W);
    p.renew(&r);
    message(&p, "second", "two");
    exit(&r, p);
    // The first session stays in std; the second is the only one in the binding's store.
    let text: Vec<String> = r.journals().iter().map(|j| fs::read_to_string(j).unwrap()).collect();
    let store = r.state().join("port/history");
    assert_eq!(r.journals().iter().map(|j| j.starts_with(&store)).collect::<Vec<_>>(), [true, false]);
    assert!(text[0].contains("second") && !text[0].contains("first") && text[1].contains("first") && !text[1].contains("second"), "{text:?}");
    assert_eq!(m.log()[1].contents("user").len(), 1, "the new session holds only the second message");
}
