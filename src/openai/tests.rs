#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::time::Instant;

const OFFERED: [&str; 4] = ["shell", "read", "write", "edit"];

fn hist3() -> Vec<Value> {
    vec![
        json!({"msg": {"content": "list the files", "role": "user"}, "t": "user", "ts": "2026-01-02T03:04:05Z"}),
        json!({"finish": "tool_calls", "msg": {"content": "", "role": "assistant", "tool_calls": [{"args": {"command": "ls"}, "id": "call_7", "name": "shell"}]},
            "t": "assistant", "ts": "2026-01-02T03:04:06Z", "usage": {"cached": 0, "in": 10, "out": 5}}),
        json!({"bytes": 21, "content": "a.txt\nb.txt\n[exit 0, 0.3 s]", "id": "call_7", "is_error": false, "ms": 312, "name": "shell", "t": "tool_result",
            "ts": "2026-01-02T03:04:07Z"}),
    ]
}

fn render(hist: &[Value]) -> String {
    String::from_utf8(body("m", 4096, "SYS", &json!([]), hist)).unwrap()
}

/// The `messages` of a rendered body.
fn messages(hist: &[Value]) -> Vec<Value> {
    let v: Value = serde_json::from_str(&render(hist)).unwrap();
    v["messages"].as_array().unwrap().clone()
}

fn chunk(delta: Value) -> String {
    json!({"choices": [{"delta": delta, "index": 0}]}).to_string()
}

fn tc(index: Option<u64>, id: Option<&str>, name: Option<&str>, args: Option<&str>) -> Value {
    let mut t = json!({"function": {}, "type": "function"});
    if let Some(i) = index {
        t["index"] = i.into();
    }
    if let Some(i) = id {
        t["id"] = i.into();
    }
    if let Some(n) = name {
        t["function"]["name"] = n.into();
    }
    if let Some(a) = args {
        t["function"]["arguments"] = a.into();
    }
    t
}

fn calls_chunk(tcs: Vec<Value>) -> String {
    chunk(json!({"tool_calls": tcs}))
}

fn finish_chunk(reason: &str) -> String {
    json!({"choices": [{"delta": {}, "finish_reason": reason, "index": 0}]}).to_string()
}

/// Feeds every payload, then finishes complete; returns the reply and the streamed parts.
fn run(data: &[String]) -> (Reply, Vec<Part>) {
    let mut acc = Acc::new(&OFFERED);
    let mut parts = vec![];
    for d in data {
        acc.data(d, &mut |p| parts.push(p)).unwrap();
    }
    (acc.finish(true).unwrap(), parts)
}

fn names(r: &Reply) -> Vec<&str> {
    r.calls.iter().map(|c| c.name.as_str()).collect()
}

fn texts(text: &str) -> (Vec<Call>, String) {
    text_calls(text, &OFFERED)
}

