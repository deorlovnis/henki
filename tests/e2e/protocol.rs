#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The wire: call forms, broken streams, errors and retries, timeouts, addresses, escaping, big outputs and the
//! context window.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::time::Duration;

fn content(d: &Done, i: usize) -> String {
    d.results().get(i).and_then(|r| r["content"].as_str()).unwrap_or_else(|| panic!("no tool result {i}\n{}", d.diag())).to_string()
}

#[test]
fn e2e_proto_native_fragments() {
    let args = json!({"command": "printf 'fragments-%s' ok"}).to_string();
    let frag = Step::Call { form: Form::Native { fragments: 9, index: Some(0), id: call_id() }, name: "shell".into(), args, finish: "tool_calls".into() };
    let m = Mock::start(vec![frag, fixture("native_call_fragments"), say("done")]);
    let r = Run::new(&m.url());
    let d = r.json("run it");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("fragments-ok"), "{}", content(&d, 0));
    assert!(content(&d, 1).starts_with("fixture-fragments-ok"), "{}", content(&d, 1));
}

#[test]
fn e2e_proto_par() {
    let two = Step::Stream {
        parts: vec![
            Step::Call {
                form: Form::Native { fragments: 3, index: Some(0), id: call_id() },
                name: "shell".into(),
                args: json!({"command": "echo first >> order.log"}).to_string(),
                finish: String::new(),
            },
            Step::Call {
                form: Form::Native { fragments: 3, index: Some(1), id: call_id() },
                name: "shell".into(),
                args: json!({"command": "echo second >> order.log"}).to_string(),
                finish: String::new(),
            },
        ],
        finish: Some("tool_calls".into()),
        done: true,
        hold_ms: 0,
    };
    let m = Mock::start(vec![two, say("both ran")]);
    let r = Run::new(&m.url());
    let d = r.json("two at once");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert_eq!(std::fs::read_to_string(r.path("order.log")).unwrap(), "first\nsecond\n");
    let a = &d.of("assistant")[0]["msg"]["tool_calls"];
    let ids: Vec<Value> = d.results().iter().map(|x| x["id"].clone()).collect();
    assert_eq!(ids, [a[0]["id"].clone(), a[1]["id"].clone()], "results in call order");
    assert_eq!(m.log()[1].contents("tool").len(), 2);
}

#[test]
fn e2e_proto_two_empty_ids() {
    let m = Mock::start(vec![fixture("two_calls_empty_ids"), say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("two calls, no ids");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let calls = d.of("assistant")[0]["msg"]["tool_calls"].as_array().unwrap().clone();
    assert_eq!(calls.len(), 2, "both kept");
    assert_eq!((calls[0]["id"].as_str(), calls[1]["id"].as_str()), (Some("h1"), Some("h2")));
    assert!(content(&d, 0).starts_with("empty-id-one") && content(&d, 1).starts_with("empty-id-two"));
}

#[test]
fn e2e_proto_nodone() {
    let m = Mock::start(vec![Step::NoDone]);
    let r = Run::new(&m.url());
    let d = r.json("start something");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("stream_incomplete")), "{}", d.diag());
    assert!(d.notices().iter().any(|n| n.contains("dropped calls: shell")), "the notice names the call: {:?}", d.notices());
    assert!(!r.path("nodone-ran").exists(), "a call of a broken stream ran");
    assert!(d.of("assistant").iter().all(|a| a["msg"]["tool_calls"].as_array().is_some_and(Vec::is_empty)), "a partial carries no calls");
}

