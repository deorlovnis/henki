#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use crate::types::TOOLS;

fn user(content: &str) -> Value {
    json!({"msg": {"content": content, "role": "user"}, "t": "user", "ts": "2026-01-02T03:04:05Z"})
}

/// An assistant record as the journal writes it for this dialect: the calls, and the blocks as received.
fn asst(text: &str, calls: Value, blocks: Value) -> Value {
    json!({"blocks": blocks, "finish": "tool_calls", "msg": {"content": text, "role": "assistant", "tool_calls": calls}, "t": "assistant",
        "ts": "2026-01-02T03:04:06Z", "usage": {"cached": 0, "in": 10, "out": 5}})
}

fn result(id: &str, content: &str) -> Value {
    json!({"bytes": 3, "content": content, "id": id, "is_error": false, "ms": 400, "name": "shell", "t": "tool_result", "ts": "2026-01-02T03:04:07Z"})
}

/// A task, a reply with thinking, text and a call, its result, and the next task.
fn hist4() -> Vec<Value> {
    let call = json!([{"args": {"command": "itse --root . status"}, "id": "toolu_1", "name": "shell"}]);
    let blocks = json!([{"signature": "EqQBCkYI", "thinking": "", "type": "thinking"}, {"text": "Checking.", "type": "text"},
        {"id": "toolu_1", "input": {"command": "itse --root . status"}, "name": "shell", "type": "tool_use"}]);
    vec![user("check the build"), asst("Checking.", call, blocks), result("toolu_1", "ok\n[exit 0, 0.4 s]"), user("now fix it")]
}

fn render(hist: &[Value]) -> String {
    String::from_utf8(body("m", 4096, "SYS", &json!([]), hist)).unwrap()
}

/// The `messages` of a rendered body.
fn messages(hist: &[Value]) -> Vec<Value> {
    serde_json::from_str::<Value>(&render(hist)).unwrap()["messages"].as_array().unwrap().clone()
}

fn start(i: u64, block: Value) -> String {
    json!({"content_block": block, "index": i, "type": "content_block_start"}).to_string()
}

fn delta(i: u64, d: Value) -> String {
    json!({"delta": d, "index": i, "type": "content_block_delta"}).to_string()
}

fn text(i: u64, t: &str) -> String {
    delta(i, json!({"text": t, "type": "text_delta"}))
}

fn input(i: u64, json: &str) -> String {
    delta(i, json!({"partial_json": json, "type": "input_json_delta"}))
}

fn tool(i: u64, id: &str, name: &str) -> String {
    start(i, json!({"id": id, "input": {}, "name": name, "type": "tool_use"}))
}

fn stop(reason: &str) -> String {
    json!({"delta": {"stop_reason": reason, "stop_sequence": null}, "type": "message_delta", "usage": {"output_tokens": 7}}).to_string()
}

const MESSAGE_STOP: &str = r#"{"type":"message_stop"}"#;
const PING: &str = r#"{"type": "ping"}"#;

/// Feeds every payload, then finishes complete; the reply and the streamed parts.
fn run(data: &[String]) -> (Result<Reply, ProviderError>, Vec<Part>) {
    let mut acc = Acc::new(&TOOLS);
    let mut parts = vec![];
    for d in data {
        acc.data(d, &mut |p| parts.push(p)).unwrap();
    }
    (acc.finish(true), parts)
}

fn reply(data: &[String]) -> Reply {
    run(data).0.unwrap()
}

