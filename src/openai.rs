//! The OpenAI-compatible chat-completions wire format: the request body, the stream accumulator with its defensive
//! call parser, and text calls for servers that leave tool markup in the content.
use crate::http::{error_object, perr};
use crate::types::{Call, Part, ProviderError, Reply, Usage};
use serde_json::{Map, Value, json};

pub const PATH: &str = "/chat/completions";
pub const CUT_ARGS: &str = "arguments cut at max_tokens; not run. Send a shorter call.";
pub const INCOMPLETE: &str = "incomplete tool call";
/// Arguments longer than this are not parsed; validation refuses them.
const ARGS_MAX: usize = 1 << 20;

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

/// Finished: a finish_reason came (2 s idle or EOF completes); Done: `[DONE]`.
pub enum Flow {
    More,
    Finished,
    Done,
}

/// One reply as it arrives. A call slot is (its `index`, the call so far with its arguments' text in `raw`).
#[derive(Default)]
pub struct Acc {
    text: String,
    reasoning: String,
    slots: Vec<(Option<u64>, Call)>,
    finish: String,
    usage: Usage,
    offered: Vec<&'static str>,
    seen: bool, // a data event came: an error now is not the first event
}

impl Acc {
    pub fn new(offered: &[&'static str]) -> Acc {
        Acc { offered: offered.to_vec(), ..Acc::default() }
    }

    /// One SSE `data` payload; streamed pieces go to `part`.
    pub fn data(&mut self, data: &str, part: &mut dyn FnMut(Part)) -> Result<Flow, ProviderError> {
        match data.trim() {
            "[DONE]" => return Ok(Flow::Done),
            "" => {} // an empty event (a keep-alive) changes nothing
            d => {
                let v = serde_json::from_str(d).map_err(|e| not_json("stream data", d, e))?;
                let first = !std::mem::replace(&mut self.seen, true);
                self.take(&v, "delta", first, part)?;
            }
        }
        Ok(if self.finish.is_empty() { Flow::More } else { Flow::Finished })
    }

    /// A 200 `application/json` completion: the same rules applied to `choices[0].message`.
    pub fn json(mut self, body: &[u8]) -> Result<Reply, ProviderError> {
        let v = serde_json::from_slice(body).map_err(|e| not_json("the completion", &String::from_utf8_lossy(body), e))?;
        self.take(&v, "message", true, &mut |_| {})?;
        self.finish(true)
    }

    /// The after-stream rules; `complete` = `[DONE]`, or EOF/idle after a finish.
    pub fn finish(self, complete: bool) -> Result<Reply, ProviderError> {
        if !complete {
            let names: Vec<&str> = self.slots.iter().map(|(_, c)| shown(&c.name)).collect();
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
        Ok(Reply { text, reasoning: self.reasoning, calls, finish: self.finish, usage: self.usage })
    }

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
        let (ch, whole) = (&v["choices"][0], key == "message");
        let d = &ch[key];
        if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) {
            self.text += t;
            part(Part::Text(t.into()));
        }
        if let Some(t) = [&d["reasoning_content"], &d["reasoning"]].into_iter().filter_map(Value::as_str).find(|t| !t.is_empty()) {
            self.reasoning += t;
            part(Part::Reasoning(t.into()));
        }
        // A whole message lists one call per entry; a stream keys its fragments by `index` when it sends one.
        for (k, tc) in d["tool_calls"].as_array().into_iter().flatten().enumerate() {
            self.call(tc, if whole { Some(k as u64) } else { tc["index"].as_u64() }, part);
        }
        if let Some(f) = ch["finish_reason"].as_str() {
            self.finish = f.into();
        }
        Ok(())
    }

    /// One `tool_calls` fragment. With an index: that slot, unless a different id arrives on a slot that has one. Without:
    /// the last slot, unless the fragment brings a new id. A new slot is announced with its name so far.
    fn call(&mut self, tc: &Value, index: Option<u64>, part: &mut dyn FnMut(Part)) {
        let id = tc["id"].as_str().unwrap_or("");
        let at = match index {
            Some(_) => self.slots.iter().rposition(|s| s.0 == index).filter(|&i| id.is_empty() || [id, ""].contains(&self.slots[i].1.id.as_str())),
            None => self.slots.len().checked_sub(1).filter(|&i| id.is_empty() || self.slots[i].1.id == id),
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
            part(Part::Call(shown(&c.name).into()));
        }
    }
}

/// Every text call in `text`, in text order, and the text without their markup.
pub fn text_calls(text: &str, offered: &[&str]) -> (Vec<Call>, String) {
    if let Some(c) = fenced(text, offered) {
        return (vec![c], String::new());
    }
    let (mut calls, mut rest, mut s) = (vec![], String::new(), text);
    while let Some(i) = [s.find("<tool_call>"), s.find("<function="), fence(s)].into_iter().flatten().min() {
        let Some((before, at)) = s.split_at_checked(i) else { break };
        let (len, call) = markup(at, offered);
        let Some((m, after)) = at.split_at_checked(len) else { break };
        rest.extend([before, if call.is_some() { "" } else { m }]);
        calls.extend(call);
        s = after;
    }
    if calls.is_empty() { (calls, text.into()) } else { (calls, (rest + s).trim().into()) }
}

