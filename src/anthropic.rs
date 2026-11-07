//! The Anthropic Messages wire format, for boxes behind their model gateway: the request body rendered from journal
//! records (a reply's content blocks replayed exactly as they came), and the stream accumulator.
use crate::types::{Flow, Part, ProviderError, Reply, Wire};
use serde_json::Value;
use std::time::Duration;

pub const PATH: &str = "/v1/messages";
pub const VERSION: &str = "2023-06-01";
pub const REFUSAL: &str = "the model declined the request (refusal)";
/// The stall limit for this dialect: the API can be silent for minutes while it buffers a large tool input.
pub const STALL: Duration = Duration::from_secs(300);

/// `anthropic-version: VERSION`, and `x-api-key: KEY` when `key` is not empty.
pub fn headers(key: &str) -> Vec<(&'static str, String)> {
    let _ = key;
    todo!()
}

/// The whole request body from journal records: compact JSON, keys sorted, old messages byte-identical every time.
/// `tools` is `tools::schema` (OpenAI form); it is rendered as `{description, input_schema, name}`.
pub fn body(model: &str, max_tokens: u64, system: &str, tools: &Value, hist: &[Value]) -> Vec<u8> {
    let _ = (model, max_tokens, system, tools, hist);
    todo!()
}

/// One reply as it arrives: blocks by index, tool input text by index, stop reason, usage, seen.
pub struct Acc {}

impl Wire for Acc {
    fn new(offered: &[&'static str]) -> Acc {
        let _ = offered;
        todo!()
    }

    fn data(&mut self, data: &str, part: &mut dyn FnMut(Part)) -> Result<Flow, ProviderError> {
        let _ = (data, part);
        todo!()
    }

    fn json(self, body: &[u8]) -> Result<Reply, ProviderError> {
        let _ = body;
        todo!()
    }

    fn finish(self, complete: bool) -> Result<Reply, ProviderError> {
        let _ = complete;
        todo!()
    }
}
