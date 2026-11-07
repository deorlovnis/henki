#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! wire/1: the request henki sends in each dialect (the head in order, the exact body fields, the message and tool
//! shapes), the responses it accepts (fixed SSE fixtures in tests/fixtures/contracts/, a JSON completion, text calls,
//! cut replies), and the error and retry rules.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;

const CUT: &str = "arguments cut at max_tokens; not run. Send a shorter call.";

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap_or_else(|| panic!("not an object: {v}")).keys().map(String::as_str).collect()
}

fn names(l: &Logged) -> Vec<&str> {
    l.headers.iter().map(|(k, _)| k.as_str()).collect()
}

fn ok(d: &Done) {
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
}

/// The `delta`s of the stdout records of kind `t`, joined.
fn deltas(d: &Done, t: &str) -> String {
    d.records().iter().filter(|r| r["t"] == t).map(|r| r["delta"].as_str().unwrap().to_string()).collect()
}

fn anthropic(m: &Mock) -> Run {
    let mut r = Run::new(&m.url());
    r.set("dialect", "\"anthropic\"");
    r
}

/// The head in order, the body compact with sorted keys and exactly its fields, the tools, and each message shape.
#[test]
fn contracts_wire_openai_request() {
    let m = Mock::start(vec![shell("echo one"), say("done")]);
    let mut r = Run::new(&m.url());
    r.set("api_key", "\"k-local\"");
    ok(&r.json("hello"));
    let log = m.log();
    let l = &log[0];
    assert_eq!(l.line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(names(l), ["Host", "User-Agent", "Content-Type", "Content-Length", "Accept", "Connection", "Authorization"]);
    let host = format!("127.0.0.1:{}", m.port());
    let agent = format!("henki/{}", env!("CARGO_PKG_VERSION"));
    let want = [
        ("Host", host.as_str()),
        ("User-Agent", &agent),
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
        ("Connection", "close"),
        ("Authorization", "Bearer k-local"),
    ];
    for (k, v) in want {
        assert_eq!(l.header(k), Some(v), "{k}");
    }
    assert_eq!(l.header("Content-Length"), Some(l.bytes.to_string().as_str()));
    assert_eq!(serde_json::to_string(&l.json).unwrap(), l.text(), "the body is not compact JSON with sorted keys");
    assert_eq!(keys(&l.json), ["chat_template_kwargs", "max_tokens", "messages", "model", "stream", "stream_options", "tools"]);
    let fixed = (&l.json["chat_template_kwargs"], &l.json["max_tokens"], &l.json["model"], &l.json["stream"], &l.json["stream_options"]);
    assert_eq!(fixed, (&json!({"enable_thinking": false}), &json!(4096), &json!("mock-model"), &json!(true), &json!({"include_usage": true})));
    let tools = l.json["tools"].as_array().unwrap();
    let args = [
        ("shell", vec!["command"], vec!["command", "timeout"]),
        ("read", vec!["path"], vec!["limit", "offset", "path"]),
        ("write", vec!["path", "content"], vec!["content", "path"]),
        ("edit", vec!["path", "old_text", "new_text"], vec!["new_text", "old_text", "path"]),
    ];
    assert_eq!(tools.len(), args.len());
    for (t, (name, required, all)) in tools.iter().zip(args) {
        assert_eq!((keys(t), &t["type"]), (vec!["function", "type"], &json!("function")));
        let f = &t["function"];
        assert_eq!((keys(f), &f["name"], &f["parameters"]["type"]), (vec!["description", "name", "parameters"], &json!(name), &json!("object")));
        assert_eq!((&f["parameters"]["required"], keys(&f["parameters"]["properties"])), (&json!(required), all), "{name}");
    }
    let msgs = l.messages();
    assert_eq!((keys(&msgs[0]), &msgs[0]["role"], keys(&msgs[1]), &msgs[1]["role"]), (vec!["content", "role"], &json!("system"), vec!["content", "role"], &json!("user")));
    let next = log[1].messages();
    let (a, t) = (&next[2], &next[3]);
    assert_eq!((keys(a), &a["role"]), (vec!["content", "role", "tool_calls"], &json!("assistant")));
    let c = &a["tool_calls"][0];
    assert_eq!(
        (keys(c), keys(&c["function"]), &c["type"], &c["function"]["name"]),
        (vec!["function", "id", "type"], vec!["arguments", "name"], &json!("function"), &json!("shell"))
    );
    let sent: Value = serde_json::from_str(c["function"]["arguments"].as_str().expect("arguments is JSON text")).unwrap();
    assert_eq!(sent, json!({"command": "echo one"}));
    assert_eq!((keys(t), &t["role"], &t["tool_call_id"]), (vec!["content", "role", "tool_call_id"], &json!("tool"), &c["id"]));
    assert!(log[1].text().starts_with(messages_prefix(l.text())), "the earlier messages are not a byte-identical prefix");
}

/// A stream with a comment, an empty event, reasoning, text, a call split over fragments (its name too), a call whose
/// arguments come as an object, and usage with the cache count: what henki journals from it.
#[test]
fn contracts_wire_openai_stream() {
    let m = Mock::start(vec![fixture("contracts/openai-stream"), say("done")]);
    let r = Run::new(&m.url());
    fs::write(r.path("a.txt"), "hello\n").unwrap();
    let d = r.json("look");
    ok(&d);
    assert_eq!(m.log()[0].header("Authorization"), None, "no key, no Authorization");
    let a = &d.of("assistant")[0];
    let calls = json!([{"args": {"command": "echo wire-ok"}, "id": "call_w1", "name": "shell"}, {"args": {"path": "a.txt"}, "id": "call_w2", "name": "read"}]);
    assert_eq!((&a["msg"]["content"], &a["msg"]["tool_calls"], &a["finish"]), (&json!("Looking"), &calls, &json!("tool_calls")));
    assert_eq!(a["usage"], json!({"cached": 64, "in": 100, "out": 7}));
    assert!(d.results()[0]["content"].as_str().unwrap().starts_with("wire-ok"), "{}", d.diag());
    assert_eq!((deltas(&d, "reasoning"), deltas(&d, "text")), ("Let me look.".into(), "Lookingdone".into()));
}

/// A whole JSON completion; and each text call form, taken out of the content.
#[test]
fn contracts_wire_openai_json_and_text_calls() {
    let whole =
        json!({"choices": [{"finish_reason": "stop", "index": 0, "message": {"content": "whole", "role": "assistant"}}], "usage": {"completion_tokens": 2, "prompt_tokens": 50}});
    let m = Mock::start(vec![Step::Json(whole)]);
    let d = Run::new(&m.url()).json("answer whole");
    ok(&d);
    let a = &d.of("assistant")[0];
    assert_eq!((&a["msg"]["content"], &a["finish"], &a["usage"]), (&json!("whole"), &json!("stop"), &json!({"cached": 0, "in": 50, "out": 2})));
    for form in [Form::Hermes, Form::QwenXml, Form::Fenced] {
        let m = Mock::start(vec![text_call(form.clone(), "shell", json!({"command": "echo text-call"})), say("ok")]);
        let d = Run::new(&m.url()).json("call in text");
        ok(&d);
        let a = &d.of("assistant")[0];
        assert_eq!(
            (&a["msg"]["tool_calls"][0]["name"], &a["msg"]["tool_calls"][0]["args"], &a["msg"]["content"]),
            (&json!("shell"), &json!({"command": "echo text-call"}), &json!("")),
            "{form:?}"
        );
        assert!(d.results()[0]["content"].as_str().unwrap().starts_with("text-call"), "{form:?}\n{}", d.diag());
    }
}

/// The anthropic head and body, and a tool round trip: the reply's blocks replayed as received, the result as a
/// tool_result block. The mock refuses what the API would refuse.
#[test]
fn contracts_wire_anthropic_request() {
    let m = Mock::start_api(Api::Anthropic, vec![shell("echo a"), say("done")]);
    let mut r = anthropic(&m);
    r.set("api_key", "\"k-local\"");
    let d = r.json("hello");
    ok(&d);
    assert!(m.rejections().is_empty(), "{:?}", m.rejections());
    let log = m.log();
    let l = &log[0];
    assert_eq!(l.line, "POST /v1/messages HTTP/1.1");
    assert_eq!(names(l), ["Host", "User-Agent", "Content-Type", "Content-Length", "Accept", "Connection", "anthropic-version", "x-api-key"]);
    assert_eq!((l.header("anthropic-version"), l.header("x-api-key")), (Some("2023-06-01"), Some("k-local")));
    assert_eq!(serde_json::to_string(&l.json).unwrap(), l.text(), "the body is not compact JSON with sorted keys");
    assert_eq!(keys(&l.json), ["cache_control", "max_tokens", "messages", "model", "stream", "system", "tools"]);
    let ephemeral = json!({"type": "ephemeral"});
    assert_eq!((&l.json["cache_control"], &l.json["max_tokens"], &l.json["stream"]), (&ephemeral, &json!(4096), &json!(true)));
    let system = &l.json["system"][0];
    assert_eq!(
        (l.json["system"].as_array().unwrap().len(), keys(system), &system["cache_control"], &system["type"]),
        (1, vec!["cache_control", "text", "type"], &ephemeral, &json!("text"))
    );
    for (t, name) in l.json["tools"].as_array().unwrap().iter().zip(["shell", "read", "write", "edit"]) {
        assert_eq!((keys(t), &t["name"], &t["input_schema"]["type"]), (vec!["description", "input_schema", "name"], &json!(name), &json!("object")));
    }
    assert_eq!(l.messages()[0], json!({"content": [{"text": l.messages()[0]["content"][0]["text"], "type": "text"}], "role": "user"}));
    let next = log[1].messages();
    assert_eq!(next.iter().map(|x| x["role"].as_str().unwrap()).collect::<Vec<_>>(), ["user", "assistant", "user"]);
    assert_eq!(next[1]["content"], d.of("assistant")[0]["blocks"], "the reply is replayed as received");
    let used = next[1]["content"].as_array().unwrap().iter().find(|b| b["type"] == "tool_use").unwrap();
    assert_eq!(keys(used), ["id", "input", "name", "type"]);
    let res = &next[2]["content"][0];
    assert_eq!(
        (keys(res), &res["type"], &res["tool_use_id"], &res["is_error"]),
        (vec!["content", "is_error", "tool_use_id", "type"], &json!("tool_result"), &used["id"], &json!(false))
    );
}

/// An anthropic stream with ping, thinking and its signature, text, and an event type henki does not know: the
/// journaled blocks, the usage sums, the deltas.
#[test]
fn contracts_wire_anthropic_stream() {
    let m = Mock::start_api(Api::Anthropic, vec![fixture("contracts/anthropic-stream")]);
    let d = anthropic(&m).json("greet");
    ok(&d);
    let a = &d.of("assistant")[0];
    let blocks = json!([{"signature": "sig-contracts", "thinking": "Think.", "type": "thinking"}, {"text": "Hello there.", "type": "text"}]);
    assert_eq!((&a["msg"]["content"], &a["msg"]["tool_calls"], &a["finish"], &a["blocks"]), (&json!("Hello there."), &json!([]), &json!("stop"), &blocks));
    assert_eq!(a["usage"], json!({"cached": 80, "in": 95, "out": 9}));
    assert_eq!((deltas(&d, "reasoning"), deltas(&d, "text")), ("Think.".into(), "Hello there.".into()));
    // A refusal ends the turn, and what streamed before it is not kept; a JSON completion is refused.
    for (step, message) in [(Step::Refuse, "the model declined the request (refusal)"), (Step::Json(json!({})), "the model server answered without a stream")] {
        let m = Mock::start_api(Api::Anthropic, vec![step]);
        let d = anthropic(&m).json("ask");
        d.finish();
        assert_eq!((d.code, &d.result()["error"]), (Some(2), &json!({"kind": "provider", "message": message})), "{}", d.diag());
        assert!(d.of("assistant").is_empty(), "{}", d.diag());
    }
}

/// A reply cut at max_tokens (`length`, anthropic `max_tokens`) runs none of its calls, in either dialect.
#[test]
fn contracts_wire_length_runs_nothing() {
    for api in [Api::OpenAi, Api::Anthropic] {
        let m = Mock::start_api(api, vec![native("shell", &json!({"command": "touch cut-ran"}).to_string(), "length"), say("ok")]);
        let mut r = Run::new(&m.url());
        if api == Api::Anthropic {
            r.set("dialect", "\"anthropic\"");
        }
        let d = r.json("cut");
        ok(&d);
        assert_eq!((&d.of("assistant")[0]["finish"], &d.results()[0]["content"]), (&json!("length"), &json!(CUT)), "{api:?}");
        assert!(!r.path("cut-ran").exists(), "{api:?}: a cut call ran");
    }
}

/// Retries: the retryable statuses wait Retry-After and try again, at most 5 attempts; other errors are not retried;
/// an error event is retried only before any part of the reply; the gateway's own refusal has its kind.
#[test]
fn contracts_wire_errors_and_retries() {
    let busy = |status, times| Step::Error { status, retry_after: Some("0".into()), body: api_error("overloaded_error", "busy"), times };
    let m = Mock::start(vec![busy(503, 4), say("finally")]);
    let d = Run::new(&m.url()).json("retry");
    ok(&d);
    assert_eq!(m.log().len(), 5);
    assert_eq!(d.notices(), (1..=4).map(|n| format!("HTTP 503; retry {n}/4 in 0.0 s")).collect::<Vec<_>>());
    let cases = [
        (vec![busy(429, 5)], 5, "provider", "(gave up after 5 attempts)"),
        (vec![error(400, &api_error("invalid_request_error", "no such thing"), 1)], 1, "provider", "no such thing"),
        (vec![error(403, "halti: blocked: not on the list", 1)], 1, "gateway", "halti: blocked: not on the list"),
        (vec![error(401, &api_error("authentication_error", "who"), 1)], 1, "auth", "who"),
        (vec![Step::ErrorEvent { ty: "overloaded_error".into(), after_start: true }], 1, "provider", "Overloaded"),
    ];
    for (steps, requests, kind, end) in cases {
        let m = Mock::start(steps);
        let d = Run::new(&m.url()).json("fail");
        d.finish();
        let e = &d.result()["error"];
        assert_eq!((d.code, e["kind"].as_str(), m.log().len()), (Some(2), Some(kind), requests), "{}", d.diag());
        assert!(e["message"].as_str().unwrap().ends_with(end), "{e}");
    }
    let m = Mock::start(vec![Step::ErrorEvent { ty: "overloaded_error".into(), after_start: false }, say("second")]);
    let d = Run::new(&m.url()).json("event first");
    ok(&d);
    assert_eq!(m.log().len(), 2, "an error as the first event is retried");
}

/// HTTP: an interim 1xx head is skipped and a body that ends at the close is read; a compressed body is refused.
#[test]
fn contracts_wire_http() {
    let body = fs::read(format!("{}/tests/fixtures/plain_answer.sse", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let interim = [b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".as_slice(), &body].concat();
    let gzip = [b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: gzip\r\nConnection: close\r\n\r\n".as_slice(), &body].concat();
    let m = Mock::start(vec![Step::Raw(interim)]);
    let d = Run::new(&m.url()).json("interim");
    ok(&d);
    assert!(!d.result()["text"].as_str().unwrap().is_empty(), "{}", d.diag());
    let m = Mock::start(vec![Step::Raw(gzip)]);
    let d = Run::new(&m.url()).json("gzip");
    d.finish();
    let e = &d.result()["error"];
    assert_eq!((d.code, &e["kind"], &e["message"]), (Some(2), &json!("provider"), &json!("the model server sent Content-Encoding gzip; henki reads only identity")));
}
