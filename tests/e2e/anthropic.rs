#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The Anthropic Messages dialect against the mock in Anthropic mode, which refuses what the API refuses (a changed
//! replay of a reply, a thinking signature from another system prompt or tool set, unpaired tool blocks, extra
//! fields): the request shape, tool round trips, thinking replayed unchanged also after a resume, the stop reasons,
//! errors and retries, usage, no elision, the long silences of a buffered tool input, and the box profile.
use crate::harness::*;
use crate::mock::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

/// A run in the Anthropic dialect at `m`.
fn at(r: &mut Run, m: &Mock) {
    r.at(&m.url()).set("dialect", "\"anthropic\"");
}

fn scripted(steps: Vec<Step>) -> (Run, Mock) {
    let m = Mock::start_api(Api::Anthropic, steps);
    let mut r = Run::new("");
    at(&mut r, &m);
    (r, m)
}

/// Nothing the mock saw was refused.
fn clean(m: &Mock) {
    assert!(m.rejections().is_empty(), "{:?}", m.rejections());
}

/// A reply that thinks first, then does `step`.
fn thinking(thought: &str, step: Step) -> Step {
    Step::Stream { parts: vec![reason(thought), step], finish: Some("tool_calls".into()), done: true, hold_ms: 0 }
}

fn error_of(d: &Done) -> (Option<i32>, String, String) {
    let e = &d.result()["error"];
    (d.code, e["kind"].as_str().unwrap_or("").into(), e["message"].as_str().unwrap_or("").into())
}

#[test]
fn e2e_anthropic_request_shape() {
    let (mut r, m) = scripted(vec![say("shaped")]);
    r.set("api_key", "\"k-local-1\"");
    let d = r.json("hello");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    let l = &m.log()[0];
    assert_eq!(l.line, "POST /v1/messages HTTP/1.1");
    assert_eq!((l.header("anthropic-version"), l.header("x-api-key"), l.header("authorization")), (Some("2023-06-01"), Some("k-local-1"), None));
    let keys: Vec<&str> = l.json.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["cache_control", "max_tokens", "messages", "model", "stream", "system", "tools"], "nothing OpenAI, no thinking, no sampling");
    let ephemeral = json!({"type": "ephemeral"});
    assert_eq!((&l.json["cache_control"], &l.json["stream"], &l.json["max_tokens"]), (&ephemeral, &json!(true), &json!(4096)));
    let system = l.json["system"].as_array().unwrap();
    assert_eq!((system.len(), &system[0]["cache_control"], &system[0]["type"]), (1, &ephemeral, &json!("text")));
    for (t, name) in l.json["tools"].as_array().unwrap().iter().zip(["shell", "read", "write", "edit"]) {
        assert_eq!(t.as_object().unwrap().keys().collect::<Vec<_>>(), ["description", "input_schema", "name"]);
        assert_eq!((t["name"].as_str(), t["input_schema"]["type"].as_str()), (Some(name), Some("object")));
    }
    let msgs = l.messages();
    assert_eq!((msgs.len(), &msgs[0]["role"], &msgs[0]["content"][0]["type"]), (1, &json!("user"), &json!("text")));
    assert!(msgs[0]["content"][0]["text"].as_str().unwrap().ends_with("\n\nhello"));
    assert_eq!(d.of("assistant")[0]["blocks"], json!([{"text": "shaped", "type": "text"}]));
}

#[test]
fn e2e_anthropic_tool_round_trip() {
    let (r, m) = scripted(vec![shell("echo round-trip"), say("done")]);
    let d = r.json("go");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("done")), "{}", d.diag());
    clean(&m);
    let msgs = m.log()[1].messages();
    let (used, result) = (&msgs[1]["content"][0], &msgs[2]["content"][0]);
    assert_eq!((msgs.len(), &used["type"], &used["input"]), (3, &json!("tool_use"), &json!({"command": "echo round-trip"})));
    assert_eq!((&result["type"], &result["tool_use_id"], &result["is_error"]), (&json!("tool_result"), &used["id"], &json!(false)));
    assert!(result["content"].as_str().unwrap().starts_with("round-trip\n"), "{result}");
    assert_eq!(d.results()[0]["id"], used["id"], "the server's id is the call's id");
}

