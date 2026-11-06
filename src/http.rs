//! HTTP/1.1 to the model server: one POST per attempt, every resolved address, bounded heads, chunked and SSE bodies,
//! socket timeouts, and the error classification shared by both wire formats.
use crate::types::{ProviderError, Url};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

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
const HEAD_OVER: &str = "a response head over 64 KiB";

/// (cancelled, a clone of the attempt's socket): MAIN cancels through it.
pub type Slot = Arc<Mutex<(bool, Option<TcpStream>)>>;

/// Marks the slot cancelled and shuts the stored socket down (both ways). A later connect sees the mark and shuts itself.
pub fn cancel(slot: &Slot) {
    let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
    g.0 = true;
    g.1.iter().for_each(|s| drop(s.shutdown(Shutdown::Both)));
}

pub struct Resp {
    pub sse: bool,
    r: BufReader<Body>,
}

/// The socket with the read timeout of the moment: the first body byte is due by `first`, then each read waits `stall`,
/// and once the caller saw the end of the reply a silence of `idle_done` reads as the end of the body.
struct Sock {
    s: TcpStream,
    first: Instant,
    started: bool,
    finished: bool,
    lim: Limits,
}

/// The response body: chunked (`left` is the rest of the current chunk), `Content-Length` (`left` is the rest of the
/// body), or up to the close (`left` = u64::MAX). After the last chunk it is no longer `chunked`, with nothing left.
struct Body {
    r: BufReader<Sock>,
    chunked: bool,
    left: u64,
}