#[test]
fn e2e_proto_stop_with_calls() {
    let m = Mock::start(vec![native("shell", &json!({"command": "printf stop-ran"}).to_string(), "stop"), fixture("call_finish_stop"), say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("run with stop");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(content(&d, 0).starts_with("stop-ran") && content(&d, 1).starts_with("fixture-stop-ok"), "{:?}", d.results());
}

#[test]
fn e2e_proto_length_cut_args() {
    let m = Mock::start(vec![native("shell", r#"{"command": "touch cut-ran && echo"#, "length"), fixture("length_cut_call"), say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("long call");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    for i in 0..2 {
        assert_eq!(content(&d, i), "arguments cut at max_tokens; not run. Send a shorter call.");
        assert_eq!(d.results()[i]["is_error"], true);
    }
    assert!(!r.path("cut-ran").exists() && !r.path("fixture-cut-ran").exists(), "a cut call ran");
}

#[test]
fn e2e_proto_malformed_then_fixed() {
    let bad = r#"{"command": "printf fixed", "#;
    let m = Mock::start(vec![native("shell", bad, "tool_calls"), shell("printf fixed"), say("fixed it")]);
    let r = Run::new(&m.url());
    let d = r.json("try twice");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let first = content(&d, 0);
    assert!(first.starts_with("arguments are not valid JSON (line 1 col ") && first.ends_with(bad), "{first}");
    let a = &d.of("assistant")[0]["msg"]["tool_calls"][0];
    assert_eq!((&a["args"], a["raw"].as_str()), (&json!({}), Some(bad)));
    let replayed = &m.log()[1].messages()[2]["tool_calls"][0]["function"]["arguments"];
    assert_eq!(replayed, "{}", "an invalid call replays with {{}}");
    assert!(content(&d, 1).starts_with("fixed"));
}

/// A text call: run once, journaled without its markup, replayed as a native call with an `h<N>` id.
fn text_form(steps: Vec<Step>, out: &[&str]) -> Done {
    let n = steps.len();
    let m = Mock::start([steps, vec![say("done")]].concat());
    let r = Run::new(&m.url());
    let d = r.json("use text calls");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    for (i, o) in out.iter().enumerate() {
        assert!(content(&d, i).starts_with(o), "{}", content(&d, i));
    }
    for a in d.of("assistant").iter().take(n) {
        let text = a["msg"]["content"].as_str().unwrap();
        assert!(!text.contains("<tool_call>") && !text.contains("<function=") && !text.contains("```json"), "markup kept: {text}");
        assert!(a["msg"]["tool_calls"][0]["id"].as_str().unwrap().starts_with('h'));
    }
    let tc = &m.log()[1].messages()[2]["tool_calls"][0];
    assert_eq!((tc["type"].as_str(), tc["function"]["name"].as_str(), tc["id"].as_str()), (Some("function"), Some("shell"), Some("h1")), "{tc}");
    d
}

#[test]
fn e2e_proto_hermes() {
    let mut lead = say("Let me look.\n");
    if let Step::Say(t) = &mut lead {
        t.push_str(&markup(&Form::Hermes, "shell", &json!({"command": "printf hermes-ok"}).to_string()));
    }
    let d = text_form(vec![lead, fixture("hermes_in_content")], &["hermes-ok", "fixture-hermes-ok"]);
    assert!(d.of("assistant")[0]["msg"]["content"].as_str().unwrap().starts_with("Let me look."));
}

#[test]
fn e2e_proto_qwen_xml() {
    let bare = say("<function=shell>\n<parameter=command>\nprintf bare-ok\n</parameter>\n<parameter=timeout>\n5\n</parameter>\n</function>");
    text_form(vec![text_call(Form::QwenXml, "shell", json!({"command": "printf qwen-ok"})), bare, fixture("qwen_xml_in_content")], &["qwen-ok", "bare-ok", "fixture-qwen-ok"]);
}

#[test]
fn e2e_proto_fenced() {
    let d = text_form(vec![text_call(Form::Fenced, "shell", json!({"command": "printf fenced-ok"})), fixture("fenced_json_call")], &["fenced-ok", "fixture-fenced-ok"]);
    assert_eq!(d.results().len(), 2);
    let m = Mock::start(vec![say("```json\n{\"name\": \"deploy\", \"arguments\": {\"target\": \"prod\"}}\n```")]);
    let r = Run::new(&m.url());
    let d = r.json("unoffered name");
    d.finish();
    assert_eq!((d.code, d.results().len()), (Some(0), 0), "a fence naming no offered tool is prose\n{}", d.diag());
}

#[test]
fn e2e_proto_bash_fence_never_runs() {
    for (step, prose) in
        [(say("Run this:\n```bash\ntouch pwned\n```\n```sh\ntouch pwned2\n```"), "```bash\ntouch pwned"), (fixture("bash_fence_prose"), "```bash\ntouch pwned && rm")]
    {
        let m = Mock::start(vec![step]);
        let r = Run::new(&m.url());
        let d = r.json("show me");
        d.finish();
        assert_eq!((d.code, d.results().len(), m.log().len()), (Some(0), 0, 1), "{}", d.diag());
        assert!(d.result()["text"].as_str().unwrap().contains(prose));
        assert!(!r.path("pwned").exists() && !r.path("pwned2").exists(), "a fence ran");
    }
}

#[test]
fn e2e_proto_json_completion() {
    let message = |content: Value, calls: Value| {
        json!({"id": "cmpl-1", "object": "chat.completion", "model": "mock-model",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content, "tool_calls": calls}, "finish_reason": if calls.is_null() { "stop" } else { "tool_calls" }}],
        "usage": {"prompt_tokens": 500, "completion_tokens": 9, "total_tokens": 509}})
    };
    let call = json!([{"id": "call_json_1", "type": "function", "function": {"name": "shell", "arguments": "{\"command\":\"printf json-ok\"}"}}]);
    let m = Mock::start(vec![Step::Json(message(Value::Null, call)), Step::Json(message(json!("json done"), Value::Null))]);
    let r = Run::new(&m.url());
    let d = r.json("no streaming");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("json done")), "{}", d.diag());
    assert!(content(&d, 0).starts_with("json-ok"));
    assert_eq!(d.of("assistant")[0]["usage"]["in"], 500);
}

