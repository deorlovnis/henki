#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! `henki serve` (front/1): a frontend's commands on stdin (`send`, `answer`, `command`, `cancel`), the json/1 records
//! on stdout plus `reject`, `command_output` and `next`, the questions, the reading rules and the ends.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const W: Duration = Duration::from_secs(15);

/// The kinds that are on serve's stdout only.
const STREAM: [&str; 5] = ["text", "reasoning", "reject", "command_output", "next"];

/// `henki serve ARGS` with stdin open.
fn serve(r: &Run, args: &[&str]) -> Live {
    r.cmd(args).serve().stdin(In::Open).spawn()
}

/// One command line.
fn put(l: &mut Live, v: Value) {
    l.write_stdin(format!("{v}\n").as_bytes());
}

/// The records of kind `t` on stdout so far.
fn of(l: &Live, t: &str) -> Vec<Value> {
    l.stdout_records().into_iter().filter(|r| r["t"] == t).collect()
}

/// Until stdout holds `n` records of kind `t`; the n-th.
fn nth(l: &mut Live, t: &str, n: usize) -> Value {
    l.until(&format!("{n} {t} records"), W, |l| of(l, t).len() >= n);
    of(l, t)[n - 1].clone()
}

/// EOF, the end, the postconditions; stdout's records other than the stream-only kinds are the journal's from `session`
/// on, without `close`.
fn end(mut l: Live, code: i32) -> Done {
    l.close_stdin();
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(code), "{}", d.diag());
    assert!(d.err().is_empty(), "serve wrote to stderr\n{}", d.diag());
    let shown: Vec<Value> = d.records().into_iter().filter(|r| !STREAM.contains(&r["t"].as_str().unwrap_or(""))).collect();
    let k = shown.iter().position(|r| r["t"] == "session").unwrap();
    let journaled: Vec<Value> = d.segment().into_iter().filter(|r| r["t"] != "close").collect();
    assert_eq!(shown[k..], journaled[..], "stdout is not the journal\n{}", d.diag());
    d
}

/// A module in the default ext dir with the user commands `cmds` (NAME, a sh script body).
fn module(r: &Run, m: &str, cmds: &[(&str, &str)]) {
    let x = r.config_dir().join("ext").join(m);
    fs::create_dir_all(x.join("cmd")).unwrap();
    fs::write(x.join("doc.md"), format!("{m}: commands for the user\n")).unwrap();
    for (name, body) in cmds {
        fs::write(x.join("cmd").join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(x.join("cmd").join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// F1, F6: a send runs a turn through `turn_end`; EOF while idle closes the session, exit 0.
#[test]
fn serve_send_and_eof() {
    let m = Mock::start(vec![say("hello there"), say("again")]);
    let r = Run::new(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "hi"}));
    assert_eq!(nth(&mut l, "turn_end", 1)["status"], "ok");
    put(&mut l, json!({"c": "send", "text": "more", "x-new": 1}));
    nth(&mut l, "turn_end", 2);
    let d = end(l, 0);
    assert_eq!(d.kinds().last().map(String::as_str), Some("close"), "{}", d.diag());
    assert_eq!(d.records().iter().filter(|r| r["t"] == "text").map(|r| r["delta"].as_str().unwrap()).collect::<String>(), "hello thereagain");
    assert_eq!(m.log()[1].contents("user").len(), 2, "the second request holds the first exchange");
}

/// F2, F16: a question is the `ask` record with `always` and `root`; yes runs the call; `always` saves the rule for
/// `ask.root` without a second question, and the same command then runs by the rule.
#[test]
fn serve_questions_yes_always() {
    let m = Mock::start(vec![shell("echo one"), shell("echo two"), shell("echo two"), say("done")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "run things"}));
    let a = nth(&mut l, "ask", 1);
    assert_eq!((&a["always"], a["root"].as_str()), (&json!(true), Some(r.root().as_str())), "{a}");
    put(&mut l, json!({"c": "answer", "id": a["id"], "verdict": "yes"}));
    let b = nth(&mut l, "ask", 2);
    put(&mut l, json!({"c": "answer", "id": b["id"], "verdict": "always"}));
    nth(&mut l, "turn_end", 1);
    let d = end(l, 0);
    let verdicts: Vec<Value> = d.of("approval").iter().map(|a| a["verdict"].clone()).collect();
    assert_eq!(verdicts, [json!("yes"), json!("always"), json!("rule")], "{}", d.diag());
    let rules = fs::read_to_string(r.allow()).unwrap();
    assert!(rules.lines().any(|l| l == format!("{}\techo two", a["root"].as_str().unwrap())), "{rules}");
    assert!(d.results().iter().all(|x| x["is_error"] == false) && d.results().len() == 3, "{}", d.diag());
}

/// F3: an answer for another call is a reject and a no; `always` where the `ask` record does not offer it is a reject,
/// and the question stays open.
#[test]
fn serve_questions_wrong_id_and_always_not_offered() {
    let m = Mock::start(vec![shell("echo one"), call("write", json!({"path": "f.txt", "content": "x"})), say("done")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "go"}));
    nth(&mut l, "ask", 1);
    put(&mut l, json!({"c": "answer", "id": "h-not-this", "verdict": "yes"}));
    nth(&mut l, "reject", 1);
    let w = nth(&mut l, "ask", 2);
    assert!(w.get("always").is_none() && w.get("root").is_none(), "{w}");
    put(&mut l, json!({"c": "answer", "id": w["id"], "verdict": "always"}));
    nth(&mut l, "reject", 2);
    put(&mut l, json!({"c": "answer", "id": w["id"], "verdict": "yes"}));
    nth(&mut l, "turn_end", 1);
    let d = end(l, 0);
    let verdicts: Vec<Value> = d.of("approval").iter().map(|a| a["verdict"].clone()).collect();
    assert_eq!(verdicts, [json!("no"), json!("yes")], "{}", d.diag());
    assert_eq!(fs::read_to_string(r.path("f.txt")).unwrap(), "x");
    assert!(!r.allow().exists(), "no rule saved");
}

