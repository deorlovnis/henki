#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! A scripted model server for the end-to-end tests, speaking either OpenAI chat completions or Anthropic Messages.
//! One std `TcpListener`, one thread per connection; request N gets step N (an `Error` step covers `times`
//! requests; a request refused as the API would refuse it gets no step); every request is logged with its head, body,
//! the refusal and whether the client closed the socket before the reply was complete. The SSE is written here, by
//! hand, so henki's parser is never tested against itself.
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The wire format the mock speaks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Api {
    /// `POST .../chat/completions` (any path).
    OpenAi,
    /// `POST .../messages` (`/v1/messages`, `/model/v1/messages`), checked as the API checks it.
    Anthropic,
}

/// How a call is written: as a native `tool_calls` delta, or as text in the content.
#[derive(Clone, Debug)]
pub enum Form {
    /// The arguments in `fragments` pieces; `index` as sent (None: no `index` field); `id` as sent (may be empty).
    /// Anthropic: a `tool_use` block with a fresh id, its input in `fragments` `input_json_delta`s.
    Native { fragments: usize, index: Option<u32>, id: String },
    /// `<tool_call>{"name": .., "arguments": ..}</tool_call>`
    Hermes,
    /// `<tool_call><function=NAME><parameter=K>V</parameter></function></tool_call>`
    QwenXml,
    /// A ```json fence holding `{"name": .., "arguments": ..}`.
    Fenced,
}

/// One reply. Stall, NoDone and Stream are the ways a reply can go wrong in time.
#[derive(Clone, Debug)]
pub enum Step {
    /// Content deltas, one per word, then `stop` (Anthropic: a text block, `end_turn`).
    Say(String),
    /// Reasoning deltas only, then `stop`: a reply with an empty answer (Anthropic: a thinking block with a fresh
    /// signature).
    Reason(String),
    /// One call; `args` is the raw argument text (it may be broken JSON); `finish` "" sends no finish reason
    /// (Anthropic: `tool_calls`, `stop`, `length` become `tool_use`, `end_turn`, `max_tokens`; others go as they are).
    Call { form: Form, name: String, args: String, finish: String },
    /// A non-200 response with this body as it is, for `times` requests in a row.
    Error { status: u16, retry_after: Option<String>, body: String, times: usize },
    /// Silence for `for_ms` (or until the test opens the gate): before the response head, or after `after_words` words
    /// of the answer. Nothing at all is sent meanwhile (no comment, no `ping`).
    Stall { before_first_byte: bool, after_words: usize, for_ms: u64 },
    /// Some text and an opened `shell` call, then the end of the body: no finish reason, no end marker.
    NoDone,
    /// A non-streamed `application/json` completion with this body.
    Json(Value),
    /// These bytes as the whole response, then close.
    Raw(Vec<u8>),
    /// One streamed reply from several pieces (`Say`, `Reason`, `Call`; `Stall` is a pause there), then the finish
    /// (None: none), the usage, the end marker when `done`; then the socket stays open `hold_ms`.
    Stream { parts: Vec<Step>, finish: Option<String>, done: bool, hold_ms: u64 },
    /// A short text, then the stop reason `refusal` (OpenAI: the finish `content_filter`).
    Refuse,
    /// An `error` event of type `ty`: as the first event, or after the reply started with some text.
    ErrorEvent { ty: String, after_start: bool },
    /// These events (`event:` name, `data:` payload) as the whole stream.
    Events(Vec<(String, Value)>),
}

/// One request as the mock saw it.
#[derive(Clone, Debug)]
pub struct Logged {
    /// 0 for the first request.
    pub n: usize,
    pub at: Instant,
    /// `POST /v1/chat/completions HTTP/1.1`
    pub line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The body parsed; Null when it is not JSON.
    pub json: Value,
    pub bytes: usize,
    /// Why the mock refused it as the API would (the error message it answered with); None when it got a step.
    pub rejected: Option<String>,
    /// The client closed (or shut down) the socket before the reply was complete.
    pub closed: bool,
    /// When the reply's last byte was written.
    pub done: Option<Instant>,
}