#[test]
fn e2e_finish_then_idle() {
    let m = Mock::start(vec![Step::Stream { parts: vec![say("idle answer")], finish: Some("stop".into()), done: false, hold_ms: 15_000 }]);
    let r = Run::new(&m.url());
    let d = r.json("answer and idle");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("idle answer")), "{}", d.diag());
    assert!(d.elapsed >= Duration::from_millis(1900) && d.elapsed < Duration::from_secs(6), "{:?}: 2 s of idle completes", d.elapsed);
}

#[test]
fn e2e_retry_after_429() {
    let body = r#"{"error":{"message":"slow down","type":"rate_limit_error"}}"#;
    let m = Mock::start(vec![Step::Error { status: 429, retry_after: Some("2".into()), body: body.into(), times: 1 }, say("after the wait")]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("after the wait")), "{}", d.diag());
    let log = m.log();
    assert!(log[1].at - log[0].at >= Duration::from_millis(1800), "gap {:?}", log[1].at - log[0].at);
    assert!(d.notices().contains(&"HTTP 429; retry 1/4 in 2.0 s".to_string()), "{:?}", d.notices());
}

#[test]
fn e2e_500_bounded() {
    let m = Mock::start(vec![Step::Error { status: 500, retry_after: Some("0".into()), body: r#"{"error":{"message":"boom"}}"#.into(), times: 50 }]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("provider")), "{}", d.diag());
    assert!(d.result()["error"]["message"].as_str().unwrap().ends_with("(gave up after 5 attempts)"), "{}", d.result());
    assert_eq!(m.log().len(), 5, "5 attempts, no more");
    assert_eq!(d.notices().iter().filter(|n| n.starts_with("HTTP 500; retry ")).count(), 4);
}

#[test]
fn e2e_server_down_fix() {
    let port = closed_port();
    let r = Run::new(&format!("http://127.0.0.1:{port}/v1"));
    let d = r.cmd(&["--json", "hello"]).timeout(Duration::from_secs(40)).go();
    d.finish();
    let msg = d.result()["error"]["message"].as_str().unwrap_or("").to_string();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("provider")), "{}", d.diag());
    assert!(msg.starts_with(&format!("could not connect to 127.0.0.1:{port}")), "{msg}");
    let fix = format!("(gave up after 5 attempts); fix: start the model server on 127.0.0.1:{port} or set base_url in {}", r.config_path.display());
    assert!(msg.ends_with(&fix), "{msg}");
    let n = d.notices();
    for (k, s) in [(1, "1.0"), (2, "2.0"), (3, "4.0"), (4, "8.0")] {
        assert!(n.iter().any(|x| x.ends_with(&format!("; retry {k}/4 in {s} s"))), "{n:?}");
    }
}

#[test]
fn e2e_stall_budget() {
    let m = Mock::start(vec![Step::Stall { before_first_byte: false, after_words: 2, for_ms: 30_000 }]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-seconds", "3", "talk"]).go();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(124), "budget"), "{}", d.diag());
    assert!(d.elapsed <= Duration::from_secs(4), "exit after {:?}", d.elapsed);
    assert!(m.wait_closed(0, SEC), "the mock never saw the socket close");
    let a = &d.of("assistant")[0];
    assert_eq!((a["finish"].as_str(), a["msg"]["content"].as_str()), (Some("interrupted"), Some("w1 w2")), "the partial is journaled: {a}");
}

#[test]
fn e2e_no_key_no_auth() {
    let m = Mock::start(vec![say("ok")]);
    let r = Run::new(&m.url());
    let d = r.json("hello");
    d.finish();
    let l = &m.log()[0];
    assert_eq!(l.line, "POST /v1/chat/completions HTTP/1.1");
    assert!(l.header("authorization").is_none(), "{:?}", l.headers);
    assert!(l.header("accept-encoding").is_none());
    assert_eq!((l.header("accept"), l.header("connection")), (Some("text/event-stream"), Some("close")));
    assert_eq!(l.header("user-agent"), Some(concat!("henki/", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn e2e_thinking_switch_sent() {
    let m = Mock::start(vec![shell("true"), say("ok")]);
    let r = Run::new(&m.url());
    r.json("hello").finish();
    for l in m.log() {
        assert_eq!(l.json["chat_template_kwargs"], json!({"enable_thinking": false}), "{}", l.text());
        assert_eq!((&l.json["stream"], &l.json["stream_options"]), (&json!(true), &json!({"include_usage": true})));
    }
}

#[test]
fn e2e_proxy_env_ignored() {
    let m = Mock::start(vec![say("direct")]);
    let mut r = Run::new(&m.url());
    let proxy = format!("http://127.0.0.1:{}", closed_port());
    for k in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy"] {
        r.env(k, &proxy);
    }
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 1), "{}", d.diag());
    assert_eq!(m.log()[0].line, "POST /v1/chat/completions HTTP/1.1", "not a proxy request");
}

#[test]
fn e2e_localhost_v6_first() {
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = ("localhost", 1).to_socket_addrs().map(Iterator::collect).unwrap_or_default();
    let (v4, v6) = (addrs.iter().any(|a| a.is_ipv4()), addrs.iter().any(|a| a.is_ipv6()));
    if !(v4 && v6) || std::net::TcpListener::bind("[::1]:0").is_err() {
        println!("skipped: localhost does not resolve to both ::1 and 127.0.0.1 here ({addrs:?})");
        return;
    }
    let last = *addrs.last().unwrap();
    let bind = if last.is_ipv6() { "[::1]:0" } else { "127.0.0.1:0" };
    let m = Mock::start_at(bind.parse().unwrap(), vec![say("found it")]).unwrap();
    let r = Run::new(&format!("http://localhost:{}/v1", m.port()));
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 1), "henki must try every address\n{}", d.diag());
}

#[test]
fn e2e_error_body_escaped() {
    let osc = "\u{1b}]52;c;aGVsbG8=\u{7}";
    let csi = "\u{1b}[31m";
    let body = json!({"error": {"message": format!("bad {osc} and {csi}red")}}).to_string();
    let tool = "printf 'out \\033]52;c;aGk=\\007 \\033[2J'";
    for json_mode in [false, true] {
        let flag: &[&str] = if json_mode { &["--json"] } else { &[] };
        let m = Mock::start(vec![error(404, &body, 1)]);
        let r = Run::new(&m.url());
        let d = r.cmd(&[flag, &["hello"]].concat()).go();
        d.finish();
        let all = [d.stdout.clone(), d.stderr.clone(), d.journal_bytes.clone()].concat();
        assert!(!all.iter().any(|&b| b == 0x1b || b == 0x07), "raw escapes written\n{}", d.diag());
        let want = if json_mode { "\\u001b]52;c;aGVsbG8=\\u0007" } else { "\\x1b]52;c;aGVsbG8=\\x07" };
        assert!(if json_mode { d.out() } else { d.err() }.contains(want), "{}", d.diag());
        let m = Mock::start(vec![shell(tool), say(&format!("answer {csi}x {osc}"))]);
        let r = Run::new(&m.url());
        let d = r.cmd(&[flag, &["hello"]].concat()).go();
        d.finish();
        assert_eq!(d.code, Some(0), "{}", d.diag());
        let all = [d.stdout.clone(), d.stderr.clone(), d.journal_bytes.clone()].concat();
        assert!(!all.iter().any(|&b| b == 0x1b || b == 0x07), "raw escapes written\n{}", d.diag());
        if json_mode {
            assert!(d.out().contains("out \\u001b]52;c;aGk=\\u0007 \\u001b[2J"), "tool output escaped: {}", d.out());
            assert!(d.out().contains("answer \\u001b[31mx \\u001b]52"), "{}", d.out());
        } else {
            assert_eq!(d.out(), "answer \\x1b[31mx \\x1b]52;c;aGVsbG8=\\x07\n");
        }
    }
}

#[test]
fn e2e_huge_output() {
    let m = Mock::start(vec![shell("head -c 20000000 /dev/zero | tr '\\0' x; echo; echo tail-marker"), say("big")]);
    let r = Run::new(&m.url());
    let d = r.json("make noise");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let c = content(&d, 0);
    let cap = 16_384;
    assert!(c.len() <= cap + 400, "result {} bytes", c.len());
    assert!(c.contains(" bytes cut; full output: ") && c.contains("tail-marker"), "{}", &c[c.len() - 300..]);
    let shown = c.split("full output: ").nth(1).unwrap().split(" ...]").next().unwrap();
    let spill = if let Some(rest) = shown.strip_prefix("~/") { r.home.join(rest) } else { std::path::PathBuf::from(shown) };
    let size = std::fs::metadata(&spill).unwrap_or_else(|e| panic!("{shown}: {e}")).len();
    assert!(size > 8_000_000 && size <= (8 << 20) + 100, "spill {size} bytes");
    let text = std::fs::read_to_string(&spill).unwrap();
    assert!(text.trim_end().ends_with("more bytes dropped]"), "{}", &text[text.len() - 100..]);
    let journal = String::from_utf8_lossy(&d.journal_bytes);
    let line = journal.lines().find(|l| l.contains("\"t\":\"tool_result\"")).unwrap();
    assert!(line.len() <= cap + 4096, "journal line {} bytes", line.len());
    assert!(d.results()[0]["bytes"].as_u64().unwrap() >= 20_000_000, "bytes is the full output");
}

/// `n` bytes of `c` from the shell.
fn bytes_cmd(n: usize, c: char) -> String {
    format!("head -c {n} /dev/zero | tr '\\0' {c}")
}

#[test]
fn e2e_ctx_elide_run() {
    let mut steps: Vec<Step> = (0..6).map(|_| shell(&bytes_cmd(2000, 'y'))).collect();
    steps.push(say("all read"));
    let m = Mock::start(steps);
    let mut r = Run::new(&m.url());
    r.set("context_window", "4096");
    let d = r.json("read a lot");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 7), "{}", d.diag());
    let elided: Vec<String> = d.of("elide").iter().flat_map(|e| e["ids"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_string()).collect::<Vec<_>>()).collect();
    assert!(!elided.is_empty(), "nothing was elided");
    let last = m.log().pop().unwrap();
    let tools: Vec<Value> = last.messages().into_iter().filter(|x| x["role"] == "tool").collect();
    assert_eq!(tools.len(), 6);
    for (i, t) in tools.iter().enumerate() {
        let id = t["tool_call_id"].as_str().unwrap().to_string();
        let original = &d.results()[i]["content"];
        if elided.contains(&id) {
            assert!(i < 4, "one of the last two steps was elided");
            assert_eq!(t["content"].as_str().unwrap(), format!("[output elided: {} bytes]", original.as_str().unwrap().len()));
        } else {
            assert_eq!(&t["content"], original);
        }
    }
    let at = d.segment().iter().position(|x| x["t"] == "elide").unwrap();
    assert!(d.segment()[at..].iter().any(|x| x["t"] == "assistant"), "the elide record precedes the request it shrinks");
}

