//! The Anthropic Messages wire format, for boxes behind their model gateway: the request body rendered from journal
//! records (a reply's content blocks replayed exactly as they came), and the stream accumulator.
use crate::http::{bad, error_object, perr};
use crate::types::{ARGS_MAX, CUT_ARGS, Call, Flow, Part, ProviderError, Reply, Usage, Wire, call_name};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};

pub const PATH: &str = "/v1/messages";
pub const VERSION: &str = "2023-06-01";
pub const REFUSAL: &str = "the model declined the request (refusal)";
/// The stall limit for this dialect: the API can be silent for minutes while it buffers a large tool input.
pub const STALL: Duration = Duration::from_secs(300);

/// `anthropic-version: VERSION`, and `x-api-key: KEY` when `key` is not empty.
pub fn headers(key: &str) -> Vec<(&'static str, String)> {
    let key = (!key.is_empty()).then(|| ("x-api-key", key.to_string()));
    [("anthropic-version", VERSION.to_string())].into_iter().chain(key).collect()
}

/// The whole request body from journal records: compact JSON, keys sorted, old messages byte-identical every time.
/// `tools` is in the OpenAI form; it is rendered as `{description, input_schema, name}`. Content is always a block
/// list: text blocks with no text but whitespace and messages left empty are dropped, and adjacent messages of one role
/// are merged, so tool results and the next task form one user message, results first.
pub fn body(model: &str, max_tokens: u64, system: &str, tools: &Value, hist: &[Value]) -> Vec<u8> {
    let mut msgs: Vec<(&str, Vec<Value>)> = Vec::new();
    for r in hist {
        let (role, blocks) = match r["t"].as_str() {
            Some("user") => ("user", vec![json!({"text": r["msg"]["content"], "type": "text"})]),
            Some("assistant") => ("assistant", r["blocks"].as_array().cloned().unwrap_or_else(|| partial(r))),
            Some("tool_result") => ("user", vec![json!({"content": r["content"], "is_error": r["is_error"], "tool_use_id": r["id"], "type": "tool_result"})]),
            _ => continue,
        };
        let mut blocks: Vec<Value> = blocks.into_iter().filter(|b| !empty_text(b)).collect();
        match msgs.last_mut() {
            Some((last, list)) if *last == role => list.append(&mut blocks),
            _ if !blocks.is_empty() => msgs.push((role, blocks)),
            _ => {}
        }
    }
    let msgs: Vec<Value> = msgs.into_iter().map(|(role, content)| json!({"content": content, "role": role})).collect();
    let cache = json!({"type": "ephemeral"});
    let mut b = json!({"cache_control": cache, "max_tokens": max_tokens, "messages": msgs, "model": model, "stream": true,
        "system": [{"cache_control": cache, "text": system, "type": "text"}]});
    let tool = |f: &Value| json!({"description": f["description"], "input_schema": f["parameters"], "name": f["name"]});
    let tools: Vec<Value> = tools.as_array().into_iter().flatten().map(|t| tool(&t["function"])).collect();
    if !tools.is_empty() {
        b["tools"] = tools.into();
    }
    b.to_string().into_bytes()
}

/// A reply journaled without its blocks (cut short): its text, then a tool_use per call, the input `{}` unless the
/// arguments are an object.
fn partial(r: &Value) -> Vec<Value> {
    let calls = r["msg"]["tool_calls"].as_array().into_iter().flatten();
    let uses = calls.map(|c| json!({"id": c["id"], "input": if c["args"].is_object() { c["args"].clone() } else { json!({}) }, "name": c["name"], "type": "tool_use"}));
    std::iter::once(json!({"text": r["msg"]["content"], "type": "text"})).chain(uses).collect()
}

/// A text block with no text but whitespace: the API refuses it, and ignores it when it compares a replayed history.
fn empty_text(b: &Value) -> bool {
    b["type"] == "text" && b["text"].as_str().is_none_or(|t| t.trim().is_empty())
}

/// One reply as it arrives: every content block by its index, as started and grown by its deltas, with the input text
/// of a tool_use block so far.
pub struct Acc {
    blocks: BTreeMap<u64, (Value, String)>,
    stop: Option<String>, // message_delta came: the reply is finished
    usage: Value,         // each count as last sent
    seen: bool,           // a part streamed: an error now is not retried, as the retry would repeat it
}