#[test]
fn e2e_anthropic_thinking_replayed() {
    let (r, m) = scripted(vec![thinking("I should list the files.", shell("ls")), say("listed")]);
    let d = r.json("list");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    let sent = &m.log()[1].messages()[1]["content"];
    assert_eq!((&sent[0]["type"], &sent[0]["thinking"], &sent[1]["type"]), (&json!("thinking"), &json!("I should list the files."), &json!("tool_use")));
    assert!(sent[0]["signature"].as_str().unwrap().starts_with("EqMockSig"), "{sent}");
    assert_eq!(&d.of("assistant")[0]["blocks"], sent, "the blocks as journaled are the blocks replayed");
}

#[test]
fn e2e_anthropic_resume_replays_blocks() {
    let mut r = Run::new("");
    let m = Mock::start_api(Api::Anthropic, vec![thinking("first", shell("echo one")), thinking("second", shell(&r.exec_sleep())), say("never")]);
    at(&mut r, &m);
    let mut l = r.cmd(&["--json", "two calls"]).spawn();
    l.wait_marked(5 * SEC);
    l.kill9();
    let d = l.wait();
    d.finish();
    clean(&m);
    let m2 = Mock::start_api(Api::Anthropic, vec![say("resumed once")]);
    at(&mut r, &m2);
    let d = r.cmd(&["--json", "--resume"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m2);
    let msgs = m2.log()[0].messages();
    let kinds: Vec<&Value> = msgs.iter().map(|x| &x["content"][0]["type"]).collect();
    assert_eq!(kinds, ["text", "thinking", "tool_result", "thinking", "tool_result"], "both replies replayed with their thinking");
    assert_eq!(msgs[4]["content"][0]["content"], "interrupted; the command may have partly run");
    let m3 = Mock::start_api(Api::Anthropic, vec![say("resumed twice")]);
    at(&mut r, &m3);
    let d = r.cmd(&["--json", "--resume", "and once more"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m3);
    let (a, b) = (m2.log()[0].text().to_string(), m3.log()[0].text().to_string());
    assert!(b.starts_with(messages_prefix(&a)), "the second resume does not replay the first byte for byte:\n{a}\n{b}");
}

#[test]
fn e2e_anthropic_merge_results_and_task() {
    let (mut r, m) = scripted(vec![shell("echo partial"), say("never")]);
    let d = r.cmd(&["--json", "--max-steps", "1", "work"]).go();
    d.finish();
    assert_eq!(d.result()["reason"], "max_steps", "{}", d.diag());
    let m2 = Mock::start_api(Api::Anthropic, vec![say("merged")]);
    at(&mut r, &m2);
    let d = r.cmd(&["--json", "--resume", "next task"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    clean(&m2);
    let msgs = m2.log()[0].messages();
    let last = msgs.last().unwrap();
    assert_eq!((msgs.len(), &last["role"], last["content"].as_array().unwrap().len()), (3, &json!("user"), 2), "{last}");
    assert_eq!((&last["content"][0]["type"], &last["content"][1]), (&json!("tool_result"), &json!({"text": "next task", "type": "text"})), "results first, then the task");
}

#[test]
fn e2e_anthropic_refusal() {
    let (r, m) = scripted(vec![Step::Refuse, say("never")]);
    let d = r.json("do something bad");
    d.finish();
    assert_eq!(error_of(&d), (Some(2), "provider".into(), "the model declined the request (refusal)".into()), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

#[test]
fn e2e_anthropic_max_tokens_cuts_calls() {
    let (r, m) = scripted(vec![native("shell", &json!({"command": "touch cut-ran"}).to_string(), "length"), say("shorter now")]);
    let d = r.json("long call");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    assert_eq!(d.of("assistant")[0]["finish"], "length");
    assert_eq!((&d.results()[0]["content"], &d.results()[0]["is_error"]), (&json!("arguments cut at max_tokens; not run. Send a shorter call."), &json!(true)));
    assert!(!r.path("cut-ran").exists(), "a cut call ran");
}

#[test]
fn e2e_anthropic_overloaded_first_retried() {
    let (r, m) = scripted(vec![Step::ErrorEvent { ty: "overloaded_error".into(), after_start: false }, say("second try")]);
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, m.log().len()), (Some(0), 2), "{}", d.diag());
    assert!(d.notices().iter().any(|n| n.contains("; retry 1/4 in ")), "{:?}", d.notices());
}

#[test]
fn e2e_anthropic_error_after_start_fatal() {
    let (r, m) = scripted(vec![Step::ErrorEvent { ty: "overloaded_error".into(), after_start: true }, say("never")]);
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str(), m.log().len()), (Some(2), Some("provider"), 1), "{}", d.diag());
}

#[test]
fn e2e_anthropic_429_retry_after() {
    let limited = Step::Error { status: 429, retry_after: Some("2".into()), body: api_error("rate_limit_error", "slow down"), times: 1 };
    let (r, m) = scripted(vec![limited, say("after the wait")]);
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("after the wait")), "{}", d.diag());
    let log = m.log();
    assert!(log[1].at - log[0].at >= Duration::from_millis(1800), "gap {:?}", log[1].at - log[0].at);
    assert!(d.notices().contains(&"HTTP 429; retry 1/4 in 2.0 s".to_string()), "{:?}", d.notices());
}