/// Resolve `url.host:url.port`; try every address in resolver order, each with `lim.connect`; when `loopback_only`,
/// refuse unless every resolved address is loopback. Store a socket clone in `slot`. Write the request in one write.
/// Read the status line and headers (≤ HEAD_MAX). Ok only for 200 with `text/event-stream` (sse) or `application/json`.
pub fn post(url: &Url, suffix: &str, headers: &[(&'static str, String)], body: &[u8], loopback_only: bool, lim: &Limits, slot: &Slot) -> Result<Resp, ProviderError> {
    let hp = host_port(url);
    let addrs: Vec<SocketAddr> = (url.host.trim_matches(['[', ']']), url.port).to_socket_addrs().map(Iterator::collect).unwrap_or_default();
    if loopback_only && !(loopback_literal(&url.host) && addrs.iter().all(|a| a.ip().is_loopback())) {
        return Err(perr("config", format!("{hp} is not a loopback address; a key or HENKI_BASE_URL is only sent to this machine")));
    }
    let Some(mut s) = connect(&addrs, lim.connect) else {
        let n = addrs.len();
        let cause = format!("could not connect to {hp}");
        let message = format!("{cause} ({n} address{}, {} s each)", if n == 1 { "" } else { "es" }, lim.connect.as_secs());
        return Err(ProviderError { kind: "provider", message, retry: Some(cause), retry_after: None });
    };
    let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if g.0 {
        return Err(perr("provider", "cancelled")); // dropping the only handle closes the socket
    }
    g.1 = s.try_clone().ok();
    drop(g);
    let (path, version, n) = (&url.path, env!("CARGO_PKG_VERSION"), body.len());
    let req = format!("POST {path}{suffix} HTTP/1.1\r\nHost: {hp}\r\nUser-Agent: henki/{version}\r\nContent-Type: application/json\r\nContent-Length: {n}\r\n");
    let req = headers.iter().fold(req + "Accept: text/event-stream\r\nConnection: close\r\n", |r, (k, v)| r + k + ": " + v + "\r\n");
    let reset = || ProviderError { kind: "provider", message: format!("connection reset by {hp} before a response"), retry: Some("connection reset".into()), retry_after: None };
    s.set_write_timeout(Some(lim.stall)).and_then(|_| s.write_all(&[req.as_bytes(), b"\r\n", body].concat())).map_err(|_| reset())?;
    let mut r = BufReader::new(Sock { s, first: Instant::now() + lim.first_byte, started: false, finished: false, lim: *lim });
    let head = |r: &mut BufReader<Sock>, max: usize| line(r, max, HEAD_OVER).map_err(|e| io_err(r.get_ref(), e));
    let first = String::from_utf8_lossy(&head(&mut r, HEAD_MAX)?.ok_or_else(reset)?).into_owned();
    let status = first.strip_prefix("HTTP/").and_then(|v| v.split_whitespace().nth(1)?.parse::<u16>().ok());
    let status = status.ok_or_else(|| perr("provider", format!("not an HTTP response from {hp}")))?;
    let (mut room, mut ctype, mut enc, mut len, mut chunked, mut ra) = (HEAD_MAX.saturating_sub(first.len()), String::new(), String::new(), None, false, None);
    let closed = || perr("provider", "the connection closed inside the response head");
    while let Some(h) = Some(head(&mut r, room)?.ok_or_else(closed)?).filter(|h| !h.is_empty()) {
        room = room.saturating_sub(h.len() + 2);
        let h = String::from_utf8_lossy(&h).to_ascii_lowercase();
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        match (k.trim(), v.trim()) {
            ("content-type", v) => ctype = v.split(';').next().unwrap_or_default().trim().to_string(),
            ("content-encoding", v) => enc = v.to_string(),
            ("transfer-encoding", v) => chunked = v.contains("chunked"),
            ("content-length", v) => len = v.parse::<u64>().ok(),
            ("retry-after", v) => ra = retry_after(v),
            _ => {}
        }
    }
    let left = if chunked { 0 } else { len.unwrap_or(u64::MAX) };
    let mut resp = Resp { sse: ctype == "text/event-stream", r: BufReader::new(Body { r, chunked, left }) };
    match status {
        _ if !matches!(enc.as_str(), "" | "identity") => Err(perr("provider", format!("the model server sent Content-Encoding {enc}; henki reads only identity"))),
        200 if resp.sse || ctype == "application/json" => Ok(resp),
        200 => Err(perr("provider", format!("HTTP 200 from http://{hp}{path}{suffix} with Content-Type {ctype:?}"))),
        _ => Err(classify(status, &resp.body(ERR_BODY_MAX), url, ra)), // 3xx included: never followed
    }
}

impl Resp {
    /// The next SSE event as (event, data); None at EOF, or after `lim.idle_done` of silence once `finished`.
    pub fn event(&mut self, finished: bool, lim: &Limits) -> Result<Option<(String, String)>, ProviderError> {
        let k = self.r.get_mut().r.get_mut();
        (k.finished, k.lim) = (finished, *lim);
        let (mut event, mut data) = (String::new(), None::<String>);
        while let Some(l) = line(&mut self.r, SSE_LINE_MAX, "an event-stream line over 1 MiB").map_err(|e| io_err(self.r.get_ref().r.get_ref(), e))? {
            let l = String::from_utf8_lossy(&l);
            let (k, v) = l.split_once(':').unwrap_or((&l, ""));
            let v = v.strip_prefix(' ').unwrap_or(v);
            match k {
                _ if l.is_empty() && data.is_some() => break,
                _ if l.is_empty() => event.clear(),
                "event" => event = v.to_string(),
                "data" => data = Some(data.map_or_else(|| v.to_string(), |d| d + "\n" + v)),
                _ => {} // a comment (its bytes already reset the stall timer), or a field henki does not use
            }
        }
        Ok(data.map(|d| (event, d)))
    }

    /// The body (JSON completion or error), at most `max` bytes.
    pub fn body(&mut self, max: usize) -> Vec<u8> {
        let mut v = Vec::new();
        let _ = self.r.by_ref().take(max as u64).read_to_end(&mut v); // on an error the bytes so far are kept
        v
    }
}

/// A read timeout becomes the first-token or the stall message; anything else keeps its own words.
fn io_err(k: &Sock, e: io::Error) -> ProviderError {
    let (what, t) = if k.started { ("no data from the model for", k.lim.stall) } else { ("no first token within", k.lim.first_byte) };
    perr("provider", if e.kind() == ErrorKind::TimedOut { format!("{what} {} s", t.as_secs()) } else { e.to_string() })
}

impl Read for Sock {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let t = if self.started { self.lim.stall } else { self.first.saturating_duration_since(Instant::now()) };
        self.s.set_read_timeout(Some(if self.finished { self.lim.idle_done } else { t.max(Duration::from_millis(1)) }))?;
        match self.s.read(b) {
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) && !self.finished => Err(ErrorKind::TimedOut.into()),
            Err(e) if e.kind() == ErrorKind::Interrupted => Err(e),
            r => Ok(r.unwrap_or(0)), // a reset, a shutdown by `cancel`, or the silence after the end: the body ends here
        }
    }
}

impl Read for Body {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.chunked && self.left == 0 {
            let mut l = Some(Vec::new());
            while l.as_ref().is_some_and(Vec::is_empty) {
                l = line(&mut self.r, CHUNK_LINE_MAX, "a chunk size line over 1 KiB")?; // a blank one ends the previous chunk's data
            }
            let hex = String::from_utf8_lossy(l.as_deref().unwrap_or(b"0")).split(';').next().unwrap_or_default().trim().to_string();
            self.left = u64::from_str_radix(&hex, 16).map_err(|_| io::Error::other("a bad chunk size line"))?;
            self.chunked = self.left > 0;
        }
        let max = self.left.min(b.len() as u64) as usize;
        let n = if max == 0 { 0 } else { self.r.read(&mut b[..max])? }; // never read into an empty slice: BufReader would wait