/// F4, F5, F9: cancel while idle does nothing; a send during a turn is dropped without a reply; cancel during a call
/// ends the turn `interrupted` within a second and leaves nothing running.
#[test]
fn serve_cancel_and_busy() {
    let mut r = Run::new("");
    let m = Mock::start(vec![say("first"), shell(&r.sleep()), say("after")]);
    r.at(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "cancel"}));
    put(&mut l, json!({"c": "send", "text": "one"}));
    nth(&mut l, "turn_end", 1);
    put(&mut l, json!({"c": "send", "text": "sleep"}));
    l.wait_marked(W);
    put(&mut l, json!({"c": "send", "text": "while busy"}));
    std::thread::sleep(Duration::from_millis(200));
    let t = Instant::now();
    put(&mut l, json!({"c": "cancel"}));
    assert_eq!(nth(&mut l, "turn_end", 2)["status"], "interrupted");
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert!(of(&l, "reject").is_empty(), "{}", l.diag());
    put(&mut l, json!({"c": "send", "text": "go on"}));
    nth(&mut l, "turn_end", 3);
    let d = end(l, 0);
    let users: Vec<String> = d.of("user").iter().map(|u| u["msg"]["content"].as_str().unwrap().to_string()).collect();
    assert!(users.len() == 3 && !users.iter().any(|u| u.contains("while busy")), "{users:?}");
}

/// F7: EOF during a turn is a hangup: the turn is interrupted, the session closes, exit 130.
#[test]
fn serve_eof_mid_turn() {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&r.sleep())]);
    r.at(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "sleep"}));
    l.wait_marked(W);
    let t = Instant::now();
    let d = end(l, 130);
    assert!(d.exited - t < Duration::from_secs(2), "{:?}", d.exited - t);
    assert_eq!(d.of("turn_end")[0]["status"], "interrupted");
    assert_eq!(d.kinds().last().map(String::as_str), Some("close"));
}

