#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! One test per `result` status, the end records, and the plain-mode streams.
use crate::harness::*;
use crate::mock::*;
use serde_json::json;

#[test]
fn e2e_run_json_ok() {
    let m = Mock::start(vec![say("All good.")]);
    let r = Run::new(&m.url());
    let d = r.json("say all good");
    d.finish();
    assert_eq!(d.code, Some(0));
    let res = d.result();
    assert_eq!((res["status"].as_str(), res["text"].as_str(), res["steps"].as_u64()), (Some("ok"), Some("All good."), Some(1)), "{}", d.diag());
    assert!(res.get("reason").is_none() && res.get("error").is_none() && res.get("call").is_none(), "{res}");
    assert_eq!(res["session"], d.of("session")[0]["id"]);
    assert!(res["duration_s"].is_number() && res["usage"]["in"].as_u64() > Some(0), "{res}");
    assert_eq!(d.kinds(), ["session", "user", "assistant", "turn_end", "result", "close"]);
    let a = &d.of("assistant")[0];
    assert_eq!((a["finish"].as_str(), a["msg"]["tool_calls"].as_array().map(Vec::len)), (Some("stop"), Some(0)), "{a}");
}

#[test]
fn e2e_run_json_gave_up_max_steps() {
    let m = Mock::start(vec![shell("echo 1"), shell("echo 2"), shell("echo 3")]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-steps", "2", "count"]).go();
    d.finish();
    assert_eq!((d.code, d.status(), d.result()["reason"].as_str()), (Some(1), "gave_up".into(), Some("max_steps")), "{}", d.diag());
    assert_eq!(d.result()["steps"], 2);
    assert_eq!(m.log().len(), 2);
}

#[test]
fn e2e_run_json_gave_up_stuck() {
    let m = Mock::start(vec![shell("false"), shell("false"), shell("false"), say("never")]);
    let r = Run::new(&m.url());
    let d = r.json("fail three times");
    d.finish();
    assert_eq!((d.code, d.status(), d.result()["reason"].as_str()), (Some(1), "gave_up".into(), Some("stuck")), "{}", d.diag());
    assert_eq!(d.of("turn_end")[0]["reason"], "stuck");
}

#[test]
fn e2e_run_json_gave_up_empty_answer() {
    let m = Mock::start(vec![reason("I could look at the files first.")]);
    let r = Run::new(&m.url());
    let d = r.json("answer nothing");
    d.finish();
    assert_eq!((d.code, d.status(), d.result()["reason"].as_str()), (Some(1), "gave_up".into(), Some("empty_answer")), "{}", d.diag());
    assert_eq!(d.result()["text"], "");
}

#[test]
fn e2e_run_json_error_provider() {
    let m = Mock::start(vec![error(404, r#"{"error":{"message":"model mock-model not found","type":"not_found_error"}}"#, 1)]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, e["kind"].as_str(), e["message"].as_str()), (Some(2), Some("provider"), Some("model mock-model not found")), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "a 404 is not retried");
}

#[test]
fn e2e_run_json_error_auth() {
    let m = Mock::start(vec![error(401, r#"{"error":{"message":"invalid key","type":"authentication_error"}}"#, 1)]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("auth")), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

#[test]
fn e2e_run_json_error_context() {
    let m = Mock::start(vec![error(400, r#"{"error":{"message":"the request exceeds the available context size","type":"invalid_request_error"}}"#, 5)]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("context_exceeded")), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "nothing to elide: no second request");
}

#[test]
fn e2e_run_json_needs_approval() {
    let command = "printf 'line one\\n'\necho \"two\" > made.txt\n\tdone";
    let m = Mock::start(vec![shell(command), say("never")]);
    let mut r = Run::new(&m.url());
    r.unset("approve");
    let d = r.json("make a file");
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(3), "needs_approval"), "{}", d.diag());
    let ask = &d.of("ask")[0];
    assert_eq!(ask["args"]["command"].as_str(), Some(command), "byte-exact in ask");
    assert_eq!((ask["name"].as_str(), ask.get("target")), (Some("shell"), None));
    let call = &d.result()["call"];
    assert_eq!((call["args"]["command"].as_str(), call["name"].as_str()), (Some(command), Some("shell")), "byte-exact in result.call");
    assert_eq!(call["id"], ask["id"]);
    assert_eq!(d.results()[0]["content"], "not run: needs approval");
    assert!(!r.path("made.txt").exists(), "the call ran");
    assert_eq!(m.log().len(), 1);
}

#[test]
fn e2e_run_json_config_error() {
    let mut r = Run::new("http://127.0.0.1:9/v1");
    r.set("bogus_key", "1");
    let d = r.json("hello");
    d.finish();
    assert_eq!(d.code, Some(2));
    assert!(d.journal.is_none(), "no session for a config error");
    let out = d.records();
    assert_eq!(out.len(), 1, "{}", d.diag());
    let res = &out[0];
    assert_eq!((res["t"].as_str(), res["status"].as_str(), res["error"]["kind"].as_str()), (Some("result"), Some("error"), Some("config")));
    assert!(res["session"].is_null() && res["steps"] == 0 && res["text"] == "", "{res}");
    let msg = res["error"]["message"].as_str().unwrap();
    assert!(msg.contains("config.toml:5") && msg.contains("unknown key bogus_key"), "{msg}");
}

#[test]
fn e2e_run_json_stream_deltas() {
    let m = Mock::start(vec![say("one two three four")]);
    let r = Run::new(&m.url());
    let d = r.json("count to four");
    d.finish();
    let out = d.records();
    let a = out.iter().position(|x| x["t"] == "assistant").expect("assistant record");
    let deltas: Vec<usize> = (0..out.len()).filter(|&i| out[i]["t"] == "text").collect();
    assert!(deltas.len() >= 2 && deltas.iter().all(|&i| i < a), "text records before their assistant: {out:?}");
    let joined: String = deltas.iter().map(|&i| out[i]["delta"].as_str().unwrap()).collect();
    assert_eq!(joined, "one two three four");
    assert!(d.journal_all().iter().all(|x| x["t"] != "text"), "deltas are stdout only");
}

#[test]
fn e2e_run_end_records() {
    let m = Mock::start(vec![shell("echo hi"), say("done")]);
    let r = Run::new(&m.url());
    let d = r.json("say hi");
    d.finish();
    let kinds = d.kinds();
    assert_eq!(kinds[kinds.len() - 3..], ["turn_end", "result", "close"]);
    let out = d.records();
    assert_eq!(out.last().unwrap()["t"], "result");
    assert!(out.iter().all(|x| x["t"] != "close"), "close is journal only");
    assert_eq!(d.of("turn_end")[0]["status"], d.result()["status"]);
    assert_eq!(d.of("turn_end")[0]["steps"], 2);
    let close = &d.of("close")[0];
    let len = d.journal_bytes.len() as u64;
    assert!(close["bytes"].as_u64().unwrap() < len);
}

#[test]
fn e2e_run_plain_no_ansi() {
    let m = Mock::start(vec![shell("printf '\\033[31mred\\033[0m\\n'"), say("Done \u{1b}[1mbold\u{1b}[0m.")]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["show colours"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(!d.stdout.contains(&0x1b) && !d.stderr.contains(&0x1b), "no ANSI on any stream");
    assert_eq!(d.out(), "Done \\x1b[1mbold\\x1b[0m.\n");
}

#[test]
fn e2e_run_plain_outcome_line() {
    let m = Mock::start(vec![shell("false"), shell("false"), shell("false")]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["fail"]).go();
    d.finish();
    assert_eq!(d.code, Some(1));
    assert_eq!(d.out(), "", "no answer on stdout");
    assert!(d.err().lines().any(|l| l == "(gave up: stuck)"), "{}", d.err());
    let m = Mock::start(vec![error(401, r#"{"error":{"message":"bad key"}}"#, 1)]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["hello"]).go();
    d.finish();
    assert_eq!(d.code, Some(2));
    assert!(d.err().lines().any(|l| l == "(error auth: bad key)"), "{}", d.err());
    let m = Mock::start(vec![shell("echo x")]);
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    let d = r.cmd(&["hello"]).go();
    d.finish();
    assert_eq!(d.code, Some(3));
    assert!(d.err().lines().any(|l| l == "(needs approval: $ echo x; henki --resume continues this session in the REPL)"), "{}", d.err());
}

#[test]
fn e2e_run_plain_call_line() {
    let m = Mock::start(vec![shell("echo hi"), shell("true"), call("read", json!({"path": "notes.txt"})), say("It says hi.")]);
    let r = Run::new(&m.url());
    std::fs::write(r.path("notes.txt"), "hi\n").unwrap();
    let d = r.cmd(&["what does it say"]).go();
    d.finish();
    assert_eq!((d.code, d.out()), (Some(0), "It says hi.\n".into()), "{}", d.diag());
    let err = d.err();
    let shell_line = err.lines().find(|l| l.starts_with("$ echo hi -> exit 0, ")).unwrap_or_else(|| panic!("no call line:\n{err}"));
    let secs = shell_line.strip_prefix("$ echo hi -> exit 0, ").unwrap().strip_suffix(" s").unwrap();
    assert!(secs.split_once('.').is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.len() == 1), "{shell_line}");
    assert!(err.lines().any(|l| l.starts_with("read ") && l.contains("notes.txt -> ok, ") && !l.contains("no output")), "{err}");
    // A command that printed nothing says so, so a claim about its output can be checked.
    assert!(err.lines().any(|l| l.starts_with("$ true -> exit 0, ") && l.ends_with(" s, no output")), "{err}");
    assert!(!err.contains("henki: $"), "call lines carry no prefix");
}
