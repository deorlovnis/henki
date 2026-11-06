#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! A scripted OpenAI-compatible model server for the end-to-end tests. One std `TcpListener`, one thread per
//! connection; request N gets step N (an `Error` step covers `times` requests); every request is logged with its
//! head, body and whether the client closed the socket before the reply was complete. The SSE is written here, by
//! hand, so henki's parser is never tested against itself.
use serde_json::{Value, json};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How a call is written: as a native `tool_calls` delta, or as text in the content.
#[derive(Clone, Debug)]
pub enum Form {
    /// The arguments in `fragments` pieces; `index` as sent (None: no `index` field); `id` as sent (may be empty).
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
    /// Content deltas, one per word, then `stop`.
    Say(String),
    /// Reasoning deltas only, then `stop`: a reply with an empty answer.
    Reason(String),
    /// One call; `args` is the raw argument text (it may be broken JSON); `finish` "" sends no finish reason.
    Call { form: Form, name: String, args: String, finish: String },
    /// A non-200 response, for `times` requests in a row.
    Error { status: u16, retry_after: Option<String>, body: String, times: usize },
    /// Silence for `for_ms`: before the response head, or after `after_words` words of the answer.
    Stall { before_first_byte: bool, after_words: usize, for_ms: u64 },
    /// Some text and an opened `shell` call, then the end of the body: no finish reason, no `[DONE]`.
    NoDone,
    /// A non-streamed `application/json` completion with this body.
    Json(Value),
    /// These bytes as the whole response, then close.
    Raw(Vec<u8>),
    /// One streamed reply from several pieces (`Say`, `Reason`, `Call`; `Stall` is a pause there), then the finish
    /// chunk (None: none), the usage chunk, `[DONE]` when `done`; then the socket stays open `hold_ms`.
    Stream { parts: Vec<Step>, finish: Option<String>, done: bool, hold_ms: u64 },
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

    /// The contents of the messages with this role, in order.
    pub fn contents(&self, role: &str) -> Vec<String> {
        self.messages().iter().filter(|m| m["role"] == role).map(|m| m["content"].as_str().unwrap_or("").to_string()).collect()
    }
}

struct Shared {
    plan: Vec<Step>,
    log: Mutex<Vec<Logged>>,
    stop: AtomicBool,
}

pub struct Mock {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Mock {
    /// On 127.0.0.1, a free port.
    pub fn start(steps: Vec<Step>) -> Mock {
        Mock::start_at("127.0.0.1:0".parse().unwrap(), steps).unwrap()
    }

