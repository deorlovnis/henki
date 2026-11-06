//! The OpenAI-compatible chat-completions wire format: the request body, the stream accumulator with its defensive
//! call parser, and text calls for servers that leave tool markup in the content.
use crate::types::{Call, Part, ProviderError, Reply};
use serde_json::Value;

pub const PATH: &str = "/chat/completions";
pub const CUT_ARGS: &str = "arguments cut at max_tokens; not run. Send a shorter call.";
pub const INCOMPLETE: &str = "incomplete tool call";

/// `Authorization: Bearer KEY` only when `key` is not empty.
pub fn headers(key: &str) -> Vec<(&'static str, String)> {
    todo!()
}

/// The whole request body from journal records: compact JSON, keys sorted, old messages byte-identical every time.
pub fn body(model: &str, max_tokens: u64, system: &str, tools: &Value, hist: &[Value]) -> Vec<u8> {
    todo!()
}

/// Finished: a finish_reason came (2 s idle or EOF completes); Done: `[DONE]`.
pub enum Flow {
    More,
    Finished,
    Done,
}

pub struct Acc {
    // private: text, reasoning, slots (index, id, name, args), finish, usage, offered, seen
}

impl Acc {
    pub fn new(offered: &[&'static str]) -> Acc {
        todo!()
    }

    /// One SSE `data` payload; streamed pieces go to `part`.
    pub fn data(&mut self, data: &str, part: &mut dyn FnMut(Part)) -> Result<Flow, ProviderError> {
        todo!()
    }

    /// A 200 `application/json` completion: the same rules applied to `choices[0].message`.
    pub fn json(self, body: &[u8]) -> Result<Reply, ProviderError> {
        todo!()
    }

    /// The after-stream rules; `complete` = `[DONE]`, or EOF/idle after a finish.
    pub fn finish(self, complete: bool) -> Result<Reply, ProviderError> {
        todo!()
    }
}

/// Every text call in `text`, in text order, and the text without their markup.
pub fn text_calls(text: &str, offered: &[&str]) -> (Vec<Call>, String) {
    todo!()
}