impl Logged {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }

    pub fn messages(&self) -> Vec<Value> {
        self.json["messages"].as_array().cloned().unwrap_or_default()
    }

    /// The contents of the messages with this role, in order (OpenAI: string contents).
    pub fn contents(&self, role: &str) -> Vec<String> {
        self.messages().iter().filter(|m| m["role"] == role).map(|m| m["content"].as_str().unwrap_or("").to_string()).collect()
    }

    /// The system prompt in either format.
    pub fn system(&self) -> String {
        self.json["system"][0]["text"].as_str().map_or_else(|| self.contents("system").concat(), String::from)
    }
}

struct Shared {
    api: Api,
    plan: Vec<Step>,
    log: Mutex<Vec<Logged>>,
    standin: Mutex<Option<String>>,
    stop: AtomicBool,
    gate: AtomicBool,
}

pub struct Mock {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Mock {
    /// OpenAI, on 127.0.0.1, a free port.
    pub fn start(steps: Vec<Step>) -> Mock {
        Mock::start_api(Api::OpenAi, steps)
    }

    pub fn start_api(api: Api, steps: Vec<Step>) -> Mock {
        Mock::bind(api, "127.0.0.1:0".parse().unwrap(), steps).unwrap()
    }

    pub fn start_at(bind: SocketAddr, steps: Vec<Step>) -> std::io::Result<Mock> {
        Mock::bind(Api::OpenAi, bind, steps)
    }