fn one_call(stop_reason: &str) -> Reply {
    reply(&[tool(0, "toolu_1", "shell"), input(0, r#"{"command": "ls"}"#), stop(stop_reason)])
}

#[test]
fn render_anthropic_golden() {
    let tools = json!([{"function": {"description": "d", "name": "shell", "parameters": {"type": "object"}}, "type": "function"}]);
    let got = String::from_utf8(body("claude-opus-5-5", 32768, "SYS", &tools, &hist4())).unwrap();
    let want = r#"{"cache_control":{"type":"ephemeral"},"max_tokens":32768,"messages":[{"content":[{"text":"check the build","type":"text"}],"role":"user"},{"content":[{"signature":"EqQBCkYI","thinking":"","type":"thinking"},{"text":"Checking.","type":"text"},{"id":"toolu_1","input":{"command":"itse --root . status"},"name":"shell","type":"tool_use"}],"role":"assistant"},{"content":[{"content":"ok\n[exit 0, 0.4 s]","is_error":false,"tool_use_id":"toolu_1","type":"tool_result"},{"text":"now fix it","type":"text"}],"role":"user"}],"model":"claude-opus-5-5","stream":true,"system":[{"cache_control":{"type":"ephemeral"},"text":"SYS","type":"text"}],"tools":[{"description":"d","input_schema":{"type":"object"},"name":"shell"}]}"#;
    assert_eq!(got, want);
}

#[test]
fn render_tools_input_schema() {
    let got = String::from_utf8(body("m", 1, "SYS", &crate::tools::schema(&TOOLS), &[])).unwrap();
    let tools = got.split_once(r#","tools":"#).unwrap().1.strip_suffix('}').unwrap();
    assert_eq!(tools.len(), 988, "{tools}");
    let v: Value = serde_json::from_str(tools).unwrap();
    let names: Vec<&str> = v.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOLS);
    for (t, f) in v.as_array().unwrap().iter().zip(crate::tools::schema(&TOOLS).as_array().unwrap()) {
        assert_eq!(t, &json!({"description": f["function"]["description"], "input_schema": f["function"]["parameters"], "name": f["function"]["name"]}));
    }
}

#[test]
fn render_blocks_unchanged() {
    let blocks = json!([{"signature": "c2ln", "thinking": "", "type": "thinking"}, {"data": "EmwKAhgB", "type": "redacted_thinking"},
        {"text": "ok", "type": "text", "citations": null}, {"type": "server_thing", "x": [1, {"b": 2, "a": 1.5}]},
        {"id": "toolu_9", "input": {"z": 1, "a": "é\u{1b}"}, "name": "shell", "type": "tool_use", "caller": {"type": "direct"}}]);
    let hist = vec![user("a"), asst("ok", json!([{"args": {}, "id": "toolu_9", "name": "shell"}]), blocks.clone()), result("toolu_9", "x")];
    assert_eq!(messages(&hist)[1], json!({"content": blocks, "role": "assistant"}));
    // Old messages are byte-identical when the history grows.
    let (short, long) = (render(&hist), render(&[hist.clone(), vec![user("b"), asst("", json!([]), json!([{"text": "done", "type": "text"}]))]].concat()));
    let replayed = format!(r#"{{"content":{},"role":"assistant"}}"#, serde_json::to_string(&blocks).unwrap());
    assert!(short.contains(&replayed) && long.contains(&replayed), "{short}");
    assert_eq!(short.split_once(&replayed).unwrap().0, long.split_once(&replayed).unwrap().0);
}

#[test]
fn render_no_empty_text() {
    let blocks = json!([{"text": "", "type": "text"}, {"type": "text"}, {"signature": "s", "thinking": "", "type": "thinking"}, {"text": " ", "type": "text"}]);
    let hist = vec![user("a"), asst("", json!([]), blocks), user("")];
    let m = messages(&hist);
    assert_eq!(m.len(), 2);
    assert_eq!(m[1]["content"], json!([{"signature": "s", "thinking": "", "type": "thinking"}]));
    assert!(!render(&hist).contains(r#""text":"""#));
}

/// A text block of whitespace alone is refused by the API, so it is never sent: not from a reply's blocks, a partial
/// reply or a user record. Text around other text keeps every byte of its whitespace.
#[test]
fn render_whitespace_text_dropped() {
    let call = json!([{"args": {"command": "ls"}, "id": "t1", "name": "shell"}]);
    let blocks = json!([{"text": "\n\n", "type": "text"}, {"id": "t1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"}, {"text": " \t\u{a0}", "type": "text"}]);
    let partial = json!({"finish": "interrupted", "msg": {"content": " \n", "role": "assistant", "tool_calls": []}, "t": "assistant"});
    let m = messages(&[user("a"), asst("\n\n", call, blocks), result("t1", "x"), user(" \n "), partial, user("\n next \n")]);
    assert_eq!(
        m,
        [
            json!({"content": [{"text": "a", "type": "text"}], "role": "user"}),
            json!({"content": [{"id": "t1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"}], "role": "assistant"}),
            json!({"content": [{"content": "x", "is_error": false, "tool_use_id": "t1", "type": "tool_result"}, {"text": "\n next \n", "type": "text"}], "role": "user"}),
        ]
    );
}

/// A dangling call repaired on resume answers its tool_use right after the replayed blocks, with the other results and
/// before the next task, in one user message.
#[test]
fn render_repair_result_after_blocks() {
    use crate::journal::rec;
    let calls = [
        Call { id: "t1".into(), name: "shell".into(), args: json!({"command": "a"}), ..Call::default() },
        Call { id: "t2".into(), name: "read".into(), args: json!({"path": "b"}), ..Call::default() },
    ];
    let blocks = json!([{"signature": "c2ln", "thinking": "", "type": "thinking"}, {"id": "t1", "input": {"command": "a"}, "name": "shell", "type": "tool_use"},
        {"id": "t2", "input": {"path": "b"}, "name": "read", "type": "tool_use"}]);
    let reply = rec::assistant("", &calls, "tool_calls", Usage::default(), Some(&blocks));
    let repair = rec::tool_result("t2", "read", crate::tools::REPAIR, true, 0, 0);
    let m = messages(&[rec::user("go"), reply, rec::tool_result("t1", "shell", "A", false, 1, 1), repair, rec::user("next")]);
    assert_eq!(m.len(), 3);
    assert_eq!(m[1], json!({"content": blocks, "role": "assistant"}));
    let want = json!([{"content": "A", "is_error": false, "tool_use_id": "t1", "type": "tool_result"},
        {"content": "interrupted; the command may have partly run", "is_error": true, "tool_use_id": "t2", "type": "tool_result"}, {"text": "next", "type": "text"}]);
    assert_eq!((&m[2]["content"], &m[2]["role"]), (&want, &json!("user")));
}

#[test]
fn render_merge_same_role() {
    let a = |t: &str| asst(t, json!([]), json!([{"text": t, "type": "text"}]));
    let m = messages(&[user("a"), user("b"), a("c"), a("d"), user("e")]);
    assert_eq!(
        m,
        [
            json!({"content": [{"text": "a", "type": "text"}, {"text": "b", "type": "text"}], "role": "user"}),
            json!({"content": [{"text": "c", "type": "text"}, {"text": "d", "type": "text"}], "role": "assistant"}),
            json!({"content": [{"text": "e", "type": "text"}], "role": "user"}),
        ]
    );
}

#[test]
fn render_results_before_text() {
    let calls = json!([{"args": {"command": "a"}, "id": "t1", "name": "shell"}, {"args": {"command": "b"}, "id": "t2", "name": "shell"}]);
    let blocks =
        json!([{"id": "t1", "input": {"command": "a"}, "name": "shell", "type": "tool_use"}, {"id": "t2", "input": {"command": "b"}, "name": "shell", "type": "tool_use"}]);
    let m = messages(&[user("go"), asst("", calls, blocks), result("t1", "A"), result("t2", "B"), user("next")]);
    assert_eq!(m.len(), 3);
    let kinds: Vec<(&str, &str)> = m[2]["content"].as_array().unwrap().iter().map(|b| (b["type"].as_str().unwrap(), b["tool_use_id"].as_str().unwrap_or(""))).collect();
    assert_eq!(kinds, [("tool_result", "t1"), ("tool_result", "t2"), ("text", "")]);
    assert_eq!(m[2]["role"], "user");
}

#[test]
fn render_drop_empty_assistant() {
    let partial = json!({"msg": {"content": "", "role": "assistant", "tool_calls": []}, "t": "assistant"});
    let m = messages(&[user("a"), partial, user("b"), asst("", json!([]), json!([])), user("c")]);
    assert_eq!(m, [json!({"content": [{"text": "a", "type": "text"}, {"text": "b", "type": "text"}, {"text": "c", "type": "text"}], "role": "user"})]);
}

#[test]
fn render_partial_assistant() {
    let calls = json!([{"args": {"command": "ls"}, "id": "h1", "name": "shell"}, {"args": "not an object", "id": "h2", "name": "read", "raw": "x"}]);
    let partial = json!({"finish": "interrupted", "msg": {"content": "half an ans", "role": "assistant", "tool_calls": calls}, "t": "assistant"});
    let m = messages(&[user("a"), partial]);
    assert_eq!(
        m[1]["content"],
        json!([{"text": "half an ans", "type": "text"}, {"id": "h1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"},
            {"id": "h2", "input": {}, "name": "read", "type": "tool_use"}])
    );
}

#[test]
fn render_tools_omitted_when_empty() {
    for tools in [json!([]), Value::Null] {
        let v: Value = serde_json::from_slice(&body("m", 1, "SYS", &tools, &[user("a")])).unwrap();
        assert!(v.get("tools").is_none(), "{tools}");
    }
}

#[test]
fn render_no_thinking_param() {
    let got = String::from_utf8(body("m", 1, "SYS", &crate::tools::schema(&TOOLS), &hist4())).unwrap();
    let v: Value = serde_json::from_str(&got).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["cache_control", "max_tokens", "messages", "model", "stream", "system", "tools"]);
    for absent in ["chat_template_kwargs", "eager_input_streaming", "temperature", "top_p", "top_k", "stream_options", "\"role\":\"system\"", "tool_calls"] {
        assert!(!got.contains(absent), "{absent}");
    }
}

#[test]
fn headers_key_and_version() {
    assert_eq!(headers("cred-managed"), [("anthropic-version", "2023-06-01".to_string()), ("x-api-key", "cred-managed".to_string())]);
}

#[test]
fn headers_no_key() {
    assert_eq!(headers(""), [("anthropic-version", "2023-06-01".to_string())]);
}

#[test]
fn parse_text_stream() {
    let begin =
        json!({"message": {"content": [], "id": "msg_1", "role": "assistant", "type": "message", "usage": {"input_tokens": 12, "output_tokens": 1}}, "type": "message_start"});
    let events = [
        begin.to_string(),
        start(0, json!({"text": "", "type": "text"})),
        PING.into(),
        text(0, "Hello"),
        text(0, " world"),
        r#"{"index":0,"type":"content_block_stop"}"#.into(),
        stop("end_turn"),
    ];
    let mut acc = Acc::new(&TOOLS);
    let flows: Vec<Flow> = events.iter().map(|e| acc.data(e, &mut |_| {}).unwrap()).collect();
    assert_eq!(flows, [Flow::More, Flow::More, Flow::More, Flow::More, Flow::More, Flow::More, Flow::Finished]);
    assert_eq!(acc.data(PING, &mut |_| {}).unwrap(), Flow::Finished);
    assert_eq!(acc.data(MESSAGE_STOP, &mut |_| {}).unwrap(), Flow::Done);
    let r = acc.finish(true).unwrap();
    let want = Reply {
        text: "Hello world".into(),
        finish: "stop".into(),
        usage: Usage { input: 12, output: 7, cached: 0 },
        blocks: Some(json!([{"text": "Hello world", "type": "text"}])),
        ..Reply::default()
    };
    assert_eq!(r, want);
}

#[test]
fn parse_thinking_signature() {
    let r = reply(&[
        start(0, json!({"signature": "", "thinking": "", "type": "thinking"})),
        delta(0, json!({"thinking": "Let me", "type": "thinking_delta"})),
        delta(0, json!({"thinking": " think", "type": "thinking_delta"})),
        delta(0, json!({"signature": "EqQB", "type": "signature_delta"})),
        delta(0, json!({"signature": "CkYI", "type": "signature_delta"})),
        // Display omitted: an empty thinking block whose signature carries the reasoning; it is kept.
        start(1, json!({"thinking": "", "type": "thinking"})),
        delta(1, json!({"signature": "c2ln", "type": "signature_delta"})),
        start(2, json!({"text": "", "type": "text"})),
        text(2, "Done."),
        stop("end_turn"),
    ]);
    assert_eq!((r.reasoning.as_str(), r.text.as_str()), ("Let me think", "Done."));
    let want = json!([{"signature": "EqQBCkYI", "thinking": "Let me think", "type": "thinking"}, {"signature": "c2ln", "thinking": "", "type": "thinking"}, {"text": "Done.", "type": "text"}]);
    assert_eq!(r.blocks, Some(want));
}

#[test]
fn parse_redacted_kept() {
    let r = reply(&[start(0, json!({"data": "EmwKAhgBEgy3va3pzix", "type": "redacted_thinking"})), start(1, json!({"text": "hi", "type": "text"})), stop("end_turn")]);
    assert_eq!(r.blocks, Some(json!([{"data": "EmwKAhgBEgy3va3pzix", "type": "redacted_thinking"}, {"text": "hi", "type": "text"}])));
    assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("hi", ""));
}

#[test]
fn parse_unknown_block_kept() {
    let odd = json!({"id": "srv_1", "input": {"q": "x"}, "name": "web", "type": "server_tool_use"});
    let r = reply(&[start(3, odd.clone()), delta(3, json!({"type": "citations_delta", "citation": {}})), delta(3, json!({"x": 1})), stop("end_turn")]);
    assert_eq!(r.blocks, Some(json!([odd])));
    assert!(r.calls.is_empty() && r.text.is_empty());
    assert_eq!(r.finish, "stop");
}

#[test]
fn parse_tool_use_fragments() {
    let r = reply(&[
        start(0, json!({"text": "", "type": "text"})),
        text(0, "Looking."),
        tool(1, "toolu_1", "shell"),
        input(1, ""),
        input(1, r#"{"comm"#),
        input(1, r#"and": "ls -la"}"#),
        tool(2, "toolu_2", "read"),
        input(2, r#"{"path": "a.txt", "limit": 5}"#),
        stop("tool_use"),
    ]);
    assert_eq!(
        r.calls,
        [
            Call { id: "toolu_1".into(), name: "shell".into(), args: json!({"command": "ls -la"}), ..Call::default() },
            Call { id: "toolu_2".into(), name: "read".into(), args: json!({"limit": 5, "path": "a.txt"}), ..Call::default() },
        ]
    );
    assert_eq!((r.text.as_str(), r.finish.as_str()), ("Looking.", "tool_calls"));
    let blocks = r.blocks.unwrap();
    assert_eq!(blocks[1], json!({"id": "toolu_1", "input": {"command": "ls -la"}, "name": "shell", "type": "tool_use"}));
    assert_eq!(blocks[2]["input"], json!({"limit": 5, "path": "a.txt"}));
}

#[test]
fn parse_input_empty_is_object() {
    let r = reply(&[tool(0, "toolu_1", "shell"), stop("tool_use")]);
    assert_eq!(r.calls, [Call { id: "toolu_1".into(), name: "shell".into(), args: json!({}), ..Call::default() }]);
    assert_eq!(r.blocks.unwrap()[0]["input"], json!({}));
    // A start block without an input, or with one that is not an object, gives `{}` as well.
    for odd in [json!({"id": "t1", "name": "shell", "type": "tool_use"}), json!({"id": "t1", "input": "ls", "name": "shell", "type": "tool_use"})] {
        let r = reply(&[start(0, odd), stop("tool_use")]);
        assert_eq!((&r.calls[0].args, r.calls[0].raw.as_ref(), &r.blocks.unwrap()[0]["input"]), (&json!({}), None, &json!({})));
    }
}

/// A proxy that streams a finished reply may send a tool_use input whole in its start block, with no deltas: the call
/// gets it. Deltas, when they come, are the input.
#[test]
fn parse_input_in_start_kept() {
    let whole = json!({"id": "t1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"});
    let r = reply(&[start(0, whole.clone()), stop("tool_use")]);
    assert_eq!(r.calls, [Call { id: "t1".into(), name: "shell".into(), args: json!({"command": "ls"}), ..Call::default() }]);
    assert_eq!(r.blocks, Some(json!([whole])));
    let placeholder = json!({"id": "t1", "input": {"command": "start"}, "name": "shell", "type": "tool_use"});
    let r = reply(&[start(0, placeholder), input(0, r#"{"command": "delta"}"#), stop("tool_use")]);
    assert_eq!((&r.calls[0].args, &r.blocks.unwrap()[0]["input"]), (&json!({"command": "delta"}), &json!({"command": "delta"})));
}

#[test]
fn parse_input_invalid_raw() {
    let r = reply(&[tool(0, "t1", "shell"), input(0, r#"{"command": "#), tool(1, "t2", "shell"), input(1, "[1]"), stop("tool_use")]);
    assert_eq!((&r.calls[0].args, r.calls[0].raw.as_deref()), (&json!({}), Some(r#"{"command": "#)));
    assert_eq!((&r.calls[1].args, r.calls[1].raw.as_deref()), (&json!([1]), None), "JSON that is not an object is left to validation");
    let blocks = r.blocks.unwrap();
    assert_eq!((&blocks[0]["input"], &blocks[1]["input"]), (&json!({}), &json!({})), "the replayed input is always an object");
}

#[test]
fn parse_input_over_1mib() {
    let fits = format!(r#"{{"command": "{}"}}"#, "x".repeat(ARGS_MAX - 15));
    assert_eq!(fits.len(), ARGS_MAX);
    let r = reply(&[tool(0, "t1", "write"), input(0, &fits), stop("tool_use")]);
    assert_eq!((r.calls[0].args["command"].as_str().unwrap().len(), r.calls[0].raw.as_ref()), (ARGS_MAX - 15, None));
    let over = format!(r#"{{"command": "{}"}}"#, "x".repeat(ARGS_MAX - 14));
    let r = reply(&[tool(0, "t1", "write"), input(0, &over), stop("tool_use")]);
    assert_eq!((&r.calls[0].args, r.calls[0].raw.as_ref()), (&json!({}), Some(&over)));
    assert_eq!(r.blocks.unwrap()[0]["input"], json!({}));
}

#[test]
fn parse_usage_cache_sum() {
    let begin =
        json!({"message": {"usage": {"cache_creation_input_tokens": 200, "cache_read_input_tokens": 1000, "input_tokens": 10, "output_tokens": 1}}, "type": "message_start"});
    let r = reply(&[begin.to_string(), stop("end_turn")]);
    assert_eq!(r.usage, Usage { input: 1210, output: 7, cached: 1000 });
    // Without cache fields, input is the input tokens; huge counts saturate instead of overflowing.
    let r = reply(&[json!({"message": {"usage": {"input_tokens": 5}}, "type": "message_start"}).to_string()]);
    assert_eq!(r.usage, Usage { input: 5, output: 0, cached: 0 });
    let max = u64::MAX;
    let huge = json!({"message": {"usage": {"cache_creation_input_tokens": max, "cache_read_input_tokens": max, "input_tokens": max}}, "type": "message_start"});
    assert_eq!(reply(&[huge.to_string()]).usage, Usage { input: max, output: 0, cached: max });
}

#[test]
fn parse_message_delta_usage() {
    let begin = json!({"message": {"usage": {"cache_read_input_tokens": 100, "input_tokens": 10, "output_tokens": 1}}, "type": "message_start"}).to_string();
    // The final counts in message_delta replace the first ones; each field it lacks keeps its value.
    let cumulative = json!({"delta": {"stop_reason": "end_turn"}, "type": "message_delta",
        "usage": {"cache_creation_input_tokens": 5, "cache_read_input_tokens": 100, "input_tokens": 20, "output_tokens": 300}});
    assert_eq!(reply(&[begin.clone(), cumulative.to_string()]).usage, Usage { input: 125, output: 300, cached: 100 });
    assert_eq!(reply(&[begin.clone(), stop("end_turn")]).usage, Usage { input: 110, output: 7, cached: 100 });
    let bare = json!({"delta": {"stop_reason": "end_turn"}, "type": "message_delta"});
    assert_eq!(reply(&[begin, bare.to_string()]).usage, Usage { input: 110, output: 1, cached: 100 });
}

/// A gateway that sends only some of the counts in message_delta keeps the cache counts message_start gave.
#[test]
fn parse_message_delta_usage_partial_cache_fields() {
    let begin = json!({"message": {"usage": {"cache_creation_input_tokens": 3, "cache_read_input_tokens": 100, "input_tokens": 10}}, "type": "message_start"}).to_string();
    let partial = json!({"delta": {"stop_reason": "end_turn"}, "type": "message_delta", "usage": {"input_tokens": 10, "output_tokens": 5}});
    assert_eq!(reply(&[begin.clone(), partial.to_string()]).usage, Usage { input: 113, output: 5, cached: 100 });
    // A null or negative count is no count: the one before stays.
    let nulls = json!({"delta": {"stop_reason": "end_turn"}, "type": "message_delta",
        "usage": {"cache_creation_input_tokens": null, "cache_read_input_tokens": -1, "input_tokens": 20, "output_tokens": 6}});
    assert_eq!(reply(&[begin.clone(), nulls.to_string()]).usage, Usage { input: 123, output: 6, cached: 100 });
    let only_cache = json!({"message": {"usage": {"cache_read_input_tokens": 50}}, "type": "message_start"}).to_string();
    assert_eq!(reply(&[only_cache]).usage, Usage { input: 50, output: 0, cached: 50 });
}

#[test]
fn parse_stop_reasons() {
    let answer = |reason: &str| reply(&[text_block("an answer"), stop(reason)]).finish;
    assert_eq!(answer("end_turn"), "stop");
    assert_eq!(one_call("end_turn").finish, "tool_calls");
    let r = one_call("tool_use");
    assert_eq!((r.finish.as_str(), r.calls[0].error.as_ref()), ("tool_calls", None));
    for cut in ["max_tokens", "model_context_window_exceeded"] {
        let r = one_call(cut);
        assert_eq!((r.finish.as_str(), r.calls[0].error.as_deref()), ("length", Some(CUT_ARGS)), "{cut}");
        assert_eq!(answer(cut), "length");
    }
    assert_eq!(CUT_ARGS, "arguments cut at max_tokens; not run. Send a shorter call.");
    for other in ["pause_turn", "stop_sequence", "something_new", ""] {
        assert_eq!((answer(other).as_str(), one_call(other).finish.as_str()), ("stop", "tool_calls"), "{other}");
    }
    // message_stop without message_delta still completes the reply.
    let mut acc = Acc::new(&TOOLS);
    acc.data(&text_block("x"), &mut |_| {}).unwrap();
    assert_eq!(acc.data(MESSAGE_STOP, &mut |_| {}).unwrap(), Flow::Done);
    assert_eq!(acc.finish(true).unwrap().finish, "stop");
}

/// A reply with calls that ends by message_stop alone (no message_delta, so no stop reason) runs its calls.
#[test]
fn parse_message_stop_without_delta_with_calls() {
    let mut acc = Acc::new(&TOOLS);
    for d in [text_block("Listing."), tool(1, "t1", "shell"), input(1, r#"{"command": "ls"}"#)] {
        assert_eq!(acc.data(&d, &mut |_| {}).unwrap(), Flow::More);
    }
    assert_eq!(acc.data(MESSAGE_STOP, &mut |_| {}).unwrap(), Flow::Done);
    let r = acc.finish(true).unwrap();
    assert_eq!((r.finish.as_str(), r.text.as_str()), ("tool_calls", "Listing."));
    assert_eq!(r.calls, [Call { id: "t1".into(), name: "shell".into(), args: json!({"command": "ls"}), ..Call::default() }]);
}

fn text_block(t: &str) -> String {
    start(0, json!({"text": t, "type": "text"}))
}

#[test]
fn parse_refusal() {
    let e = run(&[text_block("I can"), stop("refusal")]).0.unwrap_err();
    assert_eq!(e, perr("provider", "the model declined the request (refusal)"));
    assert_eq!(REFUSAL, "the model declined the request (refusal)");
}

#[test]
fn parse_error_first_retryable() {
    let overloaded = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let e = Acc::new(&TOOLS).data(overloaded, &mut |_| {}).err().unwrap();
    assert_eq!((e.kind, e.message.as_str(), e.retry.as_deref()), ("provider", "Overloaded", Some("overloaded_error")));
    let auth = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let e = Acc::new(&TOOLS).data(auth, &mut |_| {}).err().unwrap();
    assert_eq!((e.kind, e.retry), ("auth", None));
}

const OVERLOADED: &str = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;

fn message_start() -> String {
    json!({"message": {"usage": {"input_tokens": 1}}, "type": "message_start"}).to_string()
}

/// The error event after `events`: its retry cause.
fn retry_after(events: &[String]) -> Option<String> {
    let mut acc = Acc::new(&TOOLS);
    for e in events {
        acc.data(e, &mut |_| {}).unwrap();
    }
    let e = acc.data(OVERLOADED, &mut |_| {}).err().unwrap();
    assert_eq!((e.kind, e.message.as_str()), ("provider", "Overloaded"));
    e.retry
}

/// Once a part has streamed (text, reasoning, a call's name), a retry would show it twice: an error is fatal.
#[test]
fn parse_error_later_fatal() {
    assert_eq!(retry_after(&[message_start(), start(0, json!({"text": "", "type": "text"})), text(0, "Hel")]), None);
    assert_eq!(retry_after(&[message_start(), tool(0, "t1", "shell")]), None);
    assert_eq!(retry_after(&[message_start(), start(0, json!({"thinking": "", "type": "thinking"})), delta(0, json!({"thinking": "hm", "type": "thinking_delta"}))]), None);
    assert_eq!(retry_after(&[start(0, json!({"text": "", "type": "text"})), text(0, "x"), stop("end_turn"), PING.into()]), None);
}

/// The API always opens a stream with message_start (and may ping): an error after those, or after a thinking block that
/// streams no text (display omitted), comes before anything was shown and is retryable.
#[test]
fn parse_error_after_ping_and_after_start() {
    let retry = Some("overloaded_error".to_string());
    assert_eq!(retry_after(&[PING.into()]), retry);
    assert_eq!(retry_after(&[message_start()]), retry);
    assert_eq!(retry_after(&[message_start(), PING.into()]), retry);
    let thinking = start(0, json!({"signature": "", "thinking": "", "type": "thinking"}));
    assert_eq!(retry_after(&[message_start(), thinking, delta(0, json!({"signature": "EqQB", "type": "signature_delta"})), PING.into()]), retry);
    // A text block that started with its text whole (no delta) showed nothing either.
    assert_eq!(retry_after(&[message_start(), text_block("unseen")]), retry);
}

#[test]
fn parse_ping_ignored() {
    let mut acc = Acc::new(&TOOLS);
    let mut parts = vec![];
    assert_eq!(acc.data(PING, &mut |p| parts.push(p)).unwrap(), Flow::More);
    acc.data(&text_block("a"), &mut |p| parts.push(p)).unwrap();
    assert_eq!(acc.data(PING, &mut |p| parts.push(p)).unwrap(), Flow::More);
    assert_eq!(acc.data(r#"{"type":"content_block_stop","index":0}"#, &mut |p| parts.push(p)).unwrap(), Flow::More);
    assert!(parts.is_empty());
    assert_eq!(acc.finish(true).unwrap().text, "a");
    // Pings before, between and after the other events change nothing.
    assert_eq!(reply(&[PING.into(), text_block("a"), PING.into(), stop("end_turn"), PING.into()]).blocks, Some(json!([{"text": "a", "type": "text"}])));
}

#[test]
fn parse_delta_without_start() {
    let mut acc = Acc::new(&TOOLS);
    acc.data(&text_block("a"), &mut |_| {}).unwrap();
    let e = acc.data(&text(3, "x"), &mut |_| {}).err().unwrap();
    assert_eq!((e.kind, e.retry), ("provider", None));
    assert!(e.message.starts_with("a delta for a content block that never started: {"), "{}", e.message);
    let no_index = json!({"delta": {"text": "x", "type": "text_delta"}, "type": "content_block_delta"}).to_string();
    assert!(Acc::new(&TOOLS).data(&no_index, &mut |_| {}).is_err());
    for bad_start in [json!({"content_block": {"type": "text"}, "type": "content_block_start"}), json!({"content_block": "text", "index": 0, "type": "content_block_start"})] {
        let e = Acc::new(&TOOLS).data(&bad_start.to_string(), &mut |_| {}).err().unwrap();
        assert!(e.message.starts_with("a content block start without an object or a new index: "), "{}", e.message);
    }
    // A payload that is not JSON is a provider error with its first 200 characters.
    let e = Acc::new(&TOOLS).data(&format!("{{not json{}", "x".repeat(300)), &mut |_| {}).err().unwrap();
    assert!(e.message.starts_with("stream data is not JSON (") && e.message.ends_with(&format!(": {{not json{}", "x".repeat(191))), "{}", e.message);
}

/// A delta of another block's type (a buggy gateway) changes nothing: no stray field in the replayed block, no stray
/// part, no input from a text block.
#[test]
fn parse_delta_type_mismatch_ignored() {
    let (r, parts) = run(&[tool(0, "t1", "shell"), text(0, "oops"), input(0, r#"{"command": "ls"}"#), stop("tool_use")]);
    let r = r.unwrap();
    assert_eq!((parts, r.text.as_str()), (vec![Part::Call("shell".into())], ""));
    assert_eq!(r.blocks, Some(json!([{"id": "t1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"}])));
    let (r, parts) = run(&[
        text_block("hi"),
        delta(0, json!({"signature": "abc", "type": "signature_delta"})),
        delta(0, json!({"thinking": "t", "type": "thinking_delta"})),
        input(0, r#"{"command": "ls"}"#),
        start(1, json!({"thinking": "", "type": "thinking"})),
        text(1, "not text"),
        input(1, "{}"),
        start(2, json!({"data": "d", "type": "redacted_thinking"})),
        delta(2, json!({"signature": "s", "type": "signature_delta"})),
        stop("end_turn"),
    ]);
    let r = r.unwrap();
    assert!(parts.is_empty(), "{parts:?}");
    assert_eq!((r.text.as_str(), r.reasoning.as_str(), r.calls.len()), ("hi", "", 0));
    assert_eq!(r.blocks, Some(json!([{"text": "hi", "type": "text"}, {"thinking": "", "type": "thinking"}, {"data": "d", "type": "redacted_thinking"}])));
}

/// A second start on an index already open is a provider error; what streamed before stays streamed.
#[test]
fn parse_block_restarted_same_index() {
    let mut acc = Acc::new(&TOOLS);
    let mut parts = vec![];
    for d in [start(0, json!({"text": "", "type": "text"})), text(0, "first")] {
        acc.data(&d, &mut |p| parts.push(p)).unwrap();
    }
    for again in [start(0, json!({"text": "", "type": "text"})), tool(0, "t1", "shell")] {
        let e = acc.data(&again, &mut |p| parts.push(p)).err().unwrap();
        assert_eq!((e.kind, e.retry), ("provider", None));
        assert!(e.message.starts_with("a content block start without an object or a new index: {"), "{}", e.message);
    }
    assert_eq!(parts, [Part::Text("first".into())]);
}

/// Whitespace alone streams and stays in the answer text, but its block is not kept for replay.
#[test]
fn parse_whitespace_text_block_dropped() {
    let (r, parts) = run(&[
        start(0, json!({"text": "", "type": "text"})),
        text(0, "\n\n"),
        tool(1, "t1", "shell"),
        input(1, "{}"),
        start(2, json!({"text": " Done. ", "type": "text"})),
        stop("tool_use"),
    ]);
    let r = r.unwrap();
    assert_eq!(parts, [Part::Text("\n\n".into()), Part::Call("shell".into())]);
    assert_eq!(r.text, "\n\n Done. ");
    assert_eq!(r.blocks, Some(json!([{"id": "t1", "input": {}, "name": "shell", "type": "tool_use"}, {"text": " Done. ", "type": "text"}])));
}

#[test]
fn parse_incomplete_names_calls() {
    let mut acc = Acc::new(&TOOLS);
    for d in [text_block("a"), tool(1, "t1", "shell"), input(1, "{\"comm"), tool(2, "t2", ""), stop("tool_use")] {
        acc.data(&d, &mut |_| {}).unwrap();
    }
    assert_eq!(acc.finish(false).unwrap_err(), perr("stream_incomplete", "stream ended early; dropped calls: shell, unknown"));
    let mut acc = Acc::new(&TOOLS);
    acc.data(&text_block("a"), &mut |_| {}).unwrap();
    assert_eq!(acc.finish(false).unwrap_err(), perr("stream_incomplete", "stream ended early"));
}

#[test]
fn parse_parts_emitted() {
    let (r, parts) = run(&[
        start(0, json!({"thinking": "", "type": "thinking"})),
        delta(0, json!({"thinking": "", "type": "thinking_delta"})),
        delta(0, json!({"thinking": "hmm", "type": "thinking_delta"})),
        delta(0, json!({"signature": "s", "type": "signature_delta"})),
        start(1, json!({"text": "", "type": "text"})),
        text(1, ""),
        text(1, "Hi"),
        tool(2, "t1", "shell"),
        input(2, "{}"),
        tool(3, "t2", ""),
        stop("tool_use"),
    ]);
    assert_eq!(parts, [Part::Reasoning("hmm".into()), Part::Text("Hi".into()), Part::Call("shell".into()), Part::Call("unknown".into())]);
    assert_eq!(r.unwrap().calls.len(), 2);
}

#[test]
fn json_refused() {
    let e = Acc::new(&TOOLS).json(br#"{"content":[],"stop_reason":"end_turn"}"#).unwrap_err();
    assert_eq!(e, perr("provider", "the model server answered without a stream"));
}

/// xorshift64: a fixed, dependency-free stream of numbers below `n`.
fn rng(seed: u64) -> impl FnMut(usize) -> usize {
    let mut x = seed;
    move |n| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    }
}

/// Random event sequences of every type with wrong, missing and extreme fields never panic `data` or `finish`; the
/// counters show the inputs reach each ending.
#[test]
fn acc_never_panics() {
    let mut next = rng(0x9e37_79b9_7f4a_7c15);
    let mut types = vec!["message_start", "message_delta", "message_delta", "message_stop", "error", "ping", "x"];
    types.extend(["content_block_start"; 5].into_iter().chain(["content_block_delta"; 6]));
    let reasons = ["null", "5", "\"end_turn\"", "\"max_tokens\"", "\"refusal\"", "\"tool_use\""];
    let deltas = ["text_delta", "thinking_delta", "signature_delta", "input_json_delta", "citations_delta", ""];
    let values = [
        "null",
        "0",
        "1",
        "2",
        "-1",
        "1.5",
        "18446744073709551615",
        "18446744073709551616",
        "true",
        "\"\"",
        "\"x\"",
        "\"{\\\"a\\\":1}\"",
        "\"{\"",
        "[]",
        "{}",
        "{\"type\":\"text\",\"text\":\"\"}",
        "{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"shell\",\"input\":{}}",
        "{\"type\":\"thinking\",\"thinking\":5}",
        "{\"type\":\"redacted_thinking\",\"data\":\"d\"}",
        "{\"input_tokens\":18446744073709551615,\"cache_read_input_tokens\":7}",
        "\"max_tokens\"",
        "\"refusal\"",
        "\"tool_use\"",
        "{\"type\":\"overloaded_error\",\"message\":\"m\"}",
    ];
    let keys = ["index", "index", "content_block", "content_block", "delta", "usage", "message", "error", "text", "partial_json", "stop_reason"];
    let (mut ok, mut failed, mut refused, mut calls) = (0, 0, 0, 0);
    for _ in 0..10_000 {
        let mut acc = Acc::new(&TOOLS);
        let mut alive = true;
        // Most streams open their blocks first, so that the deltas reach them.
        for i in 0..2 {
            if next(4) == 0 {
                continue;
            }
            let ty = ["text", "thinking", "tool_use"][next(3)];
            acc.data(&start(i, json!({"type": ty})), &mut |_| {}).unwrap();
        }
        for _ in 0..next(12) {
            let ty = types[next(types.len())];
            let mut fields = vec![format!("\"type\":\"{ty}\"")];
            fields.extend((0..next(4)).map(|_| format!("\"{}\":{}", keys[next(keys.len())], values[next(values.len())])));
            match ty {
                "content_block_delta" => {
                    let d = deltas[next(deltas.len())];
                    let k = ["text", "thinking", "signature", "partial_json"][next(4)];
                    fields.push(format!("\"index\":{},\"delta\":{{\"type\":\"{d}\",\"{k}\":{}}}", next(2), values[next(values.len())]));
                }
                // Mostly a new index (a repeated one is an error), which the deltas do not reach.
                "content_block_start" => fields.push(format!("\"index\":{},\"content_block\":{}", 2 + next(40), values[15 + next(5)])),
                "message_delta" => fields.push(format!("\"delta\":{{\"stop_reason\":{}}},\"usage\":{}", reasons[next(reasons.len())], values[next(values.len())])),
                _ => {}
            }
            // A repeated key: the last one wins. Now and then a payload that is not JSON at all.
            let payload = if next(50) == 0 { ["{", "", "[", "{\"type\":", "\u{0}"][next(5)].to_string() } else { format!("{{{}}}", fields.join(",")) };
            if acc.data(&payload, &mut |_| {}).is_err() {
                alive = false;
                failed += 1;
                break;
            }
        }
        if alive {
            match acc.finish(next(4) != 0) {
                Ok(r) => {
                    ok += 1;
                    calls += r.calls.len();
                    assert!(r.blocks.unwrap().as_array().unwrap().iter().all(|b| !empty_text(b)));
                }
                Err(e) if e.message == REFUSAL => refused += 1,
                Err(_) => failed += 1,
            }
        }
    }
    assert!(ok > 2000 && failed > 2000 && refused > 100 && calls > 1000, "ok {ok}, failed {failed}, refused {refused}, calls {calls}");
}

/// Random histories of record-like values (wrong types, missing fields, odd blocks) never panic `body`, and every body is
/// JSON whose messages alternate roles and hold no empty text block.
#[test]
fn body_never_panics() {
    let mut next = rng(0x2545_f491_4f6c_dd1d);
    let blocks = [
        json!({"text": "", "type": "text"}),
        json!({"text": "t", "type": "text"}),
        json!({"type": "text"}),
        json!({"signature": "s", "thinking": "", "type": "thinking"}),
        json!({"id": "t1", "input": {}, "name": "shell", "type": "tool_use"}),
        json!(5),
        json!(null),
        json!("text"),
        json!([]),
    ];
    let values = [json!(null), json!(""), json!("x"), json!(7), json!([]), json!({}), json!([{"id": "h1", "args": "x"}, 5, {"args": {"a": 1}}]), json!({"tool_calls": 5})];
    let kinds = ["user", "assistant", "assistant", "tool_result", "notice", "x"];
    let mut nonempty = 0;
    for _ in 0..10_000 {
        let hist: Vec<Value> = (0..next(8))
            .map(|_| {
                if next(15) == 0 {
                    return values[next(values.len())].clone();
                }
                let mut r = json!({"t": kinds[next(kinds.len())]});
                for k in ["msg", "content", "id", "is_error", "blocks"] {
                    match next(4) {
                        0 => r[k] = values[next(values.len())].clone(),
                        1 if k == "blocks" => r[k] = (0..next(4)).map(|_| blocks[next(blocks.len())].clone()).collect(),
                        1 if k == "msg" => {
                            let (content, calls) = (["", "c"][next(2)], &values[next(values.len())]);
                            r[k] = json!({"content": content, "tool_calls": calls});
                        }
                        _ => {}
                    }
                }
                r
            })
            .collect();
        let tools = [json!(null), json!([]), json!([5, {"function": 3}, {"function": {"name": "shell"}}])][next(3)].clone();
        let v: Value = serde_json::from_slice(&body("m", 1, "s", &tools, &hist)).unwrap();
        let m = v["messages"].as_array().unwrap();
        nonempty += usize::from(!m.is_empty());
        assert!(m.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{v}");
        assert!(m.iter().all(|m| m["content"].as_array().is_some_and(|c| !c.is_empty() && !c.iter().any(empty_text))), "{v}");
    }
    assert!(nonempty > 3000, "{nonempty}");
}
