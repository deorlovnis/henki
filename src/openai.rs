//! The OpenAI-compatible chat-completions wire format: the request body, the stream accumulator with its defensive
//! call parser, and text calls for servers that leave tool markup in the content.
use crate::types::{ARGS_MAX, CUT_ARGS, Call, Flow, Part, ProviderError, Reply, Usage, Wire, call_name};
use crate::{http::bad, http::error_object, http::perr};
use serde_json::{Map, Value, json};

pub const PATH: &str = "/chat/completions";
pub const INCOMPLETE: &str = "incomplete tool call";

/// `Authorization: Bearer KEY` only when `key` is not empty.
pub fn headers(key: &str) -> Vec<(&'static str, String)> {
    if key.is_empty() { vec![] } else { vec![("Authorization", format!("Bearer {key}"))] }
}

/// The whole request body from journal records: compact JSON, keys sorted, old messages byte-identical every time.
pub fn body(model: &str, max_tokens: u64, system: &str, tools: &Value, hist: &[Value]) -> Vec<u8> {
    let mut msgs = vec![json!({"content": system, "role": "system"})];
    for r in hist {
        let (m, calls) = (&r["msg"], r["msg"]["tool_calls"].as_array().map_or(&[][..], Vec::as_slice));
        msgs.push(match r["t"].as_str() {
            Some("user") => json!({"content": m["content"], "role": "user"}),
            Some("assistant") if calls.is_empty() => json!({"content": m["content"], "role": "assistant"}),
            Some("assistant") => json!({"content": m["content"], "role": "assistant", "tool_calls": calls.iter().map(native).collect::<Vec<_>>()}),
            Some("tool_result") => json!({"content": r["content"], "role": "tool", "tool_call_id": r["id"]}),
            _ => continue,
        });
    }
    let mut b = json!({"chat_template_kwargs": {"enable_thinking": false}, "max_tokens": max_tokens, "messages": msgs, "model": model, "stream": true,
        "stream_options": {"include_usage": true}});
    if tools.as_array().is_some_and(|t| !t.is_empty()) {
        b["tools"] = tools.clone();
    }
    b.to_string().into_bytes()
}

/// A journaled call as a `tool_calls` entry. Arguments that are not an object go back as {}: the call's result already
/// quotes what was sent.
fn native(c: &Value) -> Value {
    let args = if c["args"].is_object() { c["args"].to_string() } else { "{}".into() };
    json!({"function": {"arguments": args, "name": c["name"]}, "id": c["id"], "type": "function"})
}

/// One reply as it arrives. A call slot is (its `index`, the call so far with its arguments' text in `raw`).
pub struct Acc {
    text: String,
    reasoning: String,
    slots: Vec<(Option<u64>, Call)>,
    finish: String,
    usage: Usage,
    offered: Vec<&'static str>,
    seen: bool, // a data event came: an error now is not the first event
}

impl Wire for Acc {
    fn new(offered: &[&'static str]) -> Acc {
        Acc { text: String::new(), reasoning: String::new(), slots: vec![], finish: String::new(), usage: Usage::default(), offered: offered.to_vec(), seen: false }
    }

    /// One SSE `data` payload: `[DONE]` ends it, an empty one is a keep-alive.
    fn data(&mut self, data: &str, part: &mut dyn FnMut(Part)) -> Result<Flow, ProviderError> {
        match data.trim() {
            "[DONE]" => return Ok(Flow::Done),
            "" => {} // an empty event (a keep-alive) changes nothing
            d => {
                let v = serde_json::from_str(d).map_err(|e| bad(&format!("stream data is not JSON ({e})"), d))?;
                let first = !std::mem::replace(&mut self.seen, true);
                self.take(&v, "delta", first, part)?;
            }
        }
        Ok(if self.finish.is_empty() { Flow::More } else { Flow::Finished })
    }

    /// A 200 `application/json` completion: the same rules applied to `choices[0].message`.
    fn json(mut self, body: &[u8]) -> Result<Reply, ProviderError> {
        let v: Value = serde_json::from_slice(body).map_err(|e| bad(&format!("the completion is not JSON ({e})"), &String::from_utf8_lossy(body)))?;
        self.take(&v, "message", true, &mut |_| {})?;
        v["choices"][0]["message"].as_object().ok_or_else(|| bad("the completion has no choices[0].message", &String::from_utf8_lossy(body)))?;
        self.finish(true)
    }

    /// The after-stream rules; `complete` = `[DONE]`, or EOF/idle after a finish.
    fn finish(self, complete: bool) -> Result<Reply, ProviderError> {
        if !complete {
            let names: Vec<&str> = self.slots.iter().map(|(_, c)| call_name(&c.name)).collect();
            let dropped = if names.is_empty() { String::new() } else { format!("; dropped calls: {}", names.join(", ")) };
            return Err(perr("stream_incomplete", format!("stream ended early{dropped}")));
        }
        let length = self.finish == "length";
        let mut calls: Vec<Call> = self.slots.into_iter().map(|(_, c)| c).collect();
        for c in &mut calls {
            set_args(c);
            c.error = length.then(|| CUT_ARGS.into());
        }
        let mut text = self.text;
        if calls.is_empty() && !length {
            (calls, text) = text_calls(&text, &self.offered);
            if calls.is_empty() {
                calls = text_calls(&self.reasoning, &self.offered).0;
            }
        }
        Ok(Reply { text, calls, finish: self.finish, usage: self.usage, blocks: None })
    }
}

impl Acc {
    /// One chunk (`key` = `delta`) or a whole completion (`message`): an error member ends it; else the usage, then
    /// `choices[0]`, its delta before its finish reason.
    fn take(&mut self, v: &Value, key: &str, first: bool, part: &mut dyn FnMut(Part)) -> Result<(), ProviderError> {
        if !v["error"].is_null() {
            return Err(error_object(v, first));
        }
        let (u, us) = (&v["usage"], &mut self.usage);
        for (n, f) in [(&mut us.input, &u["prompt_tokens"]), (&mut us.output, &u["completion_tokens"]), (&mut us.cached, &u["prompt_tokens_details"]["cached_tokens"])] {
            *n = f.as_u64().unwrap_or(*n);
        }
        us.cached = v["timings"]["cache_n"].as_u64().unwrap_or(us.cached); // llama.cpp
        let (ch, d, whole) = (&v["choices"][0], &v["choices"][0][key], key == "message");
        if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) {
            self.text += t;
            part(Part::Text(t.into()));
        }
        if let Some(t) = [&d["reasoning_content"], &d["reasoning"]].into_iter().filter_map(Value::as_str).find(|t| !t.is_empty()) {
            self.reasoning += t;
            part(Part::Reasoning(t.into()));
        }
        // A whole message has a call per entry; a stream keys fragments by `index`, and a later entry of a chunk without one is a new call.
        for (k, tc) in d["tool_calls"].as_array().into_iter().flatten().filter(|tc| tc.is_object()).enumerate() {
            self.call(tc, if whole { Some(k as u64) } else { tc["index"].as_u64() }, k > 0, part);
        }
        if let Some(f) = ch["finish_reason"].as_str() {
            self.finish = f.into();
        }
        Ok(())
    }