#[test]
fn e2e_anthropic_auth_standin_fix() {
    let m = Mock::start_api(Api::Anthropic, vec![say("let in")]).standin("cred-managed");
    let mut r = Run::new("");
    at(&mut r, &m);
    let d = r.json("hello");
    d.finish();
    let (code, kind, msg) = error_of(&d);
    assert_eq!((code, kind.as_str(), m.log().len()), (Some(2), "auth", 1), "{}", d.diag());
    assert!(msg.ends_with("; fix: set api_key to the gateway's stand-in"), "{msg}");
    r.set("api_key", "\"cred-managed\"");
    let d = r.json("hello");
    d.finish();
    assert_eq!((d.code, m.log()[1].header("x-api-key")), (Some(0), Some("cred-managed")), "{}", d.diag());
}

#[test]
fn e2e_anthropic_gateway_body() {
    let (r, m) = scripted(vec![error(403, "halti: this box may not use that model\nask an admin", 3), say("never")]);
    let d = r.json("hello");
    d.finish();
    assert_eq!(error_of(&d), (Some(2), "gateway".into(), "halti: this box may not use that model".into()), "{}", d.diag());
    assert_eq!(m.log().len(), 1, "never retried");
}

#[test]
fn e2e_anthropic_no_elide() {
    let mut steps: Vec<Step> = (0..3).map(|_| shell("head -c 2000 /dev/zero | tr '\\0' q")).collect();
    steps.push(error(400, &api_error("invalid_request_error", "prompt is too long: 5000 tokens > 4096 maximum"), 20));
    let (mut r, m) = scripted(steps);
    r.set("context_window", "4096");
    let d = r.json("overflow");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("context_exceeded")), "{}", d.diag());
    assert_eq!((m.log().len(), d.of("elide").len()), (4, 0), "no elision and no second try");
    let sent: Vec<Value> =
        m.log()[3].messages().iter().flat_map(|x| x["content"].as_array().unwrap().clone()).filter(|b| b["type"] == "tool_result").map(|b| b["content"].clone()).collect();
    let journaled: Vec<Value> = d.results().iter().map(|x| x["content"].clone()).collect();
    assert_eq!(sent, journaled, "the history went out unedited");
}

#[test]
fn e2e_anthropic_stream_incomplete() {
    let (r, m) = scripted(vec![Step::NoDone]);
    let d = r.json("start something");
    d.finish();
    assert_eq!((d.code, d.result()["error"]["kind"].as_str()), (Some(2), Some("stream_incomplete")), "{}", d.diag());
    assert!(d.notices().iter().any(|n| n.contains("dropped calls: shell")), "{:?}", d.notices());
    assert!(!r.path("nodone-ran").exists() && m.log().len() == 1);
}