#[test]
fn e2e_ctx_exceeded_bounded() {
    let body = r#"{"error":{"message":"the request exceeds the available context size","type":"invalid_request_error"}}"#;
    let mut steps: Vec<Step> = (0..3).map(|_| shell(&bytes_cmd(2000, 'q'))).collect();
    steps.push(error(400, body, 20));
    let m = Mock::start(steps);
    let r = Run::new(&m.url());
    let d = r.json("overflow");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("context_exceeded")), "{}", d.diag());
    let n = m.log().len();
    assert!((4..=5).contains(&n), "{n} requests: at most one elision and one more try");
    assert_eq!(d.of("elide").len(), n - 4, "one elide record per extra try");
}

#[test]
fn e2e_fixture_replies() {
    let cases: [(&str, &str); 8] = [
        ("plain_answer", "The answer is forty-two."),
        ("reasoning_then_answer", "Three files changed."),
        ("comments_keepalive", "Still here."),
        ("usage_only_chunk", "Counted."),
        ("call_no_index", "fixture-no-index-ok"),
        ("call_name_split", "fixture-split-name-ok"),
        ("native_call_one_chunk", "fixture-one-chunk-ok"),
        ("qwen_xml_bare", "fixture-qwen-bare-ok"),
    ];
    for (name, want) in cases {
        let m = Mock::start(vec![fixture(name), say("fixture done")]);
        let r = Run::new(&m.url());
        let d = r.json("fixture");
        d.finish();
        assert_eq!(d.code, Some(0), "{name}\n{}", d.diag());
        let got = d.results().first().map(|x| x["content"].as_str().unwrap().to_string()).unwrap_or_else(|| d.result()["text"].as_str().unwrap().to_string());
        assert!(got.starts_with(want), "{name}: {got}");
    }
    let m = Mock::start(vec![fixture("reasoning_only")]);
    let d = Run::new(&m.url()).json("fixture");
    d.finish();
    assert_eq!((d.code, d.result()["reason"].as_str()), (Some(1), Some("empty_answer")), "{}", d.diag());
    let m = Mock::start(vec![fixture("length_degenerate_text")]);
    let d = Run::new(&m.url()).json("fixture");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    assert!(d.notices().contains(&"answer cut at max_tokens".to_string()), "{:?}", d.notices());
    let m = Mock::start(vec![fixture("error_event_first"), say("second try")]);
    let d = Run::new(&m.url()).json("fixture");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 2), "a retryable first error event is retried\n{}", d.diag());
    let m = Mock::start(vec![fixture("error_event_later"), say("never")]);
    let d = Run::new(&m.url()).json("fixture");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(2), 1), "an error after the first event is fatal\n{}", d.diag());
    let m = Mock::start(vec![fixture("content_with_escapes")]);
    let d = Run::new(&m.url()).cmd(&["fixture"]).go();
    d.finish();
    assert_eq!(d.out(), "Colours: \\x1b[32mgreen\\x1b[0m and a title \\x1b]0;owned\\x07.\n", "{}", d.diag());
}