impl Wire for Acc {
    fn new(_offered: &[&'static str]) -> Acc {
        Acc { blocks: BTreeMap::new(), stop: None, usage: Value::Null, seen: false }
    }

    /// One SSE `data` payload, by its `type`; `ping`, `content_block_stop`, unknown types and a delta of another block's
    /// type change nothing. An `error` is retryable only while no part has streamed.
    fn data(&mut self, data: &str, part: &mut dyn FnMut(Part)) -> Result<Flow, ProviderError> {
        let v: Value = serde_json::from_str(data).map_err(|e| bad(&format!("stream data is not JSON ({e})"), data))?;
        let (first, at) = (!self.seen, v["index"].as_u64());
        // A part reaches the user: from here on a retry would show it twice.
        let part = &mut |p| self.seen = (part(p), true).1;
        match v["type"].as_str().unwrap_or_default() {
            "message_start" => merge(&mut self.usage, &v["message"]["usage"]),
            "content_block_start" => {
                let b = &v["content_block"];
                let Some(i) = at.filter(|i| b.is_object() && !self.blocks.contains_key(i)) else { return Err(bad("a content block start without an object or a new index", data)) };
                if b["type"] == "tool_use" {
                    part(Part::Call(call_name(b["name"].as_str().unwrap_or_default()).into()));
                }
                self.blocks.insert(i, (b.clone(), String::new()));
            }
            "content_block_delta" => {
                let Some((b, input)) = at.and_then(|i| self.blocks.get_mut(&i)) else { return Err(bad("a delta for a content block that never started", data)) };
                let d = |k: &str| v["delta"][k].as_str().unwrap_or_default();
                match d("type") {
                    "text_delta" if b["type"] == "text" && !d("text").is_empty() => part(Part::Text(append(b, "text", d("text")))),
                    "thinking_delta" if b["type"] == "thinking" && !d("thinking").is_empty() => part(Part::Reasoning(append(b, "thinking", d("thinking")))),
                    "signature_delta" if b["type"] == "thinking" => _ = append(b, "signature", d("signature")),
                    "input_json_delta" if b["type"] == "tool_use" => input.push_str(d("partial_json")),
                    _ => {}
                }
            }
            "message_delta" => {
                self.stop = Some(v["delta"]["stop_reason"].as_str().unwrap_or_default().into());
                merge(&mut self.usage, &v["usage"]);
            }
            "message_stop" => return Ok(Flow::Done),
            "error" => return Err(error_object(&v, first)),
            _ => {}
        }
        Ok(if self.stop.is_some() { Flow::Finished } else { Flow::More })
    }

    /// The gateway always streams; a JSON completion is refused.
    fn json(self, _body: &[u8]) -> Result<Reply, ProviderError> {
        Err(perr("provider", "the model server answered without a stream"))
    }

    /// The after-stream rules; `complete` = `message_stop` came, or EOF or idle after `message_delta`. A cut reply
    /// (`max_tokens`, or the context window full) runs none of its calls.
    fn finish(self, complete: bool) -> Result<Reply, ProviderError> {
        if !complete {
            let names: Vec<&str> = self.blocks.values().filter(|(b, _)| b["type"] == "tool_use").map(|(b, _)| call_name(b["name"].as_str().unwrap_or_default())).collect();
            let dropped = if names.is_empty() { String::new() } else { format!("; dropped calls: {}", names.join(", ")) };
            return Err(perr("stream_incomplete", format!("stream ended early{dropped}")));
        }
        let stop = self.stop.unwrap_or_default();
        if stop == "refusal" {
            return Err(perr("provider", REFUSAL));
        }
        let length = matches!(stop.as_str(), "max_tokens" | "model_context_window_exceeded");
        // Input counts every prompt token, cache reads and writes included; `cached` is the reads.
        let n = |k: &str| self.usage[k].as_u64().unwrap_or(0);
        let input = n("input_tokens").saturating_add(n("cache_read_input_tokens")).saturating_add(n("cache_creation_input_tokens"));
        let usage = Usage { input, output: n("output_tokens"), cached: n("cache_read_input_tokens") };
        let (mut r, mut blocks) = (Reply { usage, ..Reply::default() }, Vec::new());
        for (mut b, input) in self.blocks.into_values() {
            match b["type"].as_str().unwrap_or_default() {
                "text" => r.text += b["text"].as_str().unwrap_or_default(),
                "tool_use" => {
                    // Without deltas, the input it started with; over 1 MiB or not JSON, `{}` and the text kept for validation to quote.
                    let input = if input.is_empty() { b.get("input").filter(|i| i.is_object()).map_or_else(|| "{}".into(), Value::to_string) } else { input };
                    let args = (input.len() <= ARGS_MAX).then(|| serde_json::from_str::<Value>(&input).ok()).flatten();
                    b["input"] = args.clone().filter(Value::is_object).unwrap_or_else(|| json!({}));
                    let (id, name) = (b["id"].as_str().unwrap_or_default().into(), b["name"].as_str().unwrap_or_default().into());
                    r.calls.push(Call { id, name, raw: args.is_none().then_some(input), args: args.unwrap_or_else(|| json!({})), error: length.then(|| CUT_ARGS.into()) });
                }
                _ => {}
            }
            blocks.extend((!empty_text(&b)).then_some(b));
        }
        r.finish = if length { "length".into() } else { (if r.calls.is_empty() { "stop" } else { "tool_calls" }).into() };
        r.blocks = Some(blocks.into());
        Ok(r)
    }
}

/// A `usage` object: each count it carries replaces the one before (`message_delta` sends the final ones).
fn merge(us: &mut Value, u: &Value) {
    u.as_object().into_iter().flatten().filter(|(_, n)| n.is_u64()).for_each(|(k, n)| us[k] = n.clone());
}

/// Appends `t` to the string field `k` of the block (an object) in place, a missing or other field becoming `t`; returns
/// `t` for the streamed part.
fn append(b: &mut Value, k: &str, t: &str) -> String {
    match b.get_mut(k) {
        Some(Value::String(s)) => s.push_str(t),
        _ => b[k] = t.into(),
    }
    t.into()
}

#[cfg(test)]
mod tests;