    /// One `tool_calls` fragment. With an index: that slot, unless a different id arrives on a slot that has one. Without:
    /// the last slot, unless the fragment brings a new id or is a `later` entry of its chunk. A new slot shows its name.
    fn call(&mut self, tc: &Value, index: Option<u64>, later: bool, part: &mut dyn FnMut(Part)) {
        let id = tc["id"].as_str().unwrap_or("");
        let at = match index {
            Some(_) => self.slots.iter().rposition(|s| s.0 == index).filter(|&i| id.is_empty() || [id, ""].contains(&self.slots[i].1.id.as_str())),
            None => self.slots.len().checked_sub(1).filter(|&i| !later && (id.is_empty() || self.slots[i].1.id == id)),
        };
        let i = at.unwrap_or(self.slots.len());
        if at.is_none() {
            self.slots.push((index, Call::default()));
        }
        let c = &mut self.slots[i].1;
        if c.id.is_empty() {
            c.id = id.into();
        }
        // A name is set when empty, ignored when repeated, else appended (split over fragments).
        let f = &tc["function"];
        if let Some(n) = f["name"].as_str().filter(|n| *n != c.name) {
            c.name += n;
        }
        let raw = c.raw.get_or_insert_default();
        match &f["arguments"] {
            Value::String(a) => raw.push_str(a),
            Value::Null => {}
            a => *raw = a.to_string(),
        }
        if at.is_none() {
            part(Part::Call(call_name(&c.name).into()));
        }
    }
}

/// Every text call in `text`, in text order, and the text without their markup.
pub fn text_calls(text: &str, offered: &[&str]) -> (Vec<Call>, String) {
    if let Some(c) = fenced(text, offered) {
        return (vec![c], String::new());
    }
    // Each candidate is looked at once and markup is skipped whole: one pass over the text.
    let starts = |i: usize| text.get(i..).is_some_and(|r| r.starts_with("<tool_call>") || r.starts_with("<function=")) || fence_at(text, i);
    let (mut calls, mut rest, mut pos) = (vec![], String::new(), 0);
    while let Some(i) = text.get(pos..).and_then(|r| r.match_indices(['<', '`']).map(|(i, _)| pos + i).find(|&i| starts(i))) {
        let at = text.get(i..).unwrap_or_default();
        let (len, found) = markup(at, offered);
        rest.extend([text.get(pos..i).unwrap_or_default(), if found.is_empty() { at.get(..len).unwrap_or_default() } else { "" }]);
        calls.extend(found);
        pos = i + len;
    }
    if calls.is_empty() { (calls, text.into()) } else { (calls, (rest + text.get(pos..).unwrap_or_default()).trim().into()) }
}