        self.left -= n as u64;
        self.r.get_mut().started |= n > 0;
        Ok(n)
    }
}

/// The next line without its `\n` and one `\r`; None at EOF. Err when it is longer than `max`.
fn line(r: &mut impl BufRead, max: usize, over: &'static str) -> io::Result<Option<Vec<u8>>> {
    let mut l = Vec::new();
    if r.take(max as u64 + 2).read_until(b'\n', &mut l)? == 0 {
        return Ok(None);
    }
    l.truncate(l.len() - usize::from(l.ends_with(b"\n")));
    l.truncate(l.len() - usize::from(l.ends_with(b"\r")));
    if l.len() > max { Err(io::Error::other(over)) } else { Ok(Some(l)) }
}

fn connect(addrs: &[SocketAddr], t: Duration) -> Option<TcpStream> {
    addrs.iter().find_map(|a| TcpStream::connect_timeout(a, t).ok())
}

fn host_port(url: &Url) -> String {
    if url.host.contains(':') && !url.host.starts_with('[') { format!("[{}]:{}", url.host, url.port) } else { format!("{}:{}", url.host, url.port) }
}

/// Integer or decimal seconds; anything else (an HTTP-date) counts as absent.
fn retry_after(v: &str) -> Option<Duration> {
    v.parse::<f64>().ok().and_then(|s| Duration::try_from_secs_f64(s).ok())
}

/// The error table's rows 1-7, first match wins, for a non-200 response.
pub fn classify(status: u16, body: &[u8], url: &Url, retry_after: Option<Duration>) -> ProviderError {
    let text = String::from_utf8_lossy(body);
    if text.starts_with("halti: ") {
        return perr("gateway", text.lines().next().unwrap_or_default());
    }
    let v = serde_json::from_slice(body).unwrap_or(Value::Null);
    rows(status, &v, &text, format!("HTTP {status} from http://{}{}: ", host_port(url), url.path), retry_after)
}

/// The same rules for an `error` object found inside a 200 stream or completion; `first` = it was the first event.
pub fn error_object(v: &Value, first: bool) -> ProviderError {
    let text = v.to_string();
    let mut e = rows(0, v, &text, "the model sent an error: ".into(), None);
    e.retry = e.retry.filter(|_| first);
    e
}

/// Rows 2, 3, 4 and 7 over an error body or event (`status` 0: inside a 200 reply). The message is the server's
/// `error.message` (OpenAI and Anthropic shapes alike), else `prefix` and the first 200 bytes of `text`.
fn rows(status: u16, v: &Value, text: &str, prefix: String, retry_after: Option<Duration>) -> ProviderError {
    let e = v.get("error").unwrap_or(v);
    let ty = e.get("type").and_then(Value::as_str).unwrap_or_default();
    let low = text.to_lowercase();
    let context = matches!(status, 400 | 413) && ["context", "exceed", "too long", "too many tokens"].iter().any(|w| low.contains(w));
    let (kind, retry) = match (status, ty) {
        _ if context => ("context_exceeded", None),
        (401 | 403, _) | (_, "authentication_error" | "permission_error") => ("auth", None),
        (408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529, _) => ("provider", Some(format!("HTTP {status}"))),
        (_, "rate_limit_error" | "overloaded_error" | "api_error") => ("provider", Some(ty.to_string())),
        _ => ("provider", None),
    };
    let fix = if kind == "auth" && low.contains("blocked:no-standin") { "; fix: set api_key to the gateway's stand-in" } else { "" };
    let message = match e.get("message").and_then(Value::as_str).or(e.as_str()).unwrap_or_default() {
        "" => prefix + String::from_utf8_lossy(&text.as_bytes()[..text.len().min(200)]).trim(),
        m if retry.is_some() && status > 0 => format!("HTTP {status}: {m}"),
        m => m.to_string(),
    } + fix;
    ProviderError { kind, message, retry_after: retry_after.filter(|_| retry.is_some()), retry }
}

/// The wait before attempt `attempt + 1`: Retry-After capped at 60 s, else min(2^(attempt-1), 30) s.
pub fn retry_wait(attempt: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map_or(Duration::from_secs((1u64 << attempt.saturating_sub(1).min(5)).min(30)), |d| d.min(Duration::from_secs(60)))
}

pub fn perr(kind: &'static str, message: impl Into<String>) -> ProviderError {
    ProviderError { kind, message: message.into(), retry: None, retry_after: None }
}

/// `localhost`, a dotted quad in 127.0.0.0/8 (no trailing dot, no other IPv4 forms), or `::1`.
pub fn loopback_literal(host: &str) -> bool {
    matches!(host, "localhost" | "::1" | "[::1]") || host.parse::<Ipv4Addr>().is_ok_and(|a| a.is_loopback())
}

#[cfg(test)]
mod tests;