/// F8, F13, F14, F15: lines that are not commands get a reject; a send of `/exit` or `!ls` is text for the model; a
/// line over 1 MiB is one reject, and the send after it runs; a send written before the `session` record is read after
/// it.
#[test]
fn serve_lines() {
    let m = Mock::start(vec![say("a"), say("b"), say("c")]);
    let r = Run::new(&m.url());
    let mut l = serve(&r, &[]);
    put(&mut l, json!({"c": "send", "text": "/exit"}));
    nth(&mut l, "turn_end", 1);
    let first = l.stdout_records();
    assert_eq!(first.iter().position(|x| x["t"] == "session"), Some(0), "{}", l.diag());
    l.write_stdin(b"not json\n");
    nth(&mut l, "reject", 1);
    put(&mut l, json!({"c": "send", "text": "   "}));
    nth(&mut l, "reject", 2);
    let mut big = vec![b'x'; 2 << 20];
    big.push(b'\n');
    l.write_stdin(&big);
    put(&mut l, json!({"c": "send", "text": "!ls"}));
    nth(&mut l, "turn_end", 2);
    assert_eq!(of(&l, "reject").len(), 3, "{}", l.diag());
    assert!(of(&l, "reject")[2]["why"].as_str().unwrap().contains("over 1 MiB"));
    put(&mut l, json!({"c": "answer", "id": "h1", "verdict": "yes"}));
    nth(&mut l, "reject", 4);
    let d = end(l, 0);
    let users: Vec<String> = m.log()[1].contents("user");
    assert!(users[0].ends_with("/exit") && users[1] == "!ls", "{users:?}");
    d.check(!d.records().iter().any(|x| x["t"] == "reject" && x.get("ts").is_some()), "a reject has no ts");
}

/// F10: `henki serve --resume` continues the session: a new segment, the first exchange in the next request.
#[test]
fn serve_resume() {
    let m = Mock::start(vec![say("first"), say("second")]);
    let r = Run::new(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "one"}));
    nth(&mut l, "turn_end", 1);
    let d = end(l, 0);
    let mut l = serve(&r, &["--resume"]);
    let s = nth(&mut l, "session", 1);
    assert!(s["resumed"].is_object(), "{s}");
    put(&mut l, json!({"c": "send", "text": "two"}));
    nth(&mut l, "turn_end", 1);
    let d2 = end(l, 0);
    assert_eq!(d.journal, d2.journal);
    assert_eq!(m.log()[1].contents("user").len(), 2);
    assert_eq!(d2.journal_all().iter().filter(|x| x["t"] == "session").count(), 2);
}

/// F11: after a trust file changed, every send gets a reject.
#[test]
fn serve_tripwire() {
    let mut r = Run::new("");
    let agents = r.config_dir().join("AGENTS.md");
    fs::create_dir_all(r.config_dir()).unwrap();
    fs::write(&agents, "Be brief.\n").unwrap();
    let m = Mock::start(vec![shell(&format!("echo more >> {}", agents.display())), say("never")]);
    r.at(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "change it"}));
    assert_eq!(nth(&mut l, "turn_end", 1)["status"], "error");
    assert!(of(&l, "notice").iter().any(|n| n["text"].as_str().unwrap().starts_with("trust file changed: ")), "{}", l.diag());
    // No reject comes unasked after the turn_end: the frontend pairs each with the line it sent.
    std::thread::sleep(Duration::from_millis(300));
    assert!(of(&l, "reject").is_empty(), "{}", l.diag());
    put(&mut l, json!({"c": "send", "text": "again"}));
    assert!(nth(&mut l, "reject", 1)["why"].as_str().unwrap().contains("restart henki"));
    let _ = end(l, 0);
    assert_eq!(m.log().len(), 1);
}

/// F12: stdout closed ends the session with 141.
#[test]
fn serve_stdout_closed() {
    let m = Mock::start(vec![say("into the void")]);
    let r = Run::new(&m.url());
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    l.close_stdout();
    put(&mut l, json!({"c": "send", "text": "hi"}));
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(141), "{}", d.diag());
}

/// F17: a start henki refuses writes one `result` with `session` null, as `run --json` does.
#[test]
fn serve_refused_start() {
    let r = Run::new("http://127.0.0.1:9/v1");
    let d = r.cmd(&["--approve", "never"]).serve().go();
    let recs = d.records();
    assert_eq!((recs.len(), &recs[0]["t"], &recs[0]["session"], d.code), (1, &json!("result"), &Value::Null, Some(2)), "{}", d.diag());
    assert!(d.err().is_empty(), "{}", d.diag());
    let d = r.cmd(&["--json"]).serve().go();
    assert_eq!((d.records().len(), d.code), (1, Some(2)), "{}", d.diag());
}