    pub fn start_at(bind: SocketAddr, steps: Vec<Step>) -> std::io::Result<Mock> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let plan = steps.into_iter().flat_map(|s| if let Step::Error { times, .. } = s { vec![s; times] } else { vec![s] }).collect();
        let shared = Arc::new(Shared { plan, log: Mutex::new(Vec::new()), stop: AtomicBool::new(false) });
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

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:PORT/v1`
    pub fn url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn log(&self) -> Vec<Logged> {
        self.shared.log.lock().unwrap().clone()
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
    let n = {
        let mut log = sh.log.lock().unwrap();
        let n = log.len();
        log.push(Logged { n, at: Instant::now(), line, headers, bytes: body.len(), body, json, closed: false, done: None });
        n
    };
    let tokens = sh.log.lock().unwrap()[n].bytes / 4;
    let missing;
    let step = match sh.plan.get(n) {
        Some(s) => s,
        None => {
            missing = Step::Error { status: 404, retry_after: None, body: json!({"error": {"message": format!("mock: no step for request {}", n + 1)}}).to_string(), times: 1 };
            &missing
        }
    };
    let mut w = Writer { s, open: true, stop: &sh.stop };
    respond(&mut w, step, tokens);
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

struct Writer<'a> {
    s: TcpStream,
    open: bool,
    stop: &'a AtomicBool,
}

impl Writer<'_> {
    fn put(&mut self, b: &[u8]) {
        if self.open && self.s.write_all(b).and_then(|()| self.s.flush()).is_err() {
            self.open = false;
        }
    }

    /// One SSE event as one HTTP chunk.
    fn event(&mut self, data: &str) {
        let ev = format!("data: {data}\n\n");
        self.put(format!("{:x}\r\n{ev}\r\n", ev.len()).as_bytes());
    }

    /// Silence for `ms`, watching for the client to close: a read gives EOF (or an error) only then.
    fn pause(&mut self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        let _ = self.s.set_read_timeout(Some(Duration::from_millis(10)));
        let mut b = [0u8; 256];
        while self.open && Instant::now() < end && !self.stop.load(Ordering::SeqCst) {
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
        _ => "Status",
    }
}

fn respond(w: &mut Writer, step: &Step, tokens: usize) {
    match step {
        Step::Error { status, retry_after, body, .. } => {
            let ra = retry_after.as_ref().map(|r| format!("Retry-After: {r}\r\n")).unwrap_or_default();
            let head = format!("HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{ra}Connection: close\r\n\r\n", reason_phrase(*status), body.len());
            w.put(head.as_bytes());
            w.put(body.as_bytes());
        }
        Step::Json(v) => {
            let body = v.to_string();
            w.put(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
        Step::Raw(b) => w.put(b),
        Step::Stream { parts, finish, done, hold_ms } => stream(w, parts, finish.as_deref(), *done, *hold_ms, tokens),
        Step::Say(_) | Step::Reason(_) => stream(w, std::slice::from_ref(step), Some("stop"), true, 0, tokens),
        Step::Call { finish, .. } => stream(w, std::slice::from_ref(step), Some(finish).filter(|f| !f.is_empty()).map(|f| f.as_str()), true, 0, tokens),
        Step::Stall { before_first_byte: true, for_ms, .. } => {
            stream(w, &[Step::Stall { before_first_byte: true, after_words: 0, for_ms: *for_ms }, say("late answer")], Some("stop"), true, 0, tokens)
        }
        Step::Stall { after_words, for_ms, .. } => {
            let words = (1..=*after_words).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
            let pause = Step::Stall { before_first_byte: false, after_words: 0, for_ms: *for_ms };
            stream(w, &[Step::Say(words), pause, say(" end")], Some("stop"), true, 0, tokens)
        }
        Step::NoDone => {
            let open = Step::Call {
                form: Form::Native { fragments: 2, index: Some(0), id: "call_nodone".into() },
                name: "shell".into(),
                args: r#"{"command": "touch nodone-ran"#.into(),
                finish: String::new(),
            };
            stream(w, &[say("checking"), open], None, false, 0, tokens)
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
            w.put(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
            w.event(&chunk(json!({"role": "assistant", "content": null}), None));
            head = true;
        }
        match p {
            Step::Say(t) => words(t).iter().for_each(|x| w.event(&chunk(json!({ "content": x }), None))),
            Step::Reason(t) => words(t).iter().for_each(|x| w.event(&chunk(json!({ "reasoning_content": x }), None))),
            Step::Stall { for_ms, .. } => w.pause(*for_ms),
            Step::Call { form: Form::Native { fragments, index, id }, name, args, .. } => {
                let mut first = json!({"id": id, "type": "function", "function": {"name": name, "arguments": ""}});
                if let Some(i) = index {
                    first["index"] = json!(i);
                }
                w.event(&chunk(json!({ "tool_calls": [first] }), None));
                let size = args.len().div_ceil((*fragments).max(1)).max(1);
                for a in pieces(args, size) {
                    let mut d = json!({"function": {"arguments": a}});
                    if let Some(i) = index {
                        d["index"] = json!(i);
                    }
                    w.event(&chunk(json!({ "tool_calls": [d] }), None));
                }
            }
            Step::Call { form, name, args, .. } => pieces(&markup(form, name, args), 12).iter().for_each(|x| w.event(&chunk(json!({ "content": x }), None))),
            _ => {}
        }
        out += 1;
    }
    if let Some(f) = finish {
        w.event(&chunk(json!({}), Some(f)));
    }
    let usage = json!({"prompt_tokens": tokens, "completion_tokens": out * 3, "total_tokens": tokens + out * 3});
    w.event(&json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1_762_400_000, "model": "mock-model", "choices": [], "usage": usage}).to_string());
    if done {
        w.event("[DONE]");
    }
    if hold_ms > 0 {
        w.pause(hold_ms);
    } else {
        w.put(b"0\r\n\r\n");
    }
}
