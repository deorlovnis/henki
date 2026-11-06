//! HTTP/1.1 to the model server: one POST per attempt, every resolved address, bounded heads, chunked and SSE bodies,
//! socket timeouts, and the error classification shared by both wire formats.
use crate::types::{ProviderError, Url};
use serde_json::Value;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `request` is one whole attempt, enforced by MAIN (`turn`), not by this module. Tests shorten any of them.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub connect: Duration,
    pub first_byte: Duration,
    pub stall: Duration,
    pub idle_done: Duration,
    pub request: Duration,
}

pub const LIMITS: Limits = Limits {
    connect: Duration::from_secs(5),
    first_byte: Duration::from_secs(300),
    stall: Duration::from_secs(60),
    idle_done: Duration::from_secs(2),
    request: Duration::from_secs(1800),
};
pub const HEAD_MAX: usize = 64 << 10;
pub const CHUNK_LINE_MAX: usize = 1 << 10;
pub const SSE_LINE_MAX: usize = 1 << 20;
pub const ERR_BODY_MAX: usize = 64 << 10;
pub const JSON_MAX: usize = 4 << 20; // a non-streamed 200 completion

/// (cancelled, a clone of the attempt's socket): MAIN cancels through it.
pub type Slot = Arc<Mutex<(bool, Option<TcpStream>)>>;

/// Marks the slot cancelled and shuts the stored socket down (both ways). A later connect sees the mark and shuts itself.
pub fn cancel(slot: &Slot) {
    todo!()
}

pub struct Resp {
    pub sse: bool,
    // private: the socket, a buffered reader, chunked/length state, started, finished
}

/// Resolve `url.host:url.port`; try every address in resolver order, each with `lim.connect`; when `loopback_only`,
/// refuse unless every resolved address is loopback. Store a socket clone in `slot`. Write the request in one write.
/// Read the status line and headers (≤ HEAD_MAX). Ok only for 200 with `text/event-stream` (sse) or `application/json`.
pub fn post(url: &Url, suffix: &str, headers: &[(&'static str, String)], body: &[u8], loopback_only: bool, lim: &Limits, slot: &Slot) -> Result<Resp, ProviderError> {
    todo!()
}

impl Resp {
    /// The next SSE event as (event, data); None at EOF, or after `lim.idle_done` of silence once `finished`.
    pub fn event(&mut self, finished: bool, lim: &Limits) -> Result<Option<(String, String)>, ProviderError> {
        todo!()
    }

    /// The body (JSON completion or error), at most `max` bytes.
    pub fn body(&mut self, max: usize) -> Vec<u8> {
        todo!()
    }
}

/// The error table's rows 1-7, first match wins, for a non-200 response.
pub fn classify(status: u16, body: &[u8], url: &Url, retry_after: Option<Duration>) -> ProviderError {
    todo!()
}

/// The same rules for an `error` object found inside a 200 stream or completion; `first` = it was the first event.
pub fn error_object(v: &Value, first: bool) -> ProviderError {
    todo!()
}

/// The wait before attempt `attempt + 1`: Retry-After capped at 60 s, else min(2^(attempt-1), 30) s.
pub fn retry_wait(attempt: u32, retry_after: Option<Duration>) -> Duration {
    todo!()
}

pub fn perr(kind: &'static str, message: impl Into<String>) -> ProviderError {
    ProviderError { kind, message: message.into(), retry: None, retry_after: None }
}

/// `localhost`, a dotted quad in 127.0.0.0/8 (no trailing dot, no other IPv4 forms), or `::1`.
pub fn loopback_literal(host: &str) -> bool {
    todo!()
}