#[test]
fn e2e_anthropic_finish_then_idle() {
    let (r, _m) = scripted(vec![Step::Stream { parts: vec![say("idle answer")], finish: Some("stop".into()), done: false, hold_ms: 15_000 }]);
    let d = r.json("answer and idle");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("idle answer")), "{}", d.diag());
    assert!(d.elapsed >= Duration::from_millis(1900) && d.elapsed < Duration::from_secs(6), "{:?}: 2 s of idle after message_delta completes", d.elapsed);
}

#[test]
fn e2e_anthropic_usage_sums() {
    let usage = json!({"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 1000, "output_tokens": 1});
    let msg = json!({"id": "msg_usage", "type": "message", "role": "assistant", "content": [], "model": "mock-model", "stop_reason": null, "stop_sequence": null, "usage": usage});
    let events = [
        ("message_start", json!({"type": "message_start", "message": msg})),
        ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})),
        ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "counted"}})),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
        ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 7}})),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let (r, _m) = scripted(vec![Step::Events(events.into_iter().map(|(n, v)| (n.to_string(), v)).collect())]);
    let d = r.json("count");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    let want = json!({"cached": 1000, "in": 1210, "out": 7});
    assert_eq!((&d.of("assistant")[0]["usage"], &d.result()["usage"]), (&want, &want));
}

#[test]
fn e2e_anthropic_resume_dialect_mismatch() {
    let m = Mock::start(vec![shell("echo one"), say("never")]);
    let mut r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-steps", "1", "work"]).go();
    d.finish();
    let m2 = Mock::start_api(Api::Anthropic, vec![say("never")]);
    at(&mut r, &m2);
    let d = r.cmd(&["--json", "--resume", "go on"]).go();
    d.finish();
    let (code, kind, msg) = error_of(&d);
    assert_eq!((code, kind.as_str(), m2.log().len()), (Some(2), "journal", 0), "{}", d.diag());
    assert!(msg.ends_with(": the session's dialect is openai, not anthropic"), "{msg}");
}

#[test]
fn e2e_anthropic_json_refused() {
    let message = json!({"id": "msg_json", "type": "message", "role": "assistant", "model": "mock-model", "content": [{"type": "text", "text": "no stream"}],
        "stop_reason": "end_turn", "stop_sequence": null, "usage": {"input_tokens": 9, "output_tokens": 2}});
    let (r, m) = scripted(vec![Step::Json(message)]);
    let d = r.json("hello");
    d.finish();
    assert_eq!(error_of(&d), (Some(2), "provider".into(), "the model server answered without a stream".into()), "{}", d.diag());
    assert_eq!(m.log().len(), 1);
}

/// The API buffers a tool's input and can then be silent for minutes, sending nothing at all (no `ping`): longer than
/// the 60 s an OpenAI server may be silent.
#[test]
fn e2e_anthropic_long_silence() {
    let (r, m) = scripted(vec![Step::Stall { before_first_byte: false, after_words: 1, for_ms: 65_000 }]);
    let d = r.cmd(&["--json", "wait for it"]).timeout(Duration::from_secs(120)).go();
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str(), m.log().len()), (Some(0), Some("w1 end"), 1), "{}", d.diag());
    assert!(d.elapsed >= Duration::from_secs(65), "{:?}", d.elapsed);
}

/// A replayed thinking block is bound to the system prompt and tools it was produced under; after the global
/// AGENTS.md changed, the API refuses the resumed session's request, and henki stops at once.
#[test]
fn e2e_anthropic_resume_after_prompt_change() {
    let (mut r, m) = scripted(vec![Step::Stream { parts: vec![reason("weighing it"), say("first answer")], finish: Some("stop".into()), done: true, hold_ms: 0 }]);
    let d = r.json("think first");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    fs::write(r.config_dir().join("AGENTS.md"), "Answer in French.\n").unwrap();
    let m2 = Mock::start_api(Api::Anthropic, vec![say("never")]);
    at(&mut r, &m2);
    let d = r.cmd(&["--json", "--resume", "again"]).go();
    d.finish();
    let (code, kind, msg) = error_of(&d);
    assert_eq!((code, kind.as_str(), m2.log().len()), (Some(2), "provider", 1), "{}", d.diag());
    assert_eq!(m2.rejections().len(), 1);
    assert!(msg.contains("Invalid signature in thinking block") && msg.contains("bound to a different conversation"), "{msg}");
}

