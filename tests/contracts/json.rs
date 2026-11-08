#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! json/1 and the record tables of journal/1: scripted runs that between them write every documented kind and field,
//! each record held to its table in the journal and on `--json` stdout; the order of the stream; the result before a
//! session; the status and exit table.
use crate::doc::{self, Seen};
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};

/// A 503 that asks for no wait: one retry notice, at once.
fn busy() -> Step {
    Step::Error { status: 503, retry_after: Some("0".into()), body: api_error("overloaded_error", "busy"), times: 1 }
}

/// A server's context overflow: henki elides old results and sends again.
fn overflow() -> Step {
    error(400, &json!({"error": {"message": "the request exceeds the available context size", "type": "invalid_request_error"}}).to_string(), 1)
}

/// The kinds of `recs`, without the stream-only ones.
fn kinds(recs: &[Value]) -> Vec<String> {
    recs.iter().map(|r| r["t"].as_str().unwrap().to_string()).filter(|t| t != "text" && t != "reasoning").collect()
}

/// The `delta`s of the stream-only records of kind `t`, joined.
fn deltas(recs: &[Value], t: &str) -> String {
    recs.iter().filter(|r| r["t"] == t).map(|r| r["delta"].as_str().unwrap()).collect()
}

/// A run that retries, streams reasoning and text, makes a good call, a call whose arguments do not parse, and elides
/// after an overflow.
fn full_run() -> (Run, Mock, Done) {
    let think = Step::Stream { parts: vec![reason("thinking first"), shell("echo one")], finish: Some("tool_calls".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![busy(), think, native("shell", "{\"command\": ", "tool_calls"), shell("echo three"), overflow(), say("all done")]);
    let r = Run::new(&m.url());
    let d = r.json("work through it");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    (r, m, d)
}

#[test]
fn contracts_json_stream_order() {
    let (_r, _m, d) = full_run();
    let out = d.records();
    let want = [
        "session",
        "user",
        "notice",
        "assistant",
        "approval",
        "tool_result",
        "assistant",
        "tool_result",
        "assistant",
        "approval",
        "tool_result",
        "elide",
        "assistant",
        "turn_end",
        "result",
    ];
    assert_eq!(kinds(&out), want, "{}", d.diag());
    // The deltas stream before the reply they belong to is journaled, and add up to what the server sent.
    let first = out.iter().position(|r| r["t"] == "assistant").unwrap();
    assert!(out[..first].iter().any(|r| r["t"] == "reasoning"), "{}", d.diag());
    assert_eq!((deltas(&out[..first], "reasoning"), deltas(&out, "text")), ("thinking first".into(), "all done".into()));
    let last = out.iter().rposition(|r| r["t"] == "assistant").unwrap();
    assert_eq!(out[last]["msg"]["content"], "all done");
    assert!(out.iter().filter(|r| r["t"] == "text" || r["t"] == "reasoning").all(|r| r.get("ts").is_none()), "stream-only records carry no ts");
    // stdout is the journal from `session` on, without `close` (and the harness checked it record by record).
    let journal = d.segment();
    assert_eq!(kinds(&journal), [&want[..], &["close"]].concat(), "{}", d.diag());
    assert!(d.notices()[0].starts_with("HTTP 503; retry 1/4 in "), "{:?}", d.notices());
    let r = d.result();
    assert_eq!((&r["status"], &r["exit"], &r["text"], &r["steps"], r["session"].as_str()), (&json!("ok"), &json!(0), &json!("all done"), &json!(4), Some(d.id().as_str())));
    assert_eq!(d.journal.as_ref().unwrap().file_stem().unwrap().to_str(), Some(d.id().as_str()), "the session id is the file stem");
}

/// Every record of the runs below fits its table in CONTRACTS.md, and together they show every documented kind and
/// field.
#[test]
fn contracts_record_fields() {
    let kinds = doc::kinds();
    let mut seen = Seen::default();
    let mut runs: Vec<(&str, Done)> = Vec::new();
    runs.push(("full", full_run().2));
    // Ask mode in `run`: the write call needs a human, so the run ends `needs_approval` with the call.
    let m = Mock::start(vec![call("write", json!({"path": "out.txt", "content": "x"}))]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    runs.push(("ask", r.json("write a file")));
    // An allow rule lets the command run; one step is the limit, so the run gives up.
    let m = Mock::start(vec![shell("echo ruled")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r.write_allow(&format!("{}\techo ruled\n", r.root()));
    runs.push(("rule", r.cmd(&["--json", "--max-steps", "1", "run the ruled command"]).go()));
    // An auth error, then a resume of that session.
    let m = Mock::start(vec![error(401, &api_error("authentication_error", "bad key"), 1)]);
    let mut r = Run::new(&m.url());
    runs.push(("error", r.json("fail")));
    let m2 = Mock::start(vec![say("better")]);
    r.at(&m2.url());
    runs.push(("resume", r.cmd(&["--json", "--resume", "again"]).go()));
    // The anthropic dialect journals the reply's blocks.
    let m = Mock::start_api(Api::Anthropic, vec![say("blocks")]);
    let mut r = Run::new(&m.url());
    r.set("dialect", "\"anthropic\"");
    runs.push(("anthropic", r.json("hello")));
    // History bound to the bash binding of the conformance suite: `session.ports`; the records as henki sends them, so
    // without std's `close.bytes` and `resumed.from_bytes`.
    let m = Mock::start(vec![say("kept outside"), say("again")]);
    let r = Run::new(&m.url());
    std::fs::create_dir_all(r.config_dir().join("port")).unwrap();
    std::os::unix::fs::symlink(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ports/toy-history"), r.config_dir().join("port/history")).unwrap();
    runs.push(("bound", r.json("hello")));
    let d = r.cmd(&["--json", "--resume", "again"]).go();
    let all = d.journal_all();
    assert!(all.iter().filter(|x| x["t"] == "close").all(|x| x.get("bytes").is_none()), "{}", d.diag());
    assert_eq!(d.of("session")[0]["resumed"], json!({}), "{}", d.diag());
    runs.push(("bound resumed", d));
    // Policy bound to the bash binding of its suite, with rules: an allow (`by`), a deny (`reason`), `session.ports`.
    let m = Mock::start(vec![shell("echo allowed"), shell("rm -rf nothing"), say("decided")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    std::fs::create_dir_all(r.config_dir().join("port")).unwrap();
    std::os::unix::fs::symlink(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ports/toy-policy"), r.config_dir().join("port/policy")).unwrap();
    std::fs::create_dir_all(r.state().join("port/policy")).unwrap();
    std::fs::write(r.state().join("port/policy/rules"), "allow\t\"command\":\"echo allowed\"\ndeny\t\"command\":\"rm \n").unwrap();
    runs.push(("policy", r.json("decide")));
    // An end before a session: the result alone, `session` null, no `ts`.
    let r = Run::new("http://127.0.0.1:9/v1");
    let d = r.cmd(&["--json", "--max-steps", "0", "x"]).go();
    let only = d.records();
    assert_eq!(only.len(), 1, "{}", d.diag());
    assert_eq!((&only[0]["t"], &only[0]["session"], only[0].get("ts"), &only[0]["error"]["kind"]), (&json!("result"), &Value::Null, None, &json!("config")), "{}", d.diag());
    runs.push(("pre-session", d));
    for (name, d) in &runs {
        d.finish();
        assert!(d.journal.is_some() || *name == "pre-session", "{name}: no journal\n{}", d.diag());
        let want = match *name {
            "ask" => "needs_approval",
            "rule" => "gave_up",
            "error" | "pre-session" => "error",
            _ => "ok",
        };
        assert_eq!(d.status(), want, "{name}\n{}", d.diag());
        d.journal_all().iter().for_each(|x| seen.check(&kinds, x, &format!("{name} journal")));
        d.records().iter().for_each(|x| seen.check(&kinds, x, &format!("{name} stdout")));
    }
    seen.covers(&kinds);
    let (_, ask) = &runs[1];
    let call = &ask.result()["call"];
    assert_eq!((&call["name"], &call["args"]["path"]), (&json!("write"), &json!("out.txt")));
    assert_eq!(ask.of("ask")[0]["target"], "out.txt", "the target as shown: relative to the project");
    assert_eq!(runs[2].1.of("approval")[0]["verdict"], "rule");
    assert_eq!((&runs[2].1.result()["reason"], &runs[3].1.result()["error"]["kind"]), (&json!("max_steps"), &json!("auth")));
}

/// The status and exit table of json/1 is the one henki and the harness use.
#[test]
fn contracts_exit_table() {
    let s = doc::section(&doc::text(), "json/1").split_whitespace().collect::<Vec<_>>().join(" ");
    for status in ["ok", "gave_up", "error", "needs_approval", "budget", "interrupted", "output_closed"] {
        assert!(s.contains(&format!("`{status}` {}", code_of(status))), "json/1 does not give {status} exit {}", code_of(status));
    }
}