    fn bind(api: Api, bind: SocketAddr, steps: Vec<Step>) -> std::io::Result<Mock> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let plan = steps.into_iter().flat_map(|s| if let Step::Error { times, .. } = s { vec![s; times] } else { vec![s] }).collect();
        let shared = Arc::new(Shared { api, plan, log: Mutex::new(Vec::new()), standin: Mutex::new(None), stop: AtomicBool::new(false), gate: AtomicBool::new(false) });
        let sh = shared.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                if sh.stop.load(Ordering::SeqCst) {
                    return;
                }
                let sh = sh.clone();
                if let Ok(s) = conn {
                    thread::spawn(move || serve(&sh, s));
                }
            }
        });
        Ok(Mock { addr, shared })
    }

    /// Like a box's model gateway: every request without `key` (`x-api-key`, or `Authorization: Bearer`) gets a 401.
    pub fn standin(self, key: &str) -> Mock {
        *self.shared.standin.lock().unwrap() = Some(key.into());
        self
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:PORT/v1` (OpenAI), `http://127.0.0.1:PORT` (Anthropic: henki appends `/v1/messages`).
    pub fn url(&self) -> String {
        match self.shared.api {
            Api::OpenAi => format!("http://{}/v1", self.addr),
            Api::Anthropic => format!("http://{}", self.addr),
        }
    }

    pub fn log(&self) -> Vec<Logged> {
        self.shared.log.lock().unwrap().clone()
    }

    /// Ends every stall now and later: what a test waits for has happened.
    pub fn open_gate(&self) {
        self.shared.gate.store(true, Ordering::SeqCst);
    }

    /// The refusals so far, as `request N: MESSAGE`.
    pub fn rejections(&self) -> Vec<String> {
        self.log().iter().filter_map(|l| Some(format!("request {}: {}", l.n + 1, l.rejected.as_ref()?))).collect()
    }

    /// Waits until `n` requests arrived; panics after `within`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux interrupt matrix uses it
    pub fn wait_requests(&self, n: usize, within: Duration) -> Vec<Logged> {
        let end = Instant::now() + within;
        loop {
            let log = self.log();
            if log.len() >= n {
                return log;
            }
            assert!(Instant::now() < end, "the mock saw {} requests, not {n}, within {within:?}", log.len());
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until request `n` is marked closed by the client; false after `within`.
    pub fn wait_closed(&self, n: usize, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if self.log().get(n).is_some_and(|l| l.closed) {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

static IDS: AtomicUsize = AtomicUsize::new(1);

/// A fresh call id, unique in this test process (`call_7`).
pub fn call_id() -> String {
    format!("call_{}", IDS.fetch_add(1, Ordering::SeqCst))
}

pub fn say(text: &str) -> Step {
    Step::Say(text.into())
}

pub fn reason(text: &str) -> Step {
    Step::Reason(text.into())
}

/// A native call with index 0, a fresh id, the arguments in 4 fragments, finish `tool_calls`.
pub fn call(name: &str, args: Value) -> Step {
    native(name, &args.to_string(), "tool_calls")
}

pub fn native(name: &str, args: &str, finish: &str) -> Step {
    Step::Call { form: Form::Native { fragments: 4, index: Some(0), id: call_id() }, name: name.into(), args: args.into(), finish: finish.into() }
}

pub fn shell(command: &str) -> Step {
    call("shell", json!({ "command": command }))
}

pub fn text_call(form: Form, name: &str, args: Value) -> Step {
    Step::Call { form, name: name.into(), args: args.to_string(), finish: "stop".into() }
}

pub fn error(status: u16, body: &str, times: usize) -> Step {
    Step::Error { status, retry_after: None, body: body.into(), times }
}

/// An Anthropic error body: `{"type":"error","error":{"type":TY,"message":MESSAGE}}`.
pub fn api_error(ty: &str, message: &str) -> String {
    json!({"type": "error", "error": {"type": ty, "message": message}}).to_string()
}

/// An SSE body from `tests/fixtures/NAME.sse`, sent with no length and no chunking (the body ends at close).
pub fn fixture(name: &str) -> Step {
    let path = format!("{}/tests/fixtures/{name}.sse", env!("CARGO_MANIFEST_DIR"));
    let body = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Step::Raw([b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n".as_slice(), &body].concat())
}

fn serve(sh: &Shared, mut s: TcpStream) {
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let Some((line, headers, body)) = read_request(&mut s) else { return };
    let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (rejected, tokens, ctx) = (refuse(sh, &line, &headers, &json), body.len() / 4, conversation(&json));
    let (n, step) = {
        let mut log = sh.log.lock().unwrap();
        let (n, served) = (log.len(), log.iter().filter(|l| l.rejected.is_none()).count());
        let why = rejected.as_ref().map(|r| r.2.clone());
        log.push(Logged { n, at: Instant::now(), line, headers, bytes: body.len(), body, json, rejected: why, closed: false, done: None });
        (n, served)
    };
    let missing = || Step::Error { status: 404, retry_after: None, body: api_error("not_found_error", &format!("mock: no step for request {}", n + 1)), times: 1 };
    let step = match rejected {
        Some((status, ty, why)) => Step::Error { status, retry_after: None, body: api_error(ty, &why), times: 1 },
        None => sh.plan.get(step).cloned().unwrap_or_else(missing),
    };
    let mut w = Writer { s, open: true, stop: &sh.stop, gate: &sh.gate };
    respond(&mut w, &step, tokens, sh.api, ctx);
    let mut log = sh.log.lock().unwrap();
    log[n].closed = !w.open;
    log[n].done = Some(Instant::now());
    drop(log);
    let _ = w.s.shutdown(Shutdown::Write);
}

/// The request line, the headers and the body.
type Request = (String, Vec<(String, String)>, Vec<u8>);

fn read_request(s: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut b = [0u8; 65536];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let k = s.read(&mut b).ok().filter(|&k| k > 0)?;
        buf.extend_from_slice(&b[..k]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let line = lines.next()?.to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < len {
        let k = s.read(&mut b).ok().filter(|&k| k > 0)?;
        body.extend_from_slice(&b[..k]);
    }
    Some((line, headers, body))
}

/// The replies the mocks of this test process sent, by what identifies one (a thinking signature, a tool_use id), and
/// each signature's thinking text and conversation: what a replayed assistant message is checked against, across
/// mocks (a resumed session talks to a new one).
struct Sent {
    replies: HashMap<String, Vec<Value>>,
    sigs: HashMap<String, (String, u64)>,
}

static SENT: Mutex<Option<Sent>> = Mutex::new(None);

/// What a signature is bound to: the request's `system` and `tools`.
fn conversation(req: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    (req["system"].to_string(), req["tools"].to_string()).hash(&mut h);
    h.finish()
}

fn sent<R>(f: impl FnOnce(&mut Sent) -> R) -> R {
    f(SENT.lock().unwrap().get_or_insert_with(|| Sent { replies: HashMap::new(), sigs: HashMap::new() }))
}

/// Why the real API (or a box's gateway, for the key) would refuse this request: (status, error type, message).
fn refuse(sh: &Shared, line: &str, headers: &[(String, String)], req: &Value) -> Option<(u16, &'static str, String)> {
    let h = |k: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
    if let Some(key) = sh.standin.lock().unwrap().clone()
        && h("x-api-key") != Some(&key)
        && h("authorization").and_then(|a| a.strip_prefix("Bearer ")) != Some(&key)
    {
        return Some((401, "authentication_error", format!("blocked:no-standin: send {key} as the key; the gateway adds the real one")));
    }
    if sh.api == Api::OpenAi {
        return None;
    }
    if !line.split(' ').nth(1).is_some_and(|p| p.ends_with("/messages")) {
        return Some((404, "not_found_error", format!("no such endpoint: {line}")));
    }
    anthropic_refusal(req, h("anthropic-version").is_some()).err().map(|why| (400, "invalid_request_error", why))
}

fn anthropic_refusal(req: &Value, version: bool) -> Result<(), String> {
    if !version {
        return Err("anthropic-version: header is required".into());
    }
    if !req["max_tokens"].is_u64() {
        return Err("max_tokens: Field required".into());
    }
    let known = ["cache_control", "max_tokens", "messages", "metadata", "model", "stop_sequences", "stream", "system", "tool_choice", "tools"];
    if let Some(k) = req.as_object().into_iter().flatten().map(|(k, _)| k).find(|k| !known.contains(&k.as_str())) {
        return Err(format!("{k}: Extra inputs are not permitted"));
    }
    let (ctx, mut pending, mut role) = (conversation(req), Vec::<String>::new(), "");
    for (i, m) in req["messages"].as_array().ok_or("messages: Field required")?.iter().enumerate() {
        let blocks = match &m["content"] {
            Value::String(s) => vec![json!({"text": s, "type": "text"})],
            c => c.as_array().cloned().filter(|b| !b.is_empty()).ok_or(format!("messages.{i}.content: a non-empty list of content blocks is required"))?,
        };
        let r = m["role"].as_str().filter(|r| ["user", "assistant"].contains(r)).ok_or(format!("messages.{i}.role: Input should be 'user' or 'assistant'"))?;
        if r == role {
            return Err(format!("messages.{i}: roles must alternate between \"user\" and \"assistant\""));
        }
        if let Some(j) = blocks.iter().position(|b| b["type"] == "text" && b["text"].as_str().is_none_or(str::is_empty)) {
            return Err(format!("messages.{i}.content.{j}.text: text content blocks must be non-empty"));
        }
        let ids = |ty: &str, k: &str| blocks.iter().filter(|b| b["type"] == ty).map(|b| b[k].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
        let results = ids("tool_result", "tool_use_id");
        let missing: Vec<&String> = pending.iter().filter(|p| !results.contains(p)).collect();
        if !missing.is_empty() {
            return Err(format!("messages.{i}: `tool_use` ids were found without `tool_result` blocks immediately after: {missing:?}"));
        }
        if let Some(x) = results.iter().find(|x| !pending.contains(x)) {
            return Err(format!("messages.{i}: unexpected `tool_use_id` found in `tool_result` blocks: {x}"));
        }
        if r == "assistant" {
            replayed(i, &blocks, ctx)?;
        }
        (pending, role) = (if r == "assistant" { ids("tool_use", "id") } else { Vec::new() }, r);
    }
    if !pending.is_empty() {
        return Err(format!("messages: `tool_use` ids were found without `tool_result` blocks immediately after: {pending:?}"));
    }
    Ok(())
}

/// An assistant message that replays a reply a mock sent must be that reply's content exactly; each thinking block's
/// signature must be one a mock issued, for this thinking text, under this request's system prompt and tools.
fn replayed(i: usize, blocks: &[Value], ctx: u64) -> Result<(), String> {
    sent(|s| {
        if let Some(orig) = blocks.iter().filter_map(key).find_map(|k| s.replies.get(&k))
            && orig[..] != blocks[..]
        {
            return Err(format!("messages.{i}.content: the assistant content was changed from the reply it replays"));
        }
        for (j, b) in blocks.iter().enumerate().filter(|(_, b)| b["type"] == "thinking") {
            let issued = b["signature"].as_str().and_then(|sig| s.sigs.get(sig)).filter(|(t, _)| b["thinking"] == t.as_str());
            match issued {
                None => return Err(format!("messages.{i}.content.{j}: Invalid signature in thinking block")),
                Some((_, c)) if *c != ctx => {
                    return Err(format!(
                        "messages.{i}.content.{j}: Invalid signature in thinking block: it is bound to a different conversation (another system prompt or tool set)"
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    })
}

/// What identifies a reply's block: a thinking signature, a tool_use id.
fn key(b: &Value) -> Option<String> {
    b["signature"].as_str().or(b["id"].as_str()).filter(|k| !k.is_empty()).map(String::from)
}

struct Writer<'a> {
    s: TcpStream,
    open: bool,
    stop: &'a AtomicBool,
    gate: &'a AtomicBool,
}

impl Writer<'_> {
    fn put(&mut self, b: &[u8]) {
        if self.open && self.s.write_all(b).and_then(|()| self.s.flush()).is_err() {
            self.open = false;
        }
    }

    /// One SSE event as one HTTP chunk; `name` adds an `event:` line.
    fn event(&mut self, name: Option<&str>, data: &str) {
        let ev = format!("{}data: {data}\n\n", name.map(|n| format!("event: {n}\n")).unwrap_or_default());
        self.put(format!("{:x}\r\n{ev}\r\n", ev.len()).as_bytes());
    }

    fn head(&mut self) {
        self.put(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
    }

    /// The end of the body, or the socket held open for `hold_ms`.
    fn end(&mut self, hold_ms: u64) {
        if hold_ms > 0 {
            self.pause(hold_ms);
        } else {
            self.put(b"0\r\n\r\n");
        }
    }

    /// Silence for `ms` or until the gate opens, watching for the client to close: a read gives EOF (or an error) only
    /// then.
    fn pause(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        let _ = self.s.set_read_timeout(Some(Duration::from_millis(10)));
        let mut b = [0u8; 256];
        while self.open && Instant::now() < end && !self.stop.load(Ordering::SeqCst) && !self.gate.load(Ordering::SeqCst) {
            match self.s.read(&mut b) {
                Ok(0) => self.open = false,
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
                Err(_) => self.open = false,
            }
        }
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        529 => "Overloaded",
        _ => "Status",
    }
}

fn respond(w: &mut Writer, step: &Step, tokens: usize, api: Api, ctx: u64) {
    let (parts, finish, done, hold) = match step {
        Step::Error { status, retry_after, body, .. } => {
            let ra = retry_after.as_ref().map(|r| format!("Retry-After: {r}\r\n")).unwrap_or_default();
            let head = format!("HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{ra}Connection: close\r\n\r\n", reason_phrase(*status), body.len());
            w.put(head.as_bytes());
            return w.put(body.as_bytes());
        }
        Step::Json(v) => {
            let body = v.to_string();
            return w.put(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
        Step::Raw(b) => return w.put(b),
        Step::Events(evs) => {
            w.head();
            let mut a = Anthropic { w, ctx, blocks: Vec::new(), open: false, head: true };
            evs.iter().for_each(|(name, data)| a.ev(name, data.clone()));
            return a.w.end(0);
        }
        Step::ErrorEvent { ty, after_start } => {
            let err: Value = serde_json::from_str(&api_error(ty, "Overloaded")).unwrap();
            let mut a = Anthropic { w, ctx, blocks: Vec::new(), open: false, head: false };
            match (api, after_start) {
                (Api::Anthropic, true) => drop(a.parts(&[say("partial")], tokens)),
                (Api::OpenAi, true) => {
                    a.w.head();
                    a.w.event(None, &chunk(json!({"content": "partial"}), None));
                }
                (_, false) => a.w.head(),
            }
            if api == Api::OpenAi {
                a.w.event(None, &err.to_string())
            } else {
                a.ev("error", err)
            }
            return a.w.end(0);
        }
        Step::Refuse if api == Api::OpenAi => (vec![say("I can't help with that.")], Some("content_filter"), true, 0),
        Step::Refuse => (vec![say("I can't help with that.")], Some("refusal"), true, 0),
        Step::Stream { parts, finish, done, hold_ms } => (parts.clone(), finish.as_deref(), *done, *hold_ms),
        Step::Say(_) | Step::Reason(_) => (vec![step.clone()], Some("stop"), true, 0),
        Step::Call { finish, .. } => (vec![step.clone()], Some(finish.as_str()).filter(|f| !f.is_empty()), true, 0),
        Step::Stall { before_first_byte: true, for_ms, .. } => {
            (vec![Step::Stall { before_first_byte: true, after_words: 0, for_ms: *for_ms }, say("late answer")], Some("stop"), true, 0)
        }
        Step::Stall { after_words, for_ms, .. } => {
            let words = (1..=*after_words).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
            (vec![Step::Say(words), Step::Stall { before_first_byte: false, after_words: 0, for_ms: *for_ms }, say(" end")], Some("stop"), true, 0)
        }
        Step::NoDone => {
            let open = Step::Call {
                form: Form::Native { fragments: 2, index: Some(0), id: "call_nodone".into() },
                name: "shell".into(),
                args: r#"{"command": "touch nodone-ran"#.into(),
                finish: String::new(),
            };
            (vec![say("checking"), open], None, false, 0)
        }
    };
    match api {
        Api::OpenAi => stream(w, &parts, finish, done, hold, tokens),
        Api::Anthropic => {
            let mut a = Anthropic { w, ctx, blocks: Vec::new(), open: false, head: false };
            a.reply(&parts, finish, done, tokens);
            a.w.end(hold);
        }
    }
}

fn chunk(delta: Value, finish: Option<&str>) -> String {
    json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1_762_400_000, "model": "mock-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    .to_string()
}

/// "one two three" as "one", " two", " three"; "" as one empty piece.
pub fn words(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for c in text.chars() {
        if c == ' ' && !out.last().unwrap().trim().is_empty() {
            out.push(String::new());
        }
        out.last_mut().unwrap().push(c);
    }
    out
}

/// `text` in pieces of at most `n` bytes, cut on char boundaries.
fn pieces(text: &str, n: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    for c in text.chars() {
        if out.last().unwrap().len() + c.len_utf8() > n {
            out.push(String::new());
        }
        out.last_mut().unwrap().push(c);
    }
    out
}

/// The markup a text call is written as.
pub fn markup(form: &Form, name: &str, args: &str) -> String {
    match form {
        Form::Hermes => format!("<tool_call>\n{{\"name\": \"{name}\", \"arguments\": {args}}}\n</tool_call>"),
        Form::Fenced => format!("```json\n{{\"name\": \"{name}\", \"arguments\": {args}}}\n```"),
        Form::QwenXml => {
            let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
            let params: String =
                v.as_object().into_iter().flatten().map(|(k, v)| format!("<parameter={k}>\n{}\n</parameter>\n", v.as_str().map_or(v.to_string(), String::from))).collect();
            format!("<tool_call>\n<function={name}>\n{params}</function>\n</tool_call>")
        }
        Form::Native { .. } => String::new(),
    }
}

fn stream(w: &mut Writer, parts: &[Step], finish: Option<&str>, done: bool, hold_ms: u64, tokens: usize) {
    let mut head = false;
    let mut out = 0;
    for p in parts {
        if let Step::Stall { before_first_byte: true, for_ms, .. } = p
            && !head
        {
            w.pause(*for_ms);
            continue;
        }
        if !head {
            w.head();
            w.event(None, &chunk(json!({"role": "assistant", "content": null}), None));
            head = true;
        }
        match p {
            Step::Say(t) => words(t).iter().for_each(|x| w.event(None, &chunk(json!({ "content": x }), None))),
            Step::Reason(t) => words(t).iter().for_each(|x| w.event(None, &chunk(json!({ "reasoning_content": x }), None))),
            Step::Stall { for_ms, .. } => w.pause(*for_ms),
            Step::Call { form: Form::Native { fragments, index, id }, name, args, .. } => {
                let mut first = json!({"id": id, "type": "function", "function": {"name": name, "arguments": ""}});
                if let Some(i) = index {
                    first["index"] = json!(i);
                }
                w.event(None, &chunk(json!({ "tool_calls": [first] }), None));
                let size = args.len().div_ceil((*fragments).max(1)).max(1);
                for a in pieces(args, size) {
                    let mut d = json!({"function": {"arguments": a}});
                    if let Some(i) = index {
                        d["index"] = json!(i);
                    }
                    w.event(None, &chunk(json!({ "tool_calls": [d] }), None));
                }
            }
            Step::Call { form, name, args, .. } => pieces(&markup(form, name, args), 12).iter().for_each(|x| w.event(None, &chunk(json!({ "content": x }), None))),
            _ => {}
        }
        out += 1;
    }
    if let Some(f) = finish {
        w.event(None, &chunk(json!({}), Some(f)));
    }
    let usage = json!({"prompt_tokens": tokens, "completion_tokens": out * 3, "total_tokens": tokens + out * 3});
    w.event(None, &json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1_762_400_000, "model": "mock-model", "choices": [], "usage": usage}).to_string());
    if done {
        w.event(None, "[DONE]");
    }
    w.end(hold_ms);
}

/// An Anthropic Messages stream as the API writes it: `message_start`, `ping`, per content block
/// `content_block_start`, its deltas and `content_block_stop`, then `message_delta` and `message_stop`. It keeps the
/// content it sent, so a later request can be checked against it.
struct Anthropic<'a, 'b> {
    w: &'a mut Writer<'b>,
    /// The request's system prompt and tools, which signatures are bound to.
    ctx: u64,
    /// The content blocks so far, by index; a tool_use block's input as text until the reply is complete.
    blocks: Vec<(Value, String)>,
    /// The last block is still open.
    open: bool,
    head: bool,
}

impl Anthropic<'_, '_> {
    /// Writes an event and keeps what it adds to the content; before the reply's end the content is registered.
    fn ev(&mut self, name: &str, data: Value) {
        let (d, at) = (&data["delta"], data["index"].as_u64().unwrap_or(0) as usize);
        match (data["type"].as_str().unwrap_or(""), d["type"].as_str().unwrap_or("")) {
            ("content_block_start", _) => {
                self.blocks.resize(self.blocks.len().max(at + 1), (Value::Null, String::new()));
                self.blocks[at] = (data["content_block"].clone(), String::new());
            }
            ("content_block_delta", ty) if at < self.blocks.len() => {
                let (b, input) = &mut self.blocks[at];
                match ty.strip_suffix("_delta").unwrap_or("") {
                    "input_json" => input.push_str(d["partial_json"].as_str().unwrap_or("")),
                    k @ ("text" | "thinking" | "signature") => b[k] = format!("{}{}", b[k].as_str().unwrap_or(""), d[k].as_str().unwrap_or("")).into(),
                    _ => {}
                }
            }
            ("message_delta" | "message_stop", _) => self.register(),
            _ => {}
        }
        self.w.event(Some(name), &data.to_string());
    }

    /// The content as the API's message holds it: tool_use input parsed (`{}` unless an object).
    fn register(&self) {
        let content: Vec<Value> = self
            .blocks
            .iter()
            .filter(|(b, _)| !b.is_null())
            .map(|(b, input)| {
                let mut b = b.clone();
                if b["type"] == "tool_use" {
                    b["input"] = serde_json::from_str::<Value>(input).ok().filter(Value::is_object).unwrap_or_else(|| json!({}));
                }
                b
            })
            .collect();
        sent(|s| {
            for (b, k) in content.iter().filter_map(|b| Some((b, key(b)?))) {
                if b["type"] == "thinking" {
                    s.sigs.insert(k.clone(), (b["thinking"].as_str().unwrap_or("").into(), self.ctx));
                }
                s.replies.insert(k, content.clone());
            }
        });
    }

    /// The response head, `message_start` (usage with cache fields) and one `ping`, once.
    fn start(&mut self, tokens: usize) {
        if !std::mem::replace(&mut self.head, true) {
            self.w.head();
            let (read, created) = (tokens / 2, tokens / 8);
            let usage = json!({"input_tokens": tokens - read - created, "cache_creation_input_tokens": created, "cache_read_input_tokens": read, "output_tokens": 1});
            let n = IDS.fetch_add(1, Ordering::SeqCst);
            let msg = json!({"id": format!("msg_mock_{n}"), "type": "message", "role": "assistant", "content": [], "model": "mock-model", "stop_reason": null, "stop_sequence": null, "usage": usage});
            self.ev("message_start", json!({"type": "message_start", "message": msg}));
            self.ev("ping", json!({"type": "ping"}));
        }
    }

    /// The open block's index when it is a text block and `text` continues it; else a new block.
    fn block(&mut self, block: Value) -> usize {
        if self.open && block["type"] == "text" && self.blocks.last().is_some_and(|b| b.0["type"] == "text") {
            return self.blocks.len() - 1;
        }
        self.close();
        let i = self.blocks.len();
        self.ev("content_block_start", json!({"type": "content_block_start", "index": i, "content_block": block}));
        self.open = true;
        i
    }

    /// Ends the open block; a thinking block gets its signature first.
    fn close(&mut self) {
        if std::mem::take(&mut self.open) {
            let i = self.blocks.len() - 1;
            if self.blocks[i].0["type"] == "thinking" {
                let sig = format!("EqMockSig{:08}", IDS.fetch_add(1, Ordering::SeqCst));
                self.ev("content_block_delta", json!({"type": "content_block_delta", "index": i, "delta": {"type": "signature_delta", "signature": sig}}));
            }
            self.ev("content_block_stop", json!({"type": "content_block_stop", "index": i}));
        }
    }

    fn delta(&mut self, i: usize, delta: Value) {
        self.ev("content_block_delta", json!({"type": "content_block_delta", "index": i, "delta": delta}));
    }

    fn parts(&mut self, parts: &[Step], tokens: usize) -> usize {
        let mut out = 0;
        for p in parts {
            if let Step::Stall { before_first_byte: true, for_ms, .. } = p
                && !self.head
            {
                self.w.pause(*for_ms);
                continue;
            }
            self.start(tokens);
            match p {
                Step::Say(t) if !t.is_empty() => {
                    let i = self.block(json!({"type": "text", "text": ""}));
                    words(t).into_iter().for_each(|x| self.delta(i, json!({"type": "text_delta", "text": x})));
                }
                Step::Reason(t) => {
                    let i = self.block(json!({"type": "thinking", "thinking": "", "signature": ""}));
                    words(t).into_iter().for_each(|x| self.delta(i, json!({"type": "thinking_delta", "thinking": x})));
                }
                Step::Stall { for_ms, .. } => self.w.pause(*for_ms),
                Step::Call { form: Form::Native { fragments, .. }, name, args, .. } => {
                    let id = format!("toolu_mock_{}", IDS.fetch_add(1, Ordering::SeqCst));
                    let i = self.block(json!({"type": "tool_use", "id": id, "name": name, "input": {}}));
                    let size = args.len().div_ceil((*fragments).max(1)).max(1);
                    pieces(args, size).into_iter().filter(|a| !a.is_empty()).for_each(|a| self.delta(i, json!({"type": "input_json_delta", "partial_json": a})));
                }
                Step::Call { form, name, args, .. } => {
                    let i = self.block(json!({"type": "text", "text": ""}));
                    self.delta(i, json!({"type": "text_delta", "text": markup(form, name, args)}));
                }
                _ => {}
            }
            out += 1;
        }
        out
    }

    /// A whole reply: its blocks, then (with a finish) the stop reason and usage, then `message_stop` when `done`.
    fn reply(&mut self, parts: &[Step], finish: Option<&str>, done: bool, tokens: usize) {
        let out = self.parts(parts, tokens);
        self.start(tokens);
        if let Some(f) = finish {
            self.close();
            let stop = match f {
                "tool_calls" => "tool_use",
                "stop" => "end_turn",
                "length" => "max_tokens",
                f => f,
            };
            self.ev("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": out * 3}}));
        }
        if done {
            self.ev("message_stop", json!({"type": "message_stop"}));
        }
    }
}