/// The shipped box profile, pointed at the mock under `/model` and at a doc-only extension.
#[test]
fn e2e_box_profile_run() {
    let m = Mock::start_api(Api::Anthropic, vec![say("box ok")]).standin("cred-managed");
    let r = Run::new("");
    let ext = r.dir.join("mnt/ext");
    fs::create_dir_all(ext.join("itse")).unwrap();
    let line = "itse workbench: itse --root . status, build, test, problems; exit 0 ok, 1 failed, 2 error, 3 ask the user";
    fs::write(ext.join("itse/doc.md"), format!("# {line}\n")).unwrap();
    let profile = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/profiles/config.box.toml")).unwrap();
    let text = profile.replace("http://127.0.0.2/model", &format!("http://127.0.0.1:{}/model", m.port())).replace("/mnt/henki/ext", &ext.display().to_string());
    assert!(text.contains(&format!("127.0.0.1:{}/model", m.port())) && text.contains(&ext.display().to_string()), "the profile changed shape:\n{profile}");
    let conf = r.dir.join("mnt/config.toml");
    fs::write(&conf, text).unwrap();
    fs::set_permissions(&conf, fs::Permissions::from_mode(0o600)).unwrap();
    let d = r.cmd(&["--json", "--config", conf.to_str().unwrap(), "check the build"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    let l = &m.log()[0];
    assert_eq!((l.line.as_str(), l.header("x-api-key")), ("POST /model/v1/messages HTTP/1.1", Some("cred-managed")));
    assert!(l.system().ends_with(&format!("\n- itse: {line} [{}]", ext.join("itse/doc.md").display())), "{}", l.system());
}

/// One POST to the mock; the status and the body (an SSE body as its `data:` payloads).
fn post(m: &Mock, path: &str, headers: &[(&str, &str)], body: &Value) -> (u16, Vec<Value>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", m.port())).unwrap();
    let (body, heads): (String, String) = (body.to_string(), headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect());
    write!(s, "POST {path} HTTP/1.1\r\nHost: mock\r\nContent-Length: {}\r\n{heads}\r\n{body}", body.len()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let datas =
        out.lines().filter_map(|l| l.strip_prefix("data: ")).chain(out.split("\r\n\r\n").nth(1).filter(|b| b.starts_with('{'))).filter_map(|d| serde_json::from_str(d).ok());
    (status, datas.collect())
}

/// The content an Anthropic stream's events build.
fn content(events: &[Value]) -> Vec<Value> {
    let mut blocks: Vec<(Value, String)> = Vec::new();
    for e in events {
        let (i, d) = (e["index"].as_u64().unwrap_or(0) as usize, &e["delta"]);
        match e["type"].as_str().unwrap() {
            "content_block_start" => blocks.push((e["content_block"].clone(), String::new())),
            "content_block_delta" if d["type"] == "input_json_delta" => blocks[i].1 += d["partial_json"].as_str().unwrap(),
            "content_block_delta" => {
                let k = d["type"].as_str().unwrap().trim_end_matches("_delta");
                blocks[i].0[k] = format!("{}{}", blocks[i].0[k].as_str().unwrap_or(""), d[k].as_str().unwrap()).into();
            }
            _ => {}
        }
    }
    let parsed = |(mut b, input): (Value, String)| {
        if b["type"] == "tool_use" {
            b["input"] = serde_json::from_str(&input).unwrap();
        }
        b
    };
    blocks.into_iter().map(parsed).collect()
}

/// The mock refuses the request shapes below as the API refuses them, so every Anthropic test checks henki's requests
/// against them. Text-only assistant content is not compared on replay: the API binds only thinking blocks to what it
/// sent, and a reply cut short is replayed as its partial text.
#[test]
fn mock_anthropic_refuses_like_the_api() {
    let m = Mock::start_api(Api::Anthropic, vec![thinking("hmm", shell("ls")), say("ok"), say("ok")]);
    let v = [("anthropic-version", "2023-06-01")];
    let user = |t: &str| json!({"role": "user", "content": [{"type": "text", "text": t}]});
    let req = |system: &str, msgs: Vec<Value>| json!({"model": "m", "max_tokens": 64, "system": [{"type": "text", "text": system}], "messages": msgs, "stream": true});
    let (status, events) = post(&m, "/v1/messages", &v, &req("sys", vec![user("hi")]));
    assert_eq!((status, events[0]["type"].as_str(), events[1]["type"].as_str()), (200, Some("message_start"), Some("ping")));
    let reply = content(&events);
    let id = reply[1]["id"].as_str().unwrap().to_string();
    let result = json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "a\n", "is_error": false}]});
    let turn = |reply: &[Value]| vec![user("hi"), json!({"role": "assistant", "content": reply}), result.clone()];
    let late = json!({"role": "user", "content": [{"type": "text", "text": "and then"}, result["content"][0]]});
    let mut changed = reply.clone();
    changed[0]["thinking"] = json!("hmm, no");
    let mut renamed = reply.clone();
    renamed[1]["input"] = json!({"command": "rm -rf ~"});
    let refused = [
        (&[][..], req("sys", vec![user("hi")]), "anthropic-version: header is required"),
        (&v[..], json!({"model": "m", "messages": [user("hi")]}), "max_tokens: Field required"),
        (&v[..], json!({"model": "m", "max_tokens": 9, "messages": [user("hi")], "thinking": {"type": "enabled"}}), "thinking: Extra inputs are not permitted"),
        (&v[..], json!({"model": "m", "max_tokens": 9, "messages": [user("hi")], "temperature": 0}), "temperature: Extra inputs are not permitted"),
        (&v[..], req("sys", vec![user("hi"), user("again")]), "messages.1: roles must alternate"),
        (&v[..], req("sys", vec![user("")]), "messages.0.content.0.text: text content blocks must be non-empty"),
        (&v[..], req("sys", vec![user(" \n\t")]), "messages.0.content.0.text: text content blocks must contain non-whitespace text"),
        (&v[..], req("sys", vec![user("hi"), json!({"role": "assistant", "content": reply}), late]), "messages.2.content.1: tool_result blocks must come first"),
        (&v[..], req("sys", turn(&reply)[..2].to_vec()), "`tool_use` ids were found without `tool_result` blocks"),
        (&v[..], req("sys", vec![user("hi"), json!({"role": "assistant", "content": [{"type": "text", "text": "x"}]}), result.clone()]), "unexpected `tool_use_id`"),
        (&v[..], req("sys", turn(&changed)), "the assistant content was changed"),
        (&v[..], req("sys", turn(&renamed)), "the assistant content was changed"),
        (&v[..], req("another system", turn(&reply)), "Invalid signature in thinking block: it is bound to a different conversation"),
    ];
    for (headers, body, says) in refused {
        let (status, err) = post(&m, "/v1/messages", headers, &body);
        assert_eq!((status, &err[0]["error"]["type"]), (400, &json!("invalid_request_error")), "{body}");
        assert!(err[0]["error"]["message"].as_str().unwrap().contains(says), "{says:?}: {}", err[0]);
    }
    assert_eq!(post(&m, "/v1/messages", &v, &req("sys", turn(&reply))).0, 200, "the unchanged replay is accepted");
    assert_eq!(post(&m, "/v1/chat/completions", &v, &req("sys", vec![user("hi")])).0, 404);
    let m = Mock::start_api(Api::Anthropic, vec![say("ok")]).standin("cred-managed");
    let (status, err) = post(&m, "/model/v1/messages", &v, &req("sys", vec![user("hi")]));
    assert!(status == 401 && err[0]["error"]["message"].as_str().unwrap().starts_with("blocked:no-standin"), "{err:?}");
    assert_eq!(post(&m, "/model/v1/messages", &[v[0], ("x-api-key", "cred-managed")], &req("sys", vec![user("hi")])).0, 200);
}

