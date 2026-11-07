#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! What henki sends: a byte-stable prefix, the task once (where and when the model works on the session's first task
//! only), the history as the journal holds it, and a small first request.
use crate::harness::*;
use crate::mock::*;

/// Every request of a session starts with the previous request's messages, byte for byte.
fn grows(log: &[Logged]) {
    for w in log.windows(2) {
        let (a, b) = (w[0].text(), w[1].text());
        let p = messages_prefix(a);
        assert!(b.starts_with(p) && b[p.len()..].starts_with(','), "request {} does not extend request {}:\n{a}\n{b}", w[1].n, w[0].n);
    }
}

#[test]
fn e2e_prefix_stable() {
    let mut r = Run::new("");
    let other = r.dir.join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let (mut firsts, mut tools) = (Vec::new(), Vec::new());
    for cwd in [r.cwd.clone(), r.cwd.clone(), other] {
        let m = Mock::start(vec![shell("echo step"), say("done")]);
        r.at(&m.url());
        let d = r.cmd(&["--json", "list the files"]).cwd(&cwd).go();
        d.finish();
        assert_eq!(d.code, Some(0), "{}", d.diag());
        let log = m.log();
        grows(&log);
        let first = log[0].text().to_string();
        let i = first.find("\"role\":\"system\"},").expect("system message first");
        firsts.push(first[..i].to_string());
        tools.push(log[0].json["tools"].to_string());
        let user = &log[0].contents("user")[0];
        assert!(user.starts_with(&format!("cwd: {}\nos: ", std::fs::canonicalize(&cwd).unwrap().display())), "{user}");
        assert!(!log[0].contents("system")[0].contains(&cwd.display().to_string()), "the cwd never enters the system prompt");
    }
    assert_eq!(r.journals().len(), 3, "three sessions");
    assert!(firsts.windows(2).all(|w| w[0] == w[1]), "the prefix changed between sessions: {firsts:?}");
    assert!(tools.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn e2e_request_user_once() {
    let m = Mock::start(vec![shell("echo one"), say("never")]);
    let mut r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-steps", "1", "first task"]).go();
    d.finish();
    assert_eq!(d.result()["reason"], "max_steps", "{}", d.diag());
    let m2 = Mock::start(vec![shell("echo two"), say("continued")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.of("user").is_empty(), "--resume without TASK adds no user message");
    let m3 = Mock::start(vec![say("second answer")]);
    r.at(&m3.url());
    let d = r.cmd(&["--json", "--resume", "second task"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    for (log, users) in [(m.log(), 1), (m2.log(), 1), (m3.log(), 2)] {
        for l in log {
            let u = l.contents("user");
            assert_eq!(u.len(), users, "{}", l.text());
            assert_eq!(u.iter().filter(|c| c.ends_with("first task")).count(), 1);
        }
    }
    assert_eq!(m3.log()[0].contents("user")[1], "second task", "a session's later tasks carry no cwd, os and date");
    grows(&[m.log(), m2.log(), m3.log()].concat());
}

#[test]
fn e2e_view_is_replay() {
    let hermes = say(&format!("Checking.\n{}", markup(&Form::Hermes, "shell", &serde_json::json!({"command": "echo text-call"}).to_string())));
    let m = Mock::start(vec![shell("echo native"), hermes, native("shell", "{\"command\": ", "tool_calls"), say("seen")]);
    let mut r = Run::new(&m.url());
    let d = r.json("look around");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let last = m.log().pop().unwrap();
    assert_eq!(last.messages()[1..], render(&before_last_answer(&d))[..], "the last request is not the journal's replay");
    let m2 = Mock::start(vec![say("after resume")]);
    r.at(&m2.url());
    let d = r.cmd(&["--json", "--resume", "and now"]).go();
    d.finish();
    assert_eq!(m2.log()[0].messages()[1..], render(&before_last_answer(&d))[..], "the request after resume is not the journal's replay");
    assert_eq!(m2.log()[0].messages()[0], last.messages()[0], "the system message is stable");
}

/// The journal's records before its last assistant record: the history the last request was built from.
fn before_last_answer(d: &Done) -> Vec<serde_json::Value> {
    let all = d.journal_all();
    let i = all.iter().rposition(|x| x["t"] == "assistant").unwrap();
    all[..i].to_vec()
}

/// The plain-answer request of a fresh session with the built-in tools only: its size against a gate in bytes.
fn first_request(r: &Run, m: &Mock, gate: usize) -> Result<(), String> {
    let d = r.json("Reply with the single word ready.");
    d.finish();
    let l = m.log().pop().unwrap();
    let (system, tools) = (l.contents("system")[0].len(), l.json["tools"].to_string().len());
    println!("first request {} bytes: system {system}, tools {tools}, user {}", l.bytes, l.contents("user")[0].len());
    if l.bytes <= gate { Ok(()) } else { Err(format!("the first request is {} bytes, over the {gate}-byte gate", l.bytes)) }
}

/// A fresh session whose global AGENTS.md is `text`.
fn with_agents_md(m: &Mock, text: &str) -> Run {
    let r = Run::new(&m.url());
    std::fs::create_dir_all(r.config_dir()).unwrap();
    std::fs::write(r.config_dir().join("AGENTS.md"), text).unwrap();
    r
}

#[test]
fn first_request_le_2048() {
    let m = Mock::start(vec![say("ready")]);
    first_request(&Run::new(&m.url()), &m, 2048).unwrap();
}

#[test]
fn first_request_gate_fires() {
    let m = Mock::start(vec![say("ready")]);
    let e = first_request(&with_agents_md(&m, &"Keep answers short.\n".repeat(80)), &m, 2048).unwrap_err();
    assert!(e.ends_with("bytes, over the 2048-byte gate"), "{e}");
}

/// The halti card (`profiles/halti-card.md`) as the global AGENTS.md, as the README sets it up: it goes into every
/// request, so the first one stays within 3,200 bytes.
#[test]
fn first_request_halti_card_le_3200() {
    let m = Mock::start(vec![say("ready")]);
    first_request(&with_agents_md(&m, include_str!("../../profiles/halti-card.md")), &m, 3200).unwrap();
}