/// `command`: a module command runs with no terminal, its output and exit code the `command_output` record, never
/// journaled and never seen by the model; an unknown one is a reject; a `send` hand-off is a `next` record the frontend
/// sends itself; cancel stops a running command.
#[test]
fn serve_command() {
    let m = Mock::start(vec![say("sent")]);
    let r = Run::new(&m.url());
    let slow = format!("echo started; exec sleep {}", r.marker);
    module(
        &r,
        "kit",
        &[("hello", "echo \"hi $1\"; tty >/dev/null || echo no tty; echo oops >&2; exit 3"), ("hand", "printf 'send\\nfrom the command\\n' > \"$HENKI_NEXT\""), ("slow", &slow)],
    );
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "command", "name": "hello", "args": "there"}));
    let o = nth(&mut l, "command_output", 1);
    assert_eq!((o["text"].as_str(), &o["exit"]), (Some("hi there\nno tty\noops\n"), &json!(3)), "{o}");
    put(&mut l, json!({"c": "command", "name": "nope", "args": ""}));
    assert!(nth(&mut l, "reject", 1)["why"].as_str().unwrap().contains("unknown command /nope"));
    put(&mut l, json!({"c": "command", "name": "exit"}));
    assert_eq!(nth(&mut l, "reject", 2)["why"], "not taken now: command");
    // Not a command's name: never the typed line `/exit`, `/help` or `/kit ARGS`.
    for (k, name) in ["exit ", " help", "kit hello", "", "a/b"].iter().enumerate() {
        put(&mut l, json!({"c": "command", "name": name}));
        assert_eq!(nth(&mut l, "reject", 3 + k)["why"], "not taken now: command", "{name:?}");
    }
    put(&mut l, json!({"c": "command", "name": "hand"}));
    let n = nth(&mut l, "next", 1);
    assert_eq!(n, json!({"t": "next", "send": "from the command\n"}));
    put(&mut l, json!({"c": "send", "text": n["send"]}));
    nth(&mut l, "turn_end", 1);
    put(&mut l, json!({"c": "command", "name": "slow"}));
    l.wait_marked(W);
    put(&mut l, json!({"c": "cancel"}));
    let o = nth(&mut l, "command_output", 3);
    assert_eq!((o["text"].as_str(), &o["exit"]), (Some("started\n"), &Value::Null), "{o}");
    let d = end(l, 0);
    assert_eq!(m.log().len(), 1);
    assert_eq!(m.log()[0].contents("user").len(), 1);
    assert!(!d.journal_all().iter().any(|x| x["t"] == "command_output" || x["t"] == "next"), "{}", d.diag());
}

/// `command` hand-offs that start a new henki (`resume`, `restart`): a `next` record with the argv (`serve`, the flags,
/// `--resume=ID`), then henki closes the session and exits 0 without starting it.
#[test]
fn serve_command_next_argv() {
    let m = Mock::start(vec![say("one")]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("again", "echo restart > \"$HENKI_NEXT\"")]);
    let mut l = serve(&r, &["--approve", "ask"]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "hi"}));
    nth(&mut l, "turn_end", 1);
    put(&mut l, json!({"c": "command", "name": "again"}));
    let n = nth(&mut l, "next", 1);
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let argv: Vec<&str> = n["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert_eq!(argv[1..], ["serve", "--approve", "ask", &format!("--resume={}", d.segment()[0]["id"].as_str().unwrap())], "{n}");
    assert_eq!(d.records().last().unwrap()["t"], "next", "next is the last record");
}