/// A reply whose text before its call is whitespace alone: the API refuses such a block in a request, so the next
/// request replays the reply without it.
#[test]
fn e2e_anthropic_whitespace_text_dropped() {
    let reply = Step::Stream { parts: vec![say("\n\n"), shell("echo ws")], finish: Some("tool_calls".into()), done: true, hold_ms: 0 };
    let (r, m) = scripted(vec![reply, say("done")]);
    let d = r.json("go");
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m);
    let replayed = &m.log()[1].messages()[1];
    assert_eq!((&replayed["role"], replayed["content"].as_array().map(Vec::len)), (&json!("assistant"), Some(1)), "{replayed}");
    assert_eq!((&replayed["content"][0]["type"], &replayed["content"][0]["input"]), (&json!("tool_use"), &json!({"command": "echo ws"})), "{replayed}");
}

/// A reply of `shell` calls, one tool_use block per (id, command), the ids as a gateway sent them: empty, or one already
/// used.
fn tool_uses(calls: &[(&str, &str)]) -> Step {
    let msg = json!({"id": "msg_ids", "type": "message", "role": "assistant", "content": [], "model": "mock-model", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 10, "output_tokens": 1}});
    let mut events = vec![("message_start".to_string(), json!({"type": "message_start", "message": msg}))];
    for (i, (id, cmd)) in calls.iter().enumerate() {
        let input = json!({"command": cmd}).to_string();
        events.push((
            "content_block_start".into(),
            json!({"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": id, "name": "shell", "input": {}}}),
        ));
        events.push(("content_block_delta".into(), json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": input}})));
        events.push(("content_block_stop".into(), json!({"type": "content_block_stop", "index": i})));
    }
    events.push(("message_delta".into(), json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 5}})));
    events.push(("message_stop".into(), json!({"type": "message_stop"})));
    Step::Events(events)
}