#[test]
fn render_openai_golden() {
    let got = String::from_utf8(body("local-27b", 4096, "SYS", &json!([{"function": {"name": "shell"}, "type": "function"}]), &hist3())).unwrap();
    let want = r#"{"chat_template_kwargs":{"enable_thinking":false},"max_tokens":4096,"messages":[{"content":"SYS","role":"system"},{"content":"list the files","role":"user"},{"content":"","role":"assistant","tool_calls":[{"function":{"arguments":"{\"command\":\"ls\"}","name":"shell"},"id":"call_7","type":"function"}]},{"content":"a.txt\nb.txt\n[exit 0, 0.3 s]","role":"tool","tool_call_id":"call_7"}],"model":"local-27b","stream":true,"stream_options":{"include_usage":true},"tools":[{"function":{"name":"shell"},"type":"function"}]}"#;
    assert_eq!(got, want);
    // Old messages are byte-identical when the history grows.
    let mut longer = hist3();
    longer.push(json!({"finish": "stop", "msg": {"content": "two files", "role": "assistant", "tool_calls": []}, "t": "assistant"}));
    let next = String::from_utf8(body("local-27b", 4096, "SYS", &json!([{"function": {"name": "shell"}, "type": "function"}]), &longer)).unwrap();
    let cut = want.find(r#"],"model""#).unwrap();
    assert!(next.starts_with(&want[..cut]), "{next}");
}

#[test]
fn render_thinking_switch_present() {
    let b = render(&[]);
    assert!(b.starts_with(r#"{"chat_template_kwargs":{"enable_thinking":false},"max_tokens":4096,"messages":[{"content":"SYS","role":"system"}]"#), "{b}");
    assert!(b.ends_with(r#""model":"m","stream":true,"stream_options":{"include_usage":true}}"#), "{b}");
}

#[test]
fn render_omits_empty_tool_calls() {
    let h = [json!({"msg": {"content": "done", "role": "assistant", "tool_calls": []}, "t": "assistant"})];
    assert_eq!(messages(&h)[1], json!({"content": "done", "role": "assistant"}));
    assert!(!render(&h).contains("tool_calls"));
    assert!(!render(&h).contains("\"tools\""), "an empty tool list is omitted");
}

#[test]
fn render_text_call_as_native() {
    // A text call is journaled without its markup and with the id the session gave it.
    let h = [json!({"msg": {"content": "I will look.", "role": "assistant", "tool_calls": [{"args": {"path": "a.txt"}, "id": "h1", "name": "read"}]},
        "t": "assistant"})];
    let want = json!({"content": "I will look.", "role": "assistant",
        "tool_calls": [{"function": {"arguments": "{\"path\":\"a.txt\"}", "name": "read"}, "id": "h1", "type": "function"}]});
    assert_eq!(messages(&h)[1], want);
}

#[test]
fn render_no_reasoning() {
    let h = [json!({"msg": {"content": "ok", "reasoning": "secret thoughts", "role": "assistant", "tool_calls": []}, "reasoning": "secret thoughts", "t": "assistant"})];
    let b = render(&h);
    assert!(!b.contains("secret") && !b.contains("reasoning"), "{b}");
}

#[test]
fn render_invalid_args_replay_empty() {
    let h = [json!({"msg": {"content": "", "role": "assistant", "tool_calls": [
        {"args": {}, "id": "a", "name": "shell", "raw": "{\"command\": \"ls\""},
        {"args": "ls", "id": "b", "name": "shell"},
        {"args": [1, 2], "id": "c", "name": "shell"}]}, "t": "assistant"})];
    let m = &messages(&h)[1];
    for i in 0..3 {
        assert_eq!(m["tool_calls"][i]["function"]["arguments"], "{}");
    }
    assert!(!render(&h).contains("raw"));
}

#[test]
fn render_skips_other_kinds() {
    let mut h = hist3();
    h.insert(1, json!({"t": "notice", "text": "x"}));
    h.insert(2, json!({"id": "call_7", "t": "approval", "verdict": "auto"}));
    assert_eq!(render(&h), render(&hist3()));
}

#[test]
#[ignore = "release gate"]
fn bench_render_3200() {
    let out = "x".repeat(2000) + "\n[exit 0, 0.1 s]";
    let mut h = vec![json!({"msg": {"content": "do the thing", "role": "user"}, "t": "user"})];
    for i in 0..1600 {
        let id = format!("h{i}");
        h.push(json!({"finish": "tool_calls", "msg": {"content": "next", "role": "assistant", "tool_calls": [{"args": {"command": format!("cat f{i}.txt")}, "id": id, "name": "shell"}]},
            "t": "assistant", "usage": {"cached": 0, "in": 1, "out": 1}}));
        h.push(json!({"bytes": out.len(), "content": out, "id": id, "is_error": false, "ms": 1, "name": "shell", "t": "tool_result"}));
    }
    h.truncate(3200);
    let tools = json!([{"function": {"name": "shell"}, "type": "function"}]);
    let t = Instant::now();
    let b = body("m", 4096, "SYS", &tools, &h);
    let took = t.elapsed();
    assert!(b.len() > 3_000_000);
    assert!(took.as_millis() < 50, "rendering 3,200 records took {took:?}");
}

#[test]
fn headers_only_with_key() {
    assert!(headers("").is_empty());
    assert_eq!(headers("k1"), vec![("Authorization", "Bearer k1".to_string())]);
}

#[test]
fn parse_content_and_reasoning() {
    let (r, parts) = run(&[
        chunk(json!({"content": null, "reasoning_content": "think", "role": "assistant"})),
        chunk(json!({"reasoning": " more"})),
        chunk(json!({"content": "Hel"})),
        chunk(json!({"content": "lo", "reasoning_content": ""})),
        finish_chunk("stop"),
    ]);
    assert_eq!(parts, vec![Part::Reasoning("think".into()), Part::Reasoning(" more".into()), Part::Text("Hel".into()), Part::Text("lo".into())]);
    assert_eq!((r.text.as_str(), r.reasoning.as_str(), r.finish.as_str()), ("Hello", "think more", "stop"));
    assert!(r.calls.is_empty());
}

#[test]
fn parse_index_slot() {
    let (r, _) = run(&[
        calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some("")), tc(Some(1), Some("b"), Some("read"), Some(""))]),
        calls_chunk(vec![tc(Some(1), None, None, Some(r#"{"path":"#))]),
        calls_chunk(vec![tc(Some(0), None, None, Some(r#"{"command":"ls"}"#))]),
        calls_chunk(vec![tc(Some(1), None, None, Some(r#""x"}"#))]),
        finish_chunk("tool_calls"),
    ]);
    assert_eq!(names(&r), ["shell", "read"]);
    assert_eq!((r.calls[0].id.as_str(), &r.calls[0].args), ("a", &json!({"command": "ls"})));
    assert_eq!((r.calls[1].id.as_str(), &r.calls[1].args), ("b", &json!({"path": "x"})));
    assert!(r.calls.iter().all(|c| c.raw.is_none() && c.error.is_none()));
}

#[test]
fn parse_index_new_id() {
    // Two calls with their own ids, both sent on index 0: two calls, not one merged call.
    let (r, _) = run(&[
        calls_chunk(vec![tc(Some(0), Some("call_a"), Some("shell"), Some(r#"{"command":"#))]),
        calls_chunk(vec![tc(Some(0), Some("call_a"), None, Some(r#""echo A"}"#))]),
        calls_chunk(vec![tc(Some(0), Some("call_b"), Some("shell"), Some(r#"{"command":"#))]),
        calls_chunk(vec![tc(Some(0), None, None, Some(r#""echo B"}"#))]),
        finish_chunk("tool_calls"),
    ]);
    let got: Vec<(&str, &Value)> = r.calls.iter().map(|c| (c.id.as_str(), &c.args)).collect();
    assert_eq!(got, [("call_a", &json!({"command": "echo A"})), ("call_b", &json!({"command": "echo B"}))]);
}

#[test]
fn parse_no_index_new_id() {
    let (r, _) = run(&[
        calls_chunk(vec![tc(None, Some("a1"), Some("shell"), Some(r#"{"command":"echo A"}"#))]),
        calls_chunk(vec![tc(None, Some("b2"), Some("shell"), Some(r#"{"command":"echo B"}"#))]),
        finish_chunk("tool_calls"),
    ]);
    let got: Vec<(&str, &Value)> = r.calls.iter().map(|c| (c.id.as_str(), &c.args)).collect();
    assert_eq!(got, [("a1", &json!({"command": "echo A"})), ("b2", &json!({"command": "echo B"}))]);
}

#[test]
fn parse_no_index_append() {
    let (r, _) = run(&[
        calls_chunk(vec![tc(None, Some("a1"), Some("shell"), Some(r#"{"comm"#))]),
        calls_chunk(vec![tc(None, None, None, Some(r#"and":"#))]),
        calls_chunk(vec![tc(None, Some(""), None, Some(r#""ls"#))]),
        calls_chunk(vec![tc(None, Some("a1"), None, Some(r#""}"#))]),
        finish_chunk("tool_calls"),
    ]);
    assert_eq!(r.calls.len(), 1);
    assert_eq!((r.calls[0].id.as_str(), &r.calls[0].args), ("a1", &json!({"command": "ls"})));
}

#[test]
fn parse_name_rules() {
    let (r, _) = run(&[
        // set when empty, then a repeat of the whole name is ignored
        calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some("{}"))]),
        calls_chunk(vec![tc(Some(0), None, Some("shell"), None)]),
        // a name split over fragments is joined
        calls_chunk(vec![tc(Some(1), Some("b"), Some("wr"), Some("{}"))]),
        calls_chunk(vec![tc(Some(1), None, Some("ite"), None)]),
        // a name that comes after the slot opened
        calls_chunk(vec![tc(Some(2), Some("c"), None, Some("{}"))]),
        calls_chunk(vec![tc(Some(2), None, Some("read"), None)]),
        finish_chunk("tool_calls"),
    ]);
    assert_eq!(names(&r), ["shell", "write", "read"]);
}

#[test]
fn parse_args_string_append() {
    let pieces = ["", "{\"comm", "and\": \"ec", "ho é\\n日本\"", ", \"timeout\": 5}"];
    let mut data = vec![calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), None)])];
    data.extend(pieces.iter().map(|p| calls_chunk(vec![tc(Some(0), None, None, Some(p))])));
    data.push(finish_chunk("tool_calls"));
    let (r, _) = run(&data);
    assert_eq!(r.calls[0].args, json!({"command": "echo é\n日本", "timeout": 5}));
    assert_eq!(r.calls[0].raw, None);
}

#[test]
fn parse_args_object() {
    let mut t = tc(Some(0), Some("a"), Some("shell"), None);
    t["function"]["arguments"] = json!({"command": "ls -l", "timeout": 3});
    let (r, _) = run(&[calls_chunk(vec![t]), finish_chunk("tool_calls")]);
    assert_eq!(r.calls[0].args, json!({"command": "ls -l", "timeout": 3}));
    assert_eq!(r.calls[0].raw, None);
}

#[test]
fn parse_usage() {
    let (r, _) = run(&[
        chunk(json!({"content": "hi"})),
        finish_chunk("stop"),
        json!({"choices": [], "usage": {"completion_tokens": 22, "prompt_tokens": 1840, "prompt_tokens_details": {"cached_tokens": 1664}, "total_tokens": 1862}}).to_string(),
    ]);
    assert_eq!(r.usage, Usage { input: 1840, output: 22, cached: 1664 });
    // A chunk with `usage: null` changes nothing.
    let (r, _) = run(&[json!({"choices": [], "usage": {"completion_tokens": 2, "prompt_tokens": 3}}).to_string(), json!({"usage": null}).to_string()]);
    assert_eq!(r.usage, Usage { input: 3, output: 2, cached: 0 });
}

#[test]
fn parse_timings_cache_n() {
    let last = json!({"choices": [{"delta": {}, "finish_reason": "stop", "index": 0}], "timings": {"cache_n": 1500, "predicted_n": 7, "prompt_n": 40},
        "usage": {"completion_tokens": 7, "prompt_tokens": 1540}});
    let (r, _) = run(&[chunk(json!({"content": "ok"})), last.to_string()]);
    assert_eq!(r.usage, Usage { input: 1540, output: 7, cached: 1500 });
}

#[test]
fn parse_error_event_fatal() {
    let mut acc = Acc::new(&OFFERED);
    assert!(matches!(acc.data(&chunk(json!({"content": "partial"})), &mut |_| {}), Ok(Flow::More)));
    let ev = json!({"error": {"message": "the server is busy", "type": "overloaded_error"}});
    let e = acc.data(&ev.to_string(), &mut |_| {}).err().unwrap();
    assert_eq!(e, error_object(&ev, false));
    assert_eq!(e.retry, None, "an error after the first event is never retried");
}

#[test]
fn parse_error_first_event_retryable() {
    let ev = json!({"error": {"message": "the server is busy", "type": "overloaded_error"}});
    let e = Acc::new(&OFFERED).data(&ev.to_string(), &mut |_| {}).err().unwrap();
    assert_eq!(e, error_object(&ev, true));
    assert!(e.retry.is_some());
}

#[test]
fn parse_non_json_data() {
    let mut acc = Acc::new(&OFFERED);
    let e = acc.data("{\"choices\": [", &mut |_| {}).err().unwrap();
    assert_eq!(e.kind, "provider");
    assert!(e.message.starts_with("stream data is not JSON ("), "{}", e.message);
    assert!(e.message.ends_with("): {\"choices\": ["), "{}", e.message);
    assert_eq!(e.retry, None);
    // At most 200 characters of it are quoted.
    let e = Acc::new(&OFFERED).data(&"é".repeat(300), &mut |_| {}).err().unwrap();
    assert!(e.message.ends_with(&format!("): {}", "é".repeat(200))));
    // An empty event is not data.
    assert!(matches!(Acc::new(&OFFERED).data("  ", &mut |_| {}), Ok(Flow::More)));
}

#[test]
fn parse_delta_before_finish() {
    let mut acc = Acc::new(&OFFERED);
    let mut parts = vec![];
    let last = json!({"choices": [{"delta": {"content": "done.", "tool_calls": [tc(Some(0), Some("a"), Some("shell"), Some(r#"{"command":"ls"}"#))]},
        "finish_reason": "tool_calls", "index": 0}]});
    assert!(matches!(acc.data(&last.to_string(), &mut |p| parts.push(p)), Ok(Flow::Finished)));
    assert_eq!(parts, vec![Part::Text("done.".into()), Part::Call("shell".into())]);
    let r = acc.finish(true).unwrap();
    assert_eq!((r.text.as_str(), names(&r), r.finish.as_str()), ("done.", vec!["shell"], "tool_calls"));
}

#[test]
fn parse_done() {
    let mut acc = Acc::new(&OFFERED);
    assert!(matches!(acc.data(&chunk(json!({"content": "a"})), &mut |_| {}), Ok(Flow::More)));
    assert!(matches!(acc.data(&finish_chunk("stop"), &mut |_| {}), Ok(Flow::Finished)));
    assert!(matches!(acc.data(&json!({"choices": [], "usage": {"prompt_tokens": 1}}).to_string(), &mut |_| {}), Ok(Flow::Finished)));
    assert!(matches!(acc.data("[DONE]", &mut |_| {}), Ok(Flow::Done)));
    assert!(matches!(Acc::new(&OFFERED).data(" [DONE] ", &mut |_| {}), Ok(Flow::Done)));
}

#[test]
fn parse_part_call_emitted() {
    let (_, parts) = run(&[
        calls_chunk(vec![tc(Some(0), Some("a"), Some("sh"), Some(""))]),
        calls_chunk(vec![tc(Some(0), None, Some("ell"), Some("{}"))]),
        calls_chunk(vec![tc(Some(1), Some("b"), None, Some("{}"))]),
        calls_chunk(vec![tc(Some(1), None, Some("read"), None)]),
        finish_chunk("tool_calls"),
    ]);
    // One per slot, when it opens, with the name so far ("unknown" before any).
    assert_eq!(parts, vec![Part::Call("sh".into()), Part::Call("unknown".into())]);
}

#[test]
fn parser_two_empty_id_calls() {
    let (r, _) = run(&[
        calls_chunk(vec![tc(Some(0), Some(""), Some("shell"), Some(r#"{"command":"echo 1"}"#))]),
        calls_chunk(vec![tc(Some(1), Some(""), Some("shell"), Some(r#"{"command":"echo 2"}"#))]),
        finish_chunk("tool_calls"),
    ]);
    let got: Vec<(&str, &Value)> = r.calls.iter().map(|c| (c.id.as_str(), &c.args)).collect();
    assert_eq!(got, [("", &json!({"command": "echo 1"})), ("", &json!({"command": "echo 2"}))]);
}

#[test]
fn parse_no_index_empty_ids() {
    // Entries of one chunk's array without an index are calls of their own, as in a whole completion.
    let (r, parts) = run(&[
        calls_chunk(vec![tc(None, None, Some("shell"), Some(r#"{"command":"echo 1"}"#)), tc(None, Some(""), Some("read"), Some(r#"{"path":"x"}"#))]),
        finish_chunk("tool_calls"),
    ]);
    let got: Vec<(&str, &Value)> = r.calls.iter().map(|c| (c.name.as_str(), &c.args)).collect();
    assert_eq!(got, [("shell", &json!({"command": "echo 1"})), ("read", &json!({"path": "x"}))]);
    assert_eq!(parts, vec![Part::Call("shell".into()), Part::Call("read".into())]);
    // Across chunks, a fragment with no index and no new id continues the last call.
    let (r, _) =
        run(&[calls_chunk(vec![tc(None, None, Some("shell"), Some(r#"{"command":"#))]), calls_chunk(vec![tc(None, Some(""), None, Some(r#""ls"}"#))]), finish_chunk("tool_calls")]);
    assert_eq!((names(&r), &r.calls[0].args), (vec!["shell"], &json!({"command": "ls"})));
}

#[test]
fn parse_tool_calls_non_object_entry() {
    // An entry that is not an object opens no call, so the text is still scanned for calls.
    let hermes = "<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call>";
    let (r, parts) = run(&[chunk(json!({"content": hermes})), calls_chunk(vec![json!(null), json!(7), json!("x"), json!([1])]), finish_chunk("stop")]);
    assert_eq!((names(&r), r.calls[0].error.as_ref()), (vec!["shell"], None));
    assert_eq!(parts, vec![Part::Text(hermes.into())]);
    // Next to a real entry, only the real entry counts.
    let (r, _) = run(&[calls_chunk(vec![json!(null), tc(Some(0), Some("a"), Some("read"), Some(r#"{"path":"x"}"#))]), finish_chunk("tool_calls")]);
    assert_eq!((names(&r), &r.calls[0].args), (vec!["read"], &json!({"path": "x"})));
}

#[test]
fn after_incomplete_names_calls() {
    let mut acc = Acc::new(&OFFERED);
    for d in [
        chunk(json!({"content": "Let me look."})),
        calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some("{\"comm"))]),
        calls_chunk(vec![tc(Some(1), Some("b"), Some("read"), None)]),
    ] {
        acc.data(&d, &mut |_| {}).unwrap();
    }
    let e = acc.finish(false).err().unwrap();
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("stream_incomplete", "stream ended early; dropped calls: shell, read", None));
    let e = Acc::new(&OFFERED).finish(false).err().unwrap();
    assert_eq!((e.kind, e.message.as_str()), ("stream_incomplete", "stream ended early"));
}

#[test]
fn after_native_calls_any_finish() {
    // Native calls run whatever the finish reason, unless it is `length`.
    for finish in [Some("tool_calls"), Some("stop"), None, Some("eos")] {
        let mut data = vec![calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some(r#"{"command":"ls"}"#))])];
        data.extend(finish.map(finish_chunk));
        let (r, _) = run(&data);
        assert_eq!((names(&r), &r.calls[0].error), (vec!["shell"], &None), "{finish:?}");
        assert_eq!(r.finish, finish.unwrap_or(""));
    }
}

#[test]
fn after_length_cut_args() {
    assert_eq!(CUT_ARGS, "arguments cut at max_tokens; not run. Send a shorter call.");
    let (r, _) = run(&[
        calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some(r#"{"command":"ls"}"#))]),
        calls_chunk(vec![tc(Some(1), Some("b"), Some("write"), Some(r#"{"path":"a.txt","content":"aaa"#))]),
        finish_chunk("length"),
    ]);
    assert_eq!(r.finish, "length");
    assert_eq!(names(&r), ["shell", "write"]);
    assert!(r.calls.iter().all(|c| c.error.as_deref() == Some(CUT_ARGS)));
    assert_eq!((&r.calls[1].args, r.calls[1].raw.as_deref()), (&json!({}), Some(r#"{"path":"a.txt","content":"aaa"#)));
}

#[test]
fn after_length_no_calls_text() {
    let text = "Here: <tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call> and more";
    let (r, _) = run(&[chunk(json!({"content": text})), finish_chunk("length")]);
    assert!(r.calls.is_empty());
    assert_eq!((r.text.as_str(), r.finish.as_str()), (text, "length"));
}

#[test]
fn after_bad_args_raw() {
    for bad in ["{\"command\": \"ls\",}", "{\"command\": ", ""] {
        let (r, _) = run(&[calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some(bad))]), finish_chunk("tool_calls")]);
        assert_eq!((&r.calls[0].args, r.calls[0].raw.as_deref(), &r.calls[0].error), (&json!({}), Some(bad), &None));
    }
    // No arguments at all: the same, with empty text.
    let (r, _) = run(&[calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), None)]), finish_chunk("tool_calls")]);
    assert_eq!((&r.calls[0].args, r.calls[0].raw.as_deref()), (&json!({}), Some("")));
}

#[test]
fn after_args_over_1mib() {
    let big = json!({"command": "x".repeat(1 << 20)}).to_string();
    let (r, _) = run(&[calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some(&big))]), finish_chunk("tool_calls")]);
    assert_eq!((&r.calls[0].args, r.calls[0].raw.as_deref()), (&json!({}), Some(big.as_str())));
    // Exactly 1 MiB still parses.
    let edge = json!({"command": "x".repeat((1 << 20) - 14)}).to_string();
    assert_eq!(edge.len(), 1 << 20);
    let (r, _) = run(&[calls_chunk(vec![tc(Some(0), Some("a"), Some("shell"), Some(&edge))]), finish_chunk("tool_calls")]);
    assert_eq!(r.calls[0].raw, None);
}

#[test]
fn after_scan_content_then_reasoning() {
    let hermes = |cmd: &str| format!("<tool_call>\n{{\"name\": \"shell\", \"arguments\": {{\"command\": \"{cmd}\"}}}}\n</tool_call>");
    // Calls in the content: those run, their markup leaves the text, the reasoning is not scanned.
    let (r, _) = run(&[chunk(json!({"reasoning_content": hermes("from reasoning")})), chunk(json!({"content": format!("I will list.\n{}", hermes("ls"))})), finish_chunk("stop")]);
    assert_eq!(r.calls.iter().map(|c| &c.args).collect::<Vec<_>>(), [&json!({"command": "ls"})]);
    assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("I will list.", hermes("from reasoning").as_str()));
    // None in the content: the reasoning's calls run, and neither text changes.
    let (r, _) = run(&[chunk(json!({"reasoning_content": hermes("pwd")})), chunk(json!({"content": "Checking."})), finish_chunk("stop")]);
    assert_eq!(r.calls.iter().map(|c| &c.args).collect::<Vec<_>>(), [&json!({"command": "pwd"})]);
    assert_eq!((r.text.as_str(), r.reasoning.as_str()), ("Checking.", hermes("pwd").as_str()));
    // Native calls win: no scan at all.
    let (r, _) = run(&[chunk(json!({"content": hermes("ls")})), calls_chunk(vec![tc(Some(0), Some("n"), Some("read"), Some(r#"{"path":"a"}"#))])]);
    assert_eq!((names(&r), r.text.as_str()), (vec!["read"], hermes("ls").as_str()));
}

#[test]
fn text_hermes() {
    let (calls, rest) = texts("Sure.\n<tool_call>\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls -la\"}}\n</tool_call>");
    assert_eq!(calls, vec![Call { name: "shell".into(), args: json!({"command": "ls -la"}), ..Call::default() }]);
    assert_eq!(rest, "Sure.");
    assert_eq!(calls[0].id, "", "the session gives text calls their ids");
    // Explicit markup is a call whatever the name; validation answers an unknown one.
    let (calls, _) = texts("<tool_call>{\"name\": \"browse\", \"arguments\": {}}</tool_call>");
    assert_eq!(calls[0].name, "browse");
}

#[test]
fn text_hermes_parameters() {
    let (calls, rest) = texts("<tool_call>{\"name\": \"read\", \"parameters\": {\"path\": \"a.txt\", \"limit\": 5}}</tool_call>");
    assert_eq!((calls[0].name.as_str(), &calls[0].args, rest.as_str()), ("read", &json!({"path": "a.txt", "limit": 5}), ""));
}

#[test]
fn text_hermes_string_args() {
    let (calls, _) = texts(r#"<tool_call>{"name": "shell", "arguments": "{\"command\": \"pwd\"}"}</tool_call>"#);
    assert_eq!((&calls[0].args, &calls[0].raw), (&json!({"command": "pwd"}), &None));
    // A string that does not hold JSON is kept in `raw` for validation.
    let (calls, _) = texts(r#"<tool_call>{"name": "shell", "arguments": "pwd"}</tool_call>"#);
    assert_eq!((&calls[0].args, calls[0].raw.as_deref(), &calls[0].error), (&json!({}), Some("pwd"), &None));
}

#[test]
fn text_qwen_xml_wrapped() {
    let t = "I'll check.\n<tool_call>\n<function=shell>\n<parameter=command>\nls -la\n</parameter>\n<parameter=timeout>\n30\n</parameter>\n</function>\n</tool_call>";
    let (calls, rest) = texts(t);
    assert_eq!(calls, vec![Call { name: "shell".into(), args: json!({"command": "ls -la", "timeout": "30"}), ..Call::default() }]);
    assert_eq!(rest, "I'll check.");
}

#[test]
fn text_qwen_xml_bare() {
    let (calls, rest) = texts("Reading.\n<function=read>\n<parameter=path>\nsrc/main.rs\n</parameter>\n</function>\nThen more.");
    assert_eq!((calls[0].name.as_str(), &calls[0].args), ("read", &json!({"path": "src/main.rs"})));
    assert_eq!(rest, "Reading.\n\nThen more.");
    let (calls, _) = texts("<function=shell><parameter=command>pwd</parameter></function>");
    assert_eq!(calls[0].args, json!({"command": "pwd"}));
}

#[test]
fn text_qwen_xml_newlines() {
    // One leading and one trailing newline go; the rest of the value is kept byte for byte.
    let t = "<function=write>\n<parameter=path>\na.txt\n</parameter>\n<parameter=content>\n\nline 1\n  line 2\n\n</parameter>\n</function>";
    let (calls, _) = texts(t);
    assert_eq!(calls[0].args, json!({"content": "\nline 1\n  line 2\n", "path": "a.txt"}));
}

fn incomplete_call(name: &str) -> Call {
    Call { name: name.into(), args: json!({}), error: Some(INCOMPLETE.into()), ..Call::default() }
}

#[test]
fn text_qwen_xml_two_in_one_wrapper() {
    // Each block in one wrapper is a call of its own: never one call with both blocks' parameters.
    let t = "On it.\n<tool_call>\n<function=write>\n<parameter=path>\nnotes.txt\n</parameter>\n<parameter=content>\nhello\n</parameter>\n</function>\n\
             <function=read>\n<parameter=path>\nsrc/main.rs\n</parameter>\n</function>\n</tool_call>";
    let (calls, rest) = texts(t);
    let want = vec![
        Call { name: "write".into(), args: json!({"content": "hello", "path": "notes.txt"}), ..Call::default() },
        Call { name: "read".into(), args: json!({"path": "src/main.rs"}), ..Call::default() },
    ];
    assert_eq!((calls, rest.as_str()), (want, "On it."));
    // A block in the wrapper that is not closed is an incomplete call; the closed one before it still counts.
    let t = "<tool_call><function=read><parameter=path>a</parameter></function>\n<function=shell><parameter=command>ls</parameter>\n</tool_call>";
    let (calls, rest) = texts(t);
    assert_eq!((calls, rest.as_str()), (vec![Call { name: "read".into(), args: json!({"path": "a"}), ..Call::default() }, incomplete_call("shell")], ""));
}

#[test]
fn text_qwen_xml_unclosed_then_next() {
    // A block that lost its `</function>` ends where the next block starts: it does not run, the next one does.
    let t = "<function=shell>\n<parameter=command>\nls\n</parameter>\n<function=read>\n<parameter=path>\na.txt\n</parameter>\n</function>\nDone.";
    let (calls, rest) = texts(t);
    assert_eq!((calls, rest.as_str()), (vec![incomplete_call("shell"), Call { name: "read".into(), args: json!({"path": "a.txt"}), ..Call::default() }], "Done."));
}

#[test]
fn text_qwen_xml_value_is_opaque() {
    // Markup inside a parameter value is part of the value: it neither cuts the call nor becomes a call of its own.
    let t = "<function=write>\n<parameter=path>\nnotes.md\n</parameter>\n<parameter=content>\nThe parser reads <function=NAME> tags.\n</parameter>\n</function>";
    assert_eq!(texts(t).0, vec![Call { name: "write".into(), args: json!({"content": "The parser reads <function=NAME> tags.", "path": "notes.md"}), ..Call::default() }]);
    let t = "<function=write>\n<parameter=path>\nREADME.md\n</parameter>\n<parameter=content>\nRun: <function=shell><parameter=command>make clean</parameter></function>\n\
             </parameter>\n</function>";
    assert_eq!(names(&Reply { calls: texts(t).0, ..Reply::default() }), ["write"], "an example call in a value never runs");
    // A value holding `</function>` cuts the call there, so it does not run.
    let t = "<function=write>\n<parameter=path>\na.md\n</parameter>\n<parameter=content>\nEnd it with </function>.\n</parameter>\n</function>";
    assert_eq!(texts(t).0, vec![incomplete_call("write")]);
}

#[test]
fn text_qwen_xml_empty_name_unknown() {
    for (t, name) in [
        ("<function=></function>", "unknown"),
        ("<function= ><parameter=path>x</parameter></function>", "unknown"),
        ("<tool_call><function=></function></tool_call>", "unknown"),
        // A name not ended by `>` is malformed; it still names the call.
        ("<function=read\n<parameter=path>x</parameter></function>", "read"),
        ("<function=read<parameter=path>x</parameter></function>", "read"),
        ("<function=read\n", "read"),
    ] {
        assert_eq!(texts(t), (vec![incomplete_call(name)], String::new()), "{t:?}");
    }
}

#[test]
fn text_calls_linear() {
    // The scan is one pass: 1 MiB of dense markup or of many fences takes well under a second. Searching the rest of the
    // text again for every match would take minutes here.
    for unit in [
        "<tool_call></tool_call>",
        "<function=a>",
        "x\n```\n",
        "a```b <function=shell><parameter=command>ls</parameter></function>\n",
        "<tool_call>{\"name\": \"read\", \"arguments\": {\"path\": \"a\"}}</tool_call>",
        "<tool_call><function=a></function><function=b>",
    ] {
        let t = unit.repeat((1 << 20) / unit.len() + 1);
        let start = Instant::now();
        let (calls, _) = texts(&t);
        let took = start.elapsed();
        assert!(!calls.is_empty() || unit.starts_with('x'), "{unit:?}");
        assert!(took.as_millis() < 3000, "{} calls from 1 MiB of {unit:?} took {took:?}", calls.len());
    }
}

#[test]
fn text_fenced_json_offered() {
    let (calls, rest) = texts("Running it:\n```json\n{\n  \"name\": \"shell\",\n  \"arguments\": {\n    \"command\": \"make\"\n  }\n}\n```\nDone soon.");
    assert_eq!((calls[0].name.as_str(), &calls[0].args), ("shell", &json!({"command": "make"})));
    assert_eq!(rest, "Running it:\n\nDone soon.");
    // An untagged fence counts too.
    let (calls, _) = texts("```\n{\"name\": \"read\", \"arguments\": {\"path\": \"x\"}}\n```");
    assert_eq!(calls[0].name, "read");
    // So does a fence the reply ends inside.
    let (calls, rest) = texts("Now:\n```json\n{\"name\": \"read\", \"arguments\": {\"path\": \"x\"}}\n");
    assert_eq!((calls.len(), rest.as_str()), (1, "Now:"));
}

#[test]
fn text_fenced_unoffered_is_prose() {
    for t in [
        "Example:\n```json\n{\"name\": \"deploy\", \"arguments\": {\"to\": \"prod\"}}\n```",
        "```json\n{\"name\": \"shell\", \"description\": \"x\", \"parameters\": {\"type\": \"object\"}}\n```",
        "```json\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"},}\n```",
    ] {
        let (calls, rest) = texts(t);
        assert!(calls.is_empty(), "{t}");
        assert_eq!(rest, t);
    }
    // Offered names are the session's, not a fixed list.
    let (calls, _) = text_calls("```json\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}\n```", &["read"]);
    assert!(calls.is_empty());
}

#[test]
fn text_whole_text_object() {
    let (calls, rest) = texts("  {\"name\": \"shell\", \"arguments\": {\"command\": \"date\"}}\n");
    assert_eq!((calls[0].name.as_str(), &calls[0].args, rest.as_str()), ("shell", &json!({"command": "date"}), ""));
    let t = "{\"name\": \"deploy\", \"arguments\": {}}";
    assert_eq!(texts(t), (vec![], t.to_string()));
}

#[test]
fn text_bash_fence_never_call() {
    for t in [
        "Run this:\n```bash\nrm -rf build && make\n```",
        "```sh\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}\n```",
        "```bash\necho '<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call>'\n```",
        "```python\nprint('<function=shell><parameter=command>ls</parameter></function>')\n```",
        "Unclosed:\n```bash\n<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call>\n",
    ] {
        assert_eq!(texts(t), (vec![], t.to_string()), "{t}");
    }
    // Inline triple backticks are not a fence: the call after them is still found.
    let (calls, _) = texts("Use ```ls``` here.\n<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call>");
    assert_eq!(calls.len(), 1);
}

#[test]
fn text_unclosed_incomplete() {
    assert_eq!(INCOMPLETE, "incomplete tool call");
    let cases = [
        ("I'll run it.\n<tool_call>\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls", "shell"),
        ("<tool_call>\n<function=write>\n<parameter=path>\na.txt\n</parameter>\n</function>\n", "write"),
        ("<function=read>\n<parameter=path>\nsrc/", "read"),
        ("text <tool_call>", "unknown"),
        ("<tool_call>{\"name\": \"sh", "unknown"),
    ];
    for (t, name) in cases {
        let (calls, rest) = texts(t);
        assert_eq!(calls, vec![Call { name: name.into(), args: json!({}), error: Some(INCOMPLETE.into()), ..Call::default() }], "{t}");
        assert_eq!(rest, t.split('<').next().unwrap().trim(), "{t}");
    }
}

#[test]
fn text_closed_unparseable_incomplete() {
    let cases = [
        ("<tool_call>not a call</tool_call>", "unknown"),
        ("<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\",}}</tool_call>", "shell"),
        ("<tool_call>{\"arguments\": {\"command\": \"ls\"}}</tool_call>", "unknown"),
        ("<tool_call>{\"name\": \"shell\"}</tool_call>", "shell"),
        ("<tool_call><function=edit><parameter=path>a</function></tool_call>", "edit"),
        ("<function=shell><parameter=command>ls</function>", "shell"),
        // Only parameters and white space may stand between the name and `</function>`.
        ("<function=shell>\nls\n</function>", "shell"),
        ("<function=shell><parameter=command>ls</parameter> and then</function>", "shell"),
        ("<tool_call><function=read>x<parameter=path>a</parameter></function></tool_call>", "read"),
    ];
    for (t, name) in cases {
        let (calls, rest) = texts(t);
        assert_eq!(calls, vec![Call { name: name.into(), args: json!({}), error: Some(INCOMPLETE.into()), ..Call::default() }], "{t}");
        assert_eq!(rest, "", "{t}");
    }
}

#[test]
fn text_markup_removed() {
    let t = "First.\n<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"a\"}}</tool_call>\nSecond.\n<function=read><parameter=path>b</parameter></function>\n\
             Third.\n```json\n{\"name\": \"shell\", \"arguments\": {\"command\": \"c\"}}\n```\nLast: ```js\nx()\n```\n";
    let (calls, rest) = texts(t);
    assert_eq!(calls.len(), 3);
    assert_eq!(rest, "First.\n\nSecond.\n\nThird.\n\nLast: ```js\nx()\n```");
    // No call: the text comes back untouched, whitespace included.
    let t = "  just an answer\n\n";
    assert_eq!(texts(t), (vec![], t.to_string()));
}

#[test]
fn text_every_match_in_order() {
    let t = "```json\n{\"name\": \"write\", \"arguments\": {\"path\": \"1\"}}\n```\n<function=read><parameter=path>2</parameter></function>\n\
             <tool_call>{\"name\": \"edit\", \"arguments\": {\"path\": \"3\"}}</tool_call>\n<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"4\"}}</tool_call>";
    let (calls, rest) = texts(t);
    let got: Vec<(&str, &Value)> = calls.iter().map(|c| (c.name.as_str(), &c.args)).collect();
    let want = [("write", json!({"path": "1"})), ("read", json!({"path": "2"})), ("edit", json!({"path": "3"})), ("shell", json!({"command": "4"}))];
    assert_eq!(got, want.iter().map(|(n, a)| (*n, a)).collect::<Vec<_>>());
    assert_eq!(rest, "");
}

#[test]
fn json_completion() {
    let b = json!({"choices": [{"finish_reason": "stop", "index": 0, "message": {"content": "All done.", "reasoning_content": "hmm", "role": "assistant"}}],
        "object": "chat.completion", "usage": {"completion_tokens": 3, "prompt_tokens": 50, "prompt_tokens_details": {"cached_tokens": 40}}});
    let r = Acc::new(&OFFERED).json(b.to_string().as_bytes()).unwrap();
    assert_eq!(
        r,
        Reply { text: "All done.".into(), reasoning: "hmm".into(), calls: vec![], finish: "stop".into(), usage: Usage { input: 50, output: 3, cached: 40 }, blocks: None }
    );
    // Text calls in a completion are found as in a stream.
    let b = json!({"choices": [{"finish_reason": "stop", "message": {"content": "<function=read><parameter=path>a</parameter></function>"}}]});
    assert_eq!(names(&Acc::new(&OFFERED).json(b.to_string().as_bytes()).unwrap()), ["read"]);
    let e = Acc::new(&OFFERED).json(b"<html>").err().unwrap();
    assert!(e.kind == "provider" && e.message.starts_with("the completion is not JSON ("), "{}", e.message);
}

#[test]
fn json_completion_calls() {
    // Each entry is a call of its own, even with empty or missing ids, and arguments may be an object.
    let b = json!({"choices": [{"finish_reason": "tool_calls", "message": {"content": null, "role": "assistant", "tool_calls": [
        {"function": {"arguments": "{\"command\":\"ls\"}", "name": "shell"}, "id": "", "type": "function"},
        {"function": {"arguments": {"path": "a"}, "name": "read"}, "type": "function"},
        {"function": {"arguments": "{bad", "name": "shell"}, "id": "c3", "index": 0, "type": "function"}]}}]});
    let r = Acc::new(&OFFERED).json(b.to_string().as_bytes()).unwrap();
    let got: Vec<(&str, &str, &Value, Option<&str>)> = r.calls.iter().map(|c| (c.id.as_str(), c.name.as_str(), &c.args, c.raw.as_deref())).collect();
    assert_eq!(got, [("", "shell", &json!({"command": "ls"}), None), ("", "read", &json!({"path": "a"}), None), ("c3", "shell", &json!({}), Some("{bad"))]);
    assert_eq!((r.text.as_str(), r.finish.as_str()), ("", "tool_calls"));
}

#[test]
fn json_completion_no_choices() {
    // Valid JSON that is not a chat completion (a wrong path on some servers) is a provider error, not an empty answer.
    for b in [r#"{"detail": "Not Found"}"#, r#"{"choices": []}"#, r#"{"choices": [{"text": "hi"}]}"#, "[]", "null"] {
        let e = Acc::new(&OFFERED).json(b.as_bytes()).err().unwrap();
        assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", format!("the completion has no choices[0].message: {b}").as_str(), None), "{b}");
    }
    // A message with no content and no calls is still a completion.
    let r = Acc::new(&OFFERED).json(br#"{"choices": [{"finish_reason": "stop", "message": {"content": null}}]}"#).unwrap();
    assert_eq!((r.text.as_str(), r.finish.as_str()), ("", "stop"));
}

/// xorshift64: reproducible noise without a crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
}

#[test]
fn acc_never_panics() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let strs = [json!(""), json!("a"), json!("sh"), json!("ell"), json!("{\"c"), json!("é日🙂"), json!(null), json!(7), json!(-1), json!({"k": 1}), json!([1])];
    for _ in 0..3000 {
        let mut acc = Acc::new(&OFFERED);
        for _ in 0..rng.next() % 12 {
            let mut t = json!({});
            for key in ["index", "id", "type"] {
                if rng.next().is_multiple_of(2) {
                    t[key] = rng.pick(&strs).clone();
                }
            }
            if rng.next().is_multiple_of(4) {
                t["index"] = (rng.next() % 3).into();
            }
            t["function"] = json!({"name": rng.pick(&strs).clone(), "arguments": rng.pick(&strs).clone()});
            let delta = json!({"content": rng.pick(&strs).clone(), "reasoning": rng.pick(&strs).clone(), "tool_calls": [t, rng.pick(&strs).clone()]});
            let d = match rng.next() % 9 {
                0 => json!({"choices": [{"delta": delta, "finish_reason": rng.pick(&["stop", "length", "tool_calls"])}]}).to_string(),
                1 => json!({"choices": rng.pick(&strs).clone(), "usage": {"prompt_tokens": rng.pick(&strs).clone()}, "timings": rng.pick(&strs).clone()}).to_string(),
                2 => rng.pick(&["", "[DONE]", "{", "null", "[]", "\"x\"", "{\"choices\":[{\"delta\":null}]}"]).to_string(),
                3 => json!({"choices": [{"delta": rng.pick(&strs).clone()}]}).to_string(),
                _ => json!({"choices": [{"delta": delta}]}).to_string(),
            };
            let _ = acc.data(&d, &mut |p| assert!(!matches!(p, Part::Call(name) if name.is_empty())));
        }
        let _ = acc.finish(rng.next().is_multiple_of(2));
    }
    for _ in 0..500 {
        let body: Vec<u8> = (0..rng.next() % 40).map(|_| *rng.pick(b"{}[]\":,choices message tool_calls 1\xff")).collect();
        let _ = Acc::new(&OFFERED).json(&body);
    }
}

#[test]
fn text_calls_never_panics() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let tokens = [
        "<tool_call>",
        "</tool_call>",
        "<function=",
        "</function>",
        "<parameter=",
        "</parameter>",
        ">",
        "<",
        "```",
        "```json",
        "```bash",
        "\n",
        " ",
        "\t",
        "{",
        "}",
        "\"",
        ":",
        ",",
        "\"name\"",
        "\"arguments\"",
        "\"parameters\"",
        "shell",
        "read",
        "é",
        "日本",
        "🙂",
        "\\",
        "{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}",
        "\"{}\"",
        "[1]",
        "null",
    ];
    for _ in 0..20_000 {
        let t: String = (0..rng.next() % 24).map(|_| *rng.pick(&tokens)).collect();
        let (calls, rest) = texts(&t);
        assert!(rest.len() <= t.len(), "{t:?}");
        assert!(calls.iter().all(|c| c.error.is_none() || c.args == json!({})), "{t:?}");
        let _ = text_calls(&t, &[]);
    }
}