/// A command that changes a trust file and hands back `restart`: each file is a `reject`, and the `next` record names
/// them in `changed`, for the frontend to show before it starts the argv (the REPL asks `restart on these changes?`).
#[test]
fn serve_command_changed_trust() {
    let m = Mock::start(vec![]);
    let r = Run::new(&m.url());
    module(&r, "kit", &[("evil", "echo 'approve = \"auto\"' >> \"$HOME/.config/henki/config.toml\"; echo restart > \"$HENKI_NEXT\"")]);
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "command", "name": "evil"}));
    let n = nth(&mut l, "next", 1);
    assert_eq!(n["changed"], json!(["~/.config/henki/config.toml"]), "{n}");
    assert!(of(&l, "reject").iter().any(|j| j["why"] == "changed while /evil ran: ~/.config/henki/config.toml"), "{}", l.diag());
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    // Nothing changed: `changed` is there, empty, and nothing is noted.
    let r = Run::new(&m.url());
    module(&r, "kit", &[("same", "echo restart > \"$HENKI_NEXT\"")]);
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "command", "name": "same"}));
    assert_eq!(nth(&mut l, "next", 1)["changed"], json!([]));
    assert!(of(&l, "reject").is_empty(), "{}", l.diag());
    l.wait().finish();
    // A change from before the command is named, and no verb starts a new henki.
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    let config = r.config_dir().join("config.toml");
    fs::write(&config, fs::read_to_string(&config).unwrap() + "# edited\n").unwrap();
    put(&mut l, json!({"c": "command", "name": "same"}));
    assert_eq!(nth(&mut l, "reject", 1)["why"], format!("trust file changed: {}", config.display()));
    assert!(nth(&mut l, "reject", 2)["why"].as_str().unwrap().contains("restart henki yourself"));
    assert!(of(&l, "next").is_empty(), "{}", l.diag());
    l.close_stdin();
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
}

/// EOF at a question is a hangup, as in the REPL: no `approval` (the person said nothing), the call is not run, the turn
/// is interrupted, exit 130.
#[test]
fn serve_eof_at_question() {
    let m = Mock::start(vec![shell("echo one"), say("never")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    let mut l = serve(&r, &[]);
    nth(&mut l, "session", 1);
    put(&mut l, json!({"c": "send", "text": "go"}));
    nth(&mut l, "ask", 1);
    let d = end(l, 130);
    assert!(d.of("approval").is_empty(), "{}", d.diag());
    let t = &d.of("tool_result")[0];
    assert_eq!((t["content"].as_str(), &d.of("turn_end")[0]["status"]), (Some("not run: interrupted"), &json!("interrupted")), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

/// EOF comes after the lines written before it: a `command` written just before EOF runs to its end (its output and a
/// file it writes after a while), then the session closes, exit 0.
#[test]
fn serve_command_then_eof() {
    let m = Mock::start(vec![]);
    let r = Run::new(&m.url());
    let mark = r.dir.join("mark");
    module(&r, "kit", &[("mark", &format!("sleep 0.5; echo done > {}; echo marked", mark.display()))]);
    let mut l = serve(&r, &[]);
    put(&mut l, json!({"c": "command", "name": "mark", "args": ""}));
    l.close_stdin();
    let d = l.wait();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let o: Vec<Value> = d.records().into_iter().filter(|x| x["t"] == "command_output").collect();
    assert_eq!(o, [json!({"t": "command_output", "text": "marked\n", "exit": 0})], "{}", d.diag());
    assert_eq!(fs::read_to_string(&mark).unwrap(), "done\n");
}

/// A turn that ends `error` says why in its `turn_end` (kind and message, as `run --json`'s result), so a frontend can
/// tell the person; the journal holds the same.
#[test]
fn serve_turn_end_error() {
    let m = Mock::start(vec![error(401, r#"{"error":{"message":"bad key"}}"#, 1), say("ok")]);
    let r = Run::new(&m.url());
    let mut l = serve(&r, &[]);
    put(&mut l, json!({"c": "send", "text": "a"}));
    let t = nth(&mut l, "turn_end", 1);
    assert_eq!((&t["status"], t["error"]["kind"].as_str(), t["error"]["message"].as_str().is_some_and(|m| m.contains("bad key"))), (&json!("error"), Some("auth"), true), "{t}");
    put(&mut l, json!({"c": "send", "text": "b"}));
    assert!(nth(&mut l, "turn_end", 2).get("error").is_none());
    let d = end(l, 0);
    assert_eq!(d.of("turn_end")[0]["error"], t["error"]);
}