/// The tool_use ids and the tool_result ids of a request, in order.
fn paired_ids(l: &Logged) -> (Vec<String>, Vec<String>) {
    let blocks: Vec<Value> = l.messages().iter().flat_map(|m| m["content"].as_array().unwrap().clone()).collect();
    let ids = |ty: &str, k: &str| blocks.iter().filter(|b| b["type"] == ty).map(|b| b[k].as_str().unwrap().to_string()).collect();
    (ids("tool_use", "id"), ids("tool_result", "tool_use_id"))
}

/// Empty tool_use ids, and an id a later reply uses again: henki names those calls itself, and the replayed tool_use
/// blocks carry the same names as the tool_results, so the API (which refuses unpaired ids) takes every request, and a
/// resumed session's too.
#[test]
fn e2e_anthropic_duplicate_tool_use_ids() {
    // The mock keeps what it sent by id across tests; an id of this test's own.
    let reused = format!("toolu_reused_{}", std::process::id());
    let again = tool_uses(&[(&reused, "echo three")]);
    let (mut r, m) = scripted(vec![tool_uses(&[("", "echo one"), ("", "echo two")]), again.clone(), again, say("all paired")]);
    let d = r.json("run them");
    d.finish();
    assert_eq!((d.code, d.result()["text"].as_str()), (Some(0), Some("all paired")), "{}", d.diag());
    clean(&m);
    let (uses, results) = paired_ids(&m.log()[3]);
    assert_eq!(uses, results, "every tool_use answered by a tool_result of its id");
    let distinct: std::collections::HashSet<&String> = uses.iter().collect();
    assert_eq!((uses.len(), distinct.len(), uses[2] == reused, uses.iter().all(|u| !u.is_empty())), (4, 4, true, true), "{uses:?}");
    let m2 = Mock::start_api(Api::Anthropic, vec![say("resumed")]);
    at(&mut r, &m2);
    let d = r.cmd(&["--json", "--resume", "go on"]).go();
    d.finish();
    assert_eq!(d.code, Some(0), "{}", d.diag());
    clean(&m2);
    assert_eq!(paired_ids(&m2.log()[0]).0, uses, "the journal replays the same ids");
}