/// The markup at the start of `s` (a fence, `<tool_call>` or `<function=`): its length (never 0) and the calls it holds.
/// No call means prose, which stays in the text. A fence runs to the end of the text when it is not closed.
fn markup(s: &str, offered: &[&str]) -> (usize, Vec<Call>) {
    if let Some(r) = s.strip_prefix("```") {
        let (lang, body) = r.split_once('\n').unwrap_or((r, ""));
        let e = body.match_indices("```").map(|(i, _)| i).find(|&i| fence_at(body, i));
        let call = if matches!(lang.trim(), "" | "json") { body.get(..e.unwrap_or(body.len())).and_then(|b| fenced(b, offered)) } else { None };
        return (e.map_or(s.len(), |e| s.len() - body.len() + e + 3), call.into_iter().collect());
    }
    let Some(r) = s.strip_prefix("<tool_call>") else { return qwen(s) };
    let Some((body, _)) = r.split_once("</tool_call>") else { return (s.len(), vec![incomplete(s)]) };
    // Hermes JSON, else each Qwen XML block in it is a call of its own (the body holds no `</tool_call>`: one level deep).
    let calls = serde_json::from_str(body.trim()).ok().and_then(|v| json_call(&v, "parameters")).map_or_else(|| text_calls(body, offered).0, |c| vec![c]);
    (s.len() - r.len() + body.len() + "</tool_call>".len(), if calls.is_empty() { vec![incomplete(body)] } else { calls })
}

/// Qwen XML from `s` to its first `</function>`: `<function=NAME>`, `<parameter=K>V</parameter>`s (V opaque, one leading
/// and one trailing newline dropped, kept a string). A `<function=` where a parameter could start begins the next call and
/// leaves the one before it unclosed; an unclosed or malformed call is incomplete (a malformed one takes the rest).
fn qwen(s: &str) -> (usize, Vec<Call>) {
    let Some(e) = s.find("</function>") else { return (s.len(), vec![incomplete(s)]) };
    let (mut calls, mut b) = (vec![], s.get(..e).unwrap_or_default());
    while let Some(r) = b.strip_prefix("<function=") {
        let ((name, mut r), mut args) = (r.split_once('>').unwrap_or_default(), Map::new());
        while let Some((k, p)) = r.trim_start().strip_prefix("<parameter=").and_then(|p| p.split_once('>')) {
            let Some((v, a)) = p.split_once("</parameter>") else { break };
            let v = v.strip_prefix('\n').unwrap_or(v);
            args.insert(k.trim().into(), v.strip_suffix('\n').unwrap_or(v).into());
            r = a;
        }
        let (r, name) = (r.trim_start(), name.trim());
        let next = if r.starts_with("<function=") { r } else { "" };
        let closed = r.is_empty() && !name.is_empty() && !name.contains(['<', '\n']);
        calls.push(if closed { Call { name: name.into(), args: args.into(), ..Call::default() } } else { incomplete(b.get(..b.len() - next.len()).unwrap_or_default()) });
        b = next;
    }
    (e + "</function>".len(), calls)
}

/// Whether a fence, "```" at the start of a line, begins at `i` in `t`.
fn fence_at(t: &str, i: usize) -> bool {
    t.get(i..).is_some_and(|r| r.starts_with("```")) && t.get(..i).map(|p| p.trim_end_matches([' ', '\t'])).is_some_and(|p| p.is_empty() || p.ends_with('\n'))
}

/// A fence, or the whole text, holding `{name, arguments}`: a call only when the name is offered.
fn fenced(s: &str, offered: &[&str]) -> Option<Call> {
    serde_json::from_str(s.trim()).ok().and_then(|v| json_call(&v, "arguments")).filter(|c| offered.contains(&c.name.as_str()))
}

/// `{"name": N, "arguments": A}` (or `alt` as the arguments' key), A an object or a string holding one.
fn json_call(v: &Value, alt: &str) -> Option<Call> {
    let a = v.get("arguments").or_else(|| v.get(alt))?;
    let mut c = Call { name: v["name"].as_str()?.into(), args: a.clone(), raw: a.as_str().map(String::from), ..Call::default() };
    set_args(&mut c);
    Some(c)
}

/// Markup opened and not closed, or closed around neither JSON nor Qwen XML: a call that does not run, named by the
/// markup when it shows a name (a Qwen XML name ends at `>`, `<` or a newline).
fn incomplete(s: &str) -> Call {
    let xml = s.split_once("<function=").and_then(|(_, r)| r.split(['>', '<', '\n']).next());
    let json = || s.split_once("\"name\"")?.1.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?.split_once('"').map(|(n, _)| n);
    let name = xml.or_else(json).map(str::trim).filter(|n| !n.is_empty()).unwrap_or("unknown");
    Call { name: name.into(), args: json!({}), error: Some(INCOMPLETE.into()), ..Call::default() }
}

/// `args` = the arguments' text in `raw` (if any), parsed; over 1 MiB or not JSON: `{}`, and `raw` stays for validation.
fn set_args(c: &mut Call) {
    let Some(a) = c.raw.take() else { return };
    match (a.len() <= ARGS_MAX).then(|| serde_json::from_str(&a).ok()).flatten() {
        Some(v) => c.args = v,
        None => (c.args, c.raw) = (json!({}), Some(a)),
    }
}

#[cfg(test)]
mod tests;