/// The markup at the start of `s` (a fence, `<tool_call>` or `<function=`): its length and the call it holds. No call
/// means prose, which stays in the text. A fence runs to the end of the text when it is not closed.
fn markup(s: &str, offered: &[&str]) -> (usize, Option<Call>) {
    if let Some(r) = s.strip_prefix("```") {
        let (lang, body) = r.split_once('\n').unwrap_or((r, ""));
        let e = fence(body);
        let call = if matches!(lang.trim(), "" | "json") { body.get(..e.unwrap_or(body.len())).and_then(|b| fenced(b, offered)) } else { None };
        return (e.map_or(s.len(), |e| s.len() - body.len() + e + 3), call);
    }
    let tagged = s.starts_with("<tool_call>");
    let close = if tagged { "</tool_call>" } else { "</function>" };
    let Some(e) = s.find(close) else { return (s.len(), Some(incomplete(s))) };
    let body = if tagged { s.get(11..e) } else { s.get(..e + close.len()) }.unwrap_or_default().trim();
    let call = serde_json::from_str(body).ok().and_then(|v| json_call(&v, "parameters")).or_else(|| qwen(body)).unwrap_or_else(|| incomplete(body));
    (e + close.len(), Some(call))
}

/// The first "```" in `s` that starts a line (`s` itself starts one).
fn fence(s: &str) -> Option<usize> {
    let starts_line = |i: usize| s.get(..i).map(|p| p.trim_end_matches([' ', '\t'])).is_some_and(|p| p.is_empty() || p.ends_with('\n'));
    s.match_indices("```").map(|(i, _)| i).find(|&i| starts_line(i))
}

/// A fence, or the whole text, holding `{name, arguments}`: a call only when the name is offered.
fn fenced(s: &str, offered: &[&str]) -> Option<Call> {
    serde_json::from_str(s.trim()).ok().and_then(|v| json_call(&v, "arguments")).filter(|c| offered.contains(&c.name.as_str()))
}

/// `{"name": N, "arguments": A}` (or `alt` as the arguments' key), A an object or a string holding one.
fn json_call(v: &Value, alt: &str) -> Option<Call> {
    let a = v.get("arguments").or_else(|| v.get(alt))?;
    let mut c = Call { name: v["name"].as_str()?.into(), args: a.clone(), raw: a.as_str().map(String::from), ..Call::default() };
    if c.raw.is_some() {
        set_args(&mut c);
    }
    Some(c)
}

/// `<function=NAME><parameter=K>V</parameter>…</function>`; V loses one leading and one trailing newline.
fn qwen(s: &str) -> Option<Call> {
    let (name, mut rest) = s.strip_prefix("<function=")?.strip_suffix("</function>")?.split_once('>')?;
    let mut args = Map::new();
    while let Some((_, r)) = rest.split_once("<parameter=") {
        let (k, r) = r.split_once('>')?;
        let (v, r) = r.split_once("</parameter>")?;
        let v = v.strip_prefix('\n').unwrap_or(v);
        args.insert(k.trim().into(), v.strip_suffix('\n').unwrap_or(v).into());
        rest = r;
    }
    Some(Call { name: name.trim().into(), args: args.into(), ..Call::default() })
}

/// Markup opened and not closed, or closed around neither JSON nor Qwen XML: a call that does not run, named by the
/// markup when it shows a name.
fn incomplete(s: &str) -> Call {
    let xml = s.split_once("<function=").and_then(|(_, r)| r.split_once('>')).map(|(n, _)| n);
    let json = || s.split_once("\"name\"")?.1.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?.split_once('"').map(|(n, _)| n);
    let name = xml.or_else(json).map(str::trim).filter(|n| !n.is_empty()).unwrap_or("unknown");
    Call { name: name.into(), args: json!({}), error: Some(INCOMPLETE.into()), ..Call::default() }
}

/// `args` = the arguments' text in `raw`, parsed; over 1 MiB or not JSON: `{}`, and `raw` stays for validation to name.
fn set_args(c: &mut Call) {
    let a = c.raw.take().unwrap_or_default();
    if a.len() <= ARGS_MAX
        && let Ok(v) = serde_json::from_str(&a)
    {
        c.args = v;
    } else {
        (c.args, c.raw) = (json!({}), Some(a));
    }
}

fn shown(name: &str) -> &str {
    if name.is_empty() { "unknown" } else { name }
}

fn not_json(what: &str, data: &str, e: serde_json::Error) -> ProviderError {
    perr("provider", format!("{what} is not JSON ({e}): {}", data.chars().take(200).collect::<String>()))
}

#[cfg(test)]
mod tests;
