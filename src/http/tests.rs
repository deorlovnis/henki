#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use serde_json::json;
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Short limits, so a timeout test takes a fraction of a second.
const LIM: Limits = Limits {
    connect: Duration::from_secs(1),
    first_byte: Duration::from_millis(300),
    stall: Duration::from_millis(300),
    idle_done: Duration::from_millis(150),
    request: Duration::from_secs(5),
};
const SSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";

/// What the fake model server saw: the request bytes, and whether the client closed the socket while it was held open.
struct Seen {
    req: Vec<u8>,
    closed: bool,
}

/// A one-connection model server: reads the whole request, writes each part after its pause in ms, then closes; with
/// `hold` it keeps the socket open until the client closes it (at most 10 s).
fn fake(parts: Vec<(u64, Vec<u8>)>, hold: bool) -> (Url, Receiver<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url { host: "127.0.0.1".into(), port: l.local_addr().unwrap().port(), path: "/v1".into() };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let req = read_request(&mut s);
        for (ms, b) in parts {
            thread::sleep(Duration::from_millis(ms));
            if s.write_all(&b).is_err() {
                break;
            }
        }
        let closed = hold && {
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            matches!(s.read(&mut [0; 16]), Ok(0))
        };
        let _ = tx.send(Seen { req, closed });
    });
    (url, rx)
}

/// One whole reply written at once, then the close.
fn reply(bytes: &str) -> Url {
    fake(vec![(0, bytes.as_bytes().to_vec())], false).0
}

fn read_request(s: &mut TcpStream) -> Vec<u8> {
    let (mut req, mut b) = (Vec::new(), [0; 4096]);
    loop {
        if let Some(i) = req.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&req[..i]).to_ascii_lowercase();
            let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |v| v.trim().parse().unwrap());
            if req.len() >= i + 4 + len {
                return req;
            }
        }
        match s.read(&mut b) {
            Ok(0) | Err(_) => return req,
            Ok(n) => req.extend_from_slice(&b[..n]),
        }
    }
}

fn slot() -> Slot {
    Arc::new(Mutex::new((false, None)))
}

fn go(url: &Url, lim: &Limits) -> Result<Resp, ProviderError> {
    post(url, "/chat/completions", &[], b"{}", false, lim, &slot())
}

fn events(url: &Url) -> Result<Vec<(String, String)>, ProviderError> {
    let mut r = go(url, &LIM)?;
    assert!(r.sse);
    let mut out = Vec::new();
    while let Some(e) = r.event(false, &LIM)? {
        out.push(e);
    }
    Ok(out)
}

fn ev(event: &str, data: &str) -> (String, String) {
    (event.to_string(), data.to_string())
}

fn err(url: &Url) -> ProviderError {
    go(url, &LIM).err().expect("an error")
}

fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn url9() -> Url {
    Url { host: "127.0.0.1".into(), port: 9, path: "/v1".into() }
}

#[test]
fn http_request_headers() {
    let (url, rx) = fake(vec![(0, format!("{SSE}data: [DONE]\n\n").into_bytes())], false);
    let mut r = post(&url, "/chat/completions", &[("Authorization", "Bearer k".into())], b"{\"a\":1}", true, &LIM, &slot()).unwrap();
    assert_eq!(r.event(false, &LIM).unwrap(), Some(ev("", "[DONE]")));
    let want = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUser-Agent: henki/{}\r\nContent-Type: application/json\r\nContent-Length: 7\r\nAccept: text/event-stream\r\nConnection: close\r\nAuthorization: Bearer k\r\n\r\n{{\"a\":1}}",
        url.port,
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(String::from_utf8(rx.recv().unwrap().req).unwrap(), want);
    assert_eq!(host_port(&Url { host: "::1".into(), port: 8080, path: String::new() }), "[::1]:8080");
}

#[test]
fn http_no_auth_without_key() {
    let (url, rx) = fake(vec![(0, format!("{SSE}data: x\n\n").into_bytes())], false);
    go(&url, &LIM).unwrap();
    let req = String::from_utf8(rx.recv().unwrap().req).unwrap().to_ascii_lowercase();
    assert!(!req.contains("authorization") && !req.contains("accept-encoding") && !req.contains("proxy"), "{req}");
}

#[test]
fn http_every_address() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead: SocketAddr = format!("127.0.0.1:{}", closed_port()).parse().unwrap();
    let s = connect(&[dead, l.local_addr().unwrap()], LIM.connect).unwrap();
    assert_eq!(s.peer_addr().unwrap(), l.local_addr().unwrap());
    assert!(connect(&[dead], LIM.connect).is_none());
}

#[test]
fn http_loopback_only_refuses() {
    let url = Url { host: "0.0.0.0".into(), port: closed_port(), path: String::new() };
    let e = post(&url, "/x", &[], b"", true, &LIM, &slot()).err().unwrap();
    assert_eq!((e.kind, e.retry), ("config", None));
    // without the rule the same URL is tried (and refused by the OS)
    assert!(post(&url, "/x", &[], b"", false, &LIM, &slot()).err().unwrap().retry.is_some());
}

#[test]
fn http_connect_failure_retryable() {
    let port = closed_port();
    let url = Url { host: "127.0.0.1".into(), port, path: "/v1".into() };
    let e = post(&url, "/chat/completions", &[], b"{}", true, &LIMITS, &slot()).err().unwrap();
    assert_eq!(e.message, format!("could not connect to 127.0.0.1:{port} (1 address, 5 s each)"));
    assert_eq!(e.retry.as_deref(), Some(format!("could not connect to 127.0.0.1:{port}").as_str()));
    assert_eq!(e.kind, "provider");
}

#[test]
fn http_reset_before_status_retryable() {
    let e = err(&reply(""));
    assert_eq!((e.kind, e.retry.as_deref()), ("provider", Some("connection reset")));
    let e = err(&reply("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n"));
    assert_eq!((e.message.as_str(), e.retry), ("the connection closed inside the response head", None));
    let e = err(&reply("SSH-2.0-OpenSSH\r\n\r\n"));
    assert_eq!(e.retry, None);
    assert!(e.message.starts_with("not an HTTP response from 127.0.0.1:"), "{}", e.message);
}

#[test]
fn loopback_literal_forms() {
    for h in ["localhost", "127.0.0.1", "127.1.2.3", "127.255.255.255", "::1", "[::1]"] {
        assert!(loopback_literal(h), "{h}");
    }
    for h in [
        "127.0.0.1.",
        "localhost.",
        "127.1",
        "0x7f.0.0.1",
        "0177.0.0.1",
        "127.000.000.001",
        "2130706433",
        "128.0.0.1",
        "0.0.0.0",
        "::ffff:127.0.0.1",
        "0:0:0:0:0:0:0:1",
        "LOCALHOST",
        "",
        "example.com",
        "127.0.0.256",
    ] {
        assert!(!loopback_literal(h), "{h}");
    }
}

#[test]
fn http_response_chunked() {
    let body = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n3;ext=1\r\n{\"a\r\nA\r\n\":[1,2,3]}\r\n0\r\n\r\n";
    let mut r = go(&reply(body), &LIM).unwrap();
    assert!(!r.sse);
    assert_eq!(r.body(JSON_MAX), br#"{"a":[1,2,3]}"#);
    let sse = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\n\r\n4\r\ndata\r\n7\r\n: a\n\nda\r\n7\r\nta: b\n\n\r\n0\r\n\r\n";
    assert_eq!(events(&reply(sse)).unwrap(), vec![ev("", "a"), ev("", "b")]);
}

#[test]
fn http_response_content_length() {
    // The server keeps the socket open: the body ends by its length, not by the close.
    let (url, _rx) = fake(vec![(0, b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\n\r\n{}{}more".to_vec())], true);
    let t = Instant::now();
    assert_eq!(go(&url, &LIM).unwrap().body(JSON_MAX), b"{}{}");
    assert!(t.elapsed() < LIM.stall, "{:?}", t.elapsed());
    let (url, _rx) = fake(vec![(0, b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 9\r\n\r\ndata: a\n\ndata: b\n\n".to_vec())], true);
    assert_eq!(events(&url).unwrap(), vec![ev("", "a")]);
}

#[test]
fn http_response_to_close() {
    let url = reply("HTTP/1.0 200 OK\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{\"x\": 1}\n");
    let mut r = go(&url, &LIM).unwrap();
    assert_eq!(r.body(JSON_MAX), b"{\"x\": 1}\n");
    assert_eq!(r.body(JSON_MAX), b"");
    let url = reply("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n0123456789");
    assert_eq!(go(&url, &LIM).unwrap().body(4), b"0123");
}

#[test]
fn http_head_over_64k() {
    let line = format!("X-Pad: {}\r\n", "p".repeat(1000));
    let head = |n: usize| format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{}\r\ndata: a\n\n", line.repeat(n));
    let e = err(&reply(&head(70)));
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", "a response head over 64 KiB", None));
    let e = err(&reply(&format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "y".repeat(HEAD_MAX))));
    assert_eq!(e.message, "a response head over 64 KiB");
    assert_eq!(events(&reply(&head(60))).unwrap(), vec![ev("", "a")]);
}

#[test]
fn http_chunk_line_over_1k() {
    let ok = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n9;{}\r\ndata: a\n\n\r\n0\r\n\r\n", "e".repeat(1000));
    assert_eq!(events(&reply(&ok)).unwrap(), vec![ev("", "a")]);
    let long = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n9;{}\r\ndata: a\n\n\r\n0\r\n\r\n", "e".repeat(1100));
    assert_eq!(events(&reply(&long)).err().unwrap().message, "a chunk size line over 1 KiB");
    let bad = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\ndata: a\n\n\r\n0\r\n\r\n";
    assert_eq!(events(&reply(bad)).err().unwrap().message, "a bad chunk size line");
}

#[test]
fn http_encoding_refused() {
    let e = err(&reply("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: gzip\r\n\r\ndata: a\n\n"));
    assert_eq!((e.kind, e.retry), ("provider", None));
    assert!(e.message.contains("Content-Encoding gzip"), "{}", e.message);
    let url = reply("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: identity\r\n\r\ndata: a\n\n");
    assert_eq!(events(&url).unwrap(), vec![ev("", "a")]);
}

#[test]
fn http_3xx_error() {
    let (url, rx) = fake(vec![(0, b"HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:1/v1/chat/completions\r\nContent-Length: 5\r\n\r\nmoved".to_vec())], false);
    let e = err(&url);
    assert_eq!((e.kind, e.retry), ("provider", None));
    assert_eq!(e.message, format!("HTTP 301 from http://127.0.0.1:{}/v1: moved", url.port));
    assert!(rx.recv().is_ok()); // one request, nothing followed
}

#[test]
fn http_2xx_other_type_error() {
    let e = err(&reply("HTTP/1.1 204 No Content\r\n\r\n"));
    assert_eq!((e.kind, e.retry), ("provider", None));
    assert!(e.message.starts_with("HTTP 204 from http://127.0.0.1:"), "{}", e.message);
    let url = reply("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>");
    let e = err(&url);
    assert_eq!(e.message, format!("HTTP 200 from http://127.0.0.1:{}/v1/chat/completions with Content-Type \"text/html\"", url.port));
    assert_eq!(err(&reply("HTTP/1.1 200 OK\r\n\r\n{}")).kind, "provider");
}

#[test]
fn http_json_200() {
    let url = reply("HTTP/1.1 200 OK\r\nCONTENT-TYPE: Application/JSON\r\ncontent-length: 11\r\n\r\n{\"ok\":true}");
    let mut r = go(&url, &LIM).unwrap();
    assert!(!r.sse);
    assert_eq!(serde_json::from_slice::<Value>(&r.body(JSON_MAX)).unwrap(), json!({"ok": true}));
}

#[test]
fn sse_crlf() {
    let url = reply(&format!("{SSE}data: a\r\n\r\ndata: b\r\r\n\r\ndata:  c\r\ndata: d"));
    assert_eq!(events(&url).unwrap(), vec![ev("", "a"), ev("", "b\r"), ev("", " c\nd")]);
}

#[test]
fn sse_comment_counts_as_activity() {
    // Each comment comes within the stall limit, the data after more than it in total.
    let mut parts = vec![(0, format!("{SSE}data: a\n\n").into_bytes())];
    parts.extend((0..6).map(|_| (100, b": keep-alive\n".to_vec())));
    parts.push((100, b"data: b\n\n".to_vec()));
    let (url, _rx) = fake(parts, false);
    assert_eq!(events(&url).unwrap(), vec![ev("", "a"), ev("", "b")]);
}

#[test]
fn sse_multi_data_joined() {
    let url = reply(&format!("{SSE}data: a\ndata:b\ndata\ndata: \nid: 7\nretry: 1\n\n\n\ndata: [DONE]\n\n"));
    assert_eq!(events(&url).unwrap(), vec![ev("", "a\nb\n\n"), ev("", "[DONE]")]);
}

#[test]
fn sse_event_field_kept() {
    let url = reply(&format!("{SSE}event: message_start\ndata: {{}}\n\nevent: ping\n\ndata: x\n\nevent:message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"));
    let want = vec![ev("message_start", "{}"), ev("", "x"), ev("message_stop", r#"{"type":"message_stop"}"#)];
    assert_eq!(events(&url).unwrap(), want);
}

#[test]
fn sse_line_over_1mib() {
    let exact = format!("data:{}", "x".repeat(SSE_LINE_MAX - 5));
    let url = reply(&format!("{SSE}{exact}\r\n\n"));
    assert_eq!(events(&url).unwrap()[0].1.len(), SSE_LINE_MAX - 5);
    let e = events(&reply(&format!("{SSE}{exact}x\n\n"))).err().unwrap();
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", "an event-stream line over 1 MiB", None));
    let e = events(&reply(&format!("{SSE}{exact}{}", "x".repeat(SSE_LINE_MAX)))).err().unwrap();
    assert_eq!(e.message, "an event-stream line over 1 MiB");
}

#[test]
fn http_timeout_first_byte() {
    let (url, _rx) = fake(vec![], true);
    let t = Instant::now();
    let e = err(&url);
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", "no first token within 0 s", None));
    assert!(t.elapsed() >= LIM.first_byte && t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // The head alone is not the first body byte: the same limit runs on.
    let (url, _rx) = fake(vec![(0, SSE.as_bytes().to_vec())], true);
    let mut r = go(&url, &LIM).unwrap();
    assert_eq!(r.event(false, &LIM).err().unwrap().message, "no first token within 0 s");
}

#[test]
fn http_timeout_stall() {
    let (url, _rx) = fake(vec![(0, format!("{SSE}data: a\n\n").into_bytes())], true);
    let mut r = go(&url, &LIM).unwrap();
    assert_eq!(r.event(false, &LIM).unwrap(), Some(ev("", "a")));
    let t = Instant::now();
    let e = r.event(false, &LIM).err().unwrap();
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", "no data from the model for 0 s", None));
    assert!(t.elapsed() >= LIM.stall && t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn timeout_messages_exact() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut k = Sock { s: TcpStream::connect(l.local_addr().unwrap()).unwrap(), first: Instant::now(), started: false, finished: false, lim: LIMITS };
    assert_eq!(io_err(&k, ErrorKind::TimedOut.into()).message, "no first token within 300 s");
    k.started = true;
    assert_eq!(io_err(&k, ErrorKind::TimedOut.into()), perr("provider", "no data from the model for 60 s"));
    assert_eq!(io_err(&k, io::Error::other("a bad chunk size line")), perr("provider", "a bad chunk size line"));
}

#[test]
fn http_idle_after_finish_completes() {
    // A long stall limit: only the idle limit can end the wait in time.
    let lim = Limits { stall: Duration::from_secs(20), ..LIM };
    let (url, _rx) = fake(vec![(0, format!("{SSE}data: a\n\ndata: b\n\n").into_bytes())], true);
    let mut r = go(&url, &lim).unwrap();
    assert_eq!(r.event(false, &lim).unwrap(), Some(ev("", "a")));
    assert_eq!(r.event(true, &lim).unwrap(), Some(ev("", "b")));
    let t = Instant::now();
    assert_eq!(r.event(true, &lim).unwrap(), None);
    assert!(t.elapsed() >= lim.idle_done && t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn http_cancel_shuts_socket() {
    let (url, rx) = fake(vec![(0, SSE.as_bytes().to_vec())], true);
    let (s, long) = (slot(), Limits { first_byte: Duration::from_secs(20), stall: Duration::from_secs(20), ..LIM });
    let mut r = post(&url, "/chat/completions", &[], b"{}", true, &long, &s).unwrap();
    let (tx, done) = mpsc::channel();
    thread::spawn(move || tx.send(r.event(false, &long)).unwrap());
    let t = Instant::now();
    cancel(&s);
    assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(None));
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().closed);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(s.lock().unwrap().0);
}

#[test]
fn http_cancel_before_connect() {
    let (url, rx) = fake(vec![], true);
    let s = slot();
    cancel(&s);
    let e = post(&url, "/chat/completions", &[], b"{}", true, &LIM, &s).err().unwrap();
    assert_eq!((e.kind, e.message.as_str(), e.retry), ("provider", "cancelled", None));
    let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(seen.req.is_empty() && seen.closed);
    cancel(&s); // twice is harmless
}

#[test]
fn retry_after_parse() {
    assert_eq!(retry_after("2"), Some(Duration::from_secs(2)));
    assert_eq!(retry_after("1.5"), Some(Duration::from_millis(1500)));
    assert_eq!(retry_after("0"), Some(Duration::ZERO));
    for v in ["Wed, 21 Oct 2015 07:28:00 GMT", "-1", "NaN", "inf", "1e999", "", "soon"] {
        assert_eq!(retry_after(v), None, "{v}");
    }
    assert_eq!(retry_wait(1, Some(Duration::from_secs(120))), Duration::from_secs(60));
    assert_eq!(retry_wait(3, Some(Duration::from_millis(1500))), Duration::from_millis(1500));
    // read from a real 429 head
    let e = err(&reply("HTTP/1.1 429 Too Many\r\nRetry-After: 1.8\r\nContent-Length: 0\r\n\r\n"));
    assert_eq!((e.retry.as_deref(), e.retry_after), (Some("HTTP 429"), Some(Duration::from_millis(1800))));
    let e = err(&reply("HTTP/1.1 503 Busy\r\nRetry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\nContent-Length: 0\r\n\r\n"));
    assert_eq!((e.retry.as_deref(), e.retry_after), (Some("HTTP 503"), None));
}

#[test]
fn retry_wait_backoff() {
    let got: Vec<u64> = (1..=8).map(|a| retry_wait(a, None).as_secs()).collect();
    assert_eq!(got, [1, 2, 4, 8, 16, 30, 30, 30]);
    assert_eq!(retry_wait(0, None), Duration::from_secs(1));
    assert_eq!(retry_wait(u32::MAX, None), Duration::from_secs(30));
}

#[test]
fn error_map_row_1() {
    let e = classify(502, b"halti: no route to the model\r\nmore\n", &url9(), Some(Duration::from_secs(1)));
    assert_eq!(e, perr("gateway", "halti: no route to the model"));
    assert_eq!(classify(200, b"halti: x", &url9(), None).kind, "gateway");
    assert_eq!(classify(429, b" halti: x", &url9(), None).kind, "provider");
}

#[test]
fn error_map_row_2() {
    let llama = br#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error"}}"#;
    let e = classify(400, llama, &url9(), None);
    assert_eq!(e, perr("context_exceeded", "the request exceeds the available context size, try increasing it"));
    let anthropic = br#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 200001 tokens > 200000 maximum"}}"#;
    assert_eq!(classify(400, anthropic, &url9(), None).kind, "context_exceeded");
    assert_eq!(classify(413, b"Too many tokens in the request", &url9(), None).kind, "context_exceeded");
    assert_eq!(classify(400, br#"{"error":{"message":"maximum Context length is 8192"}}"#, &url9(), None).kind, "context_exceeded");
    assert_eq!(classify(500, b"context too long", &url9(), None).kind, "provider");
    assert_eq!(classify(400, br#"{"error":{"message":"bad tools"}}"#, &url9(), None), perr("provider", "bad tools"));
}

#[test]
fn error_map_row_3() {
    assert_eq!(classify(401, br#"{"error":{"message":"invalid key"}}"#, &url9(), None), perr("auth", "invalid key"));
    assert_eq!(classify(403, b"", &url9(), None), perr("auth", "HTTP 403 from http://127.0.0.1:9/v1: "));
    let blocked = br#"{"type":"error","error":{"type":"permission_error","message":"blocked:no-standin"}}"#;
    let e = classify(400, blocked, &url9(), None);
    assert_eq!(e, perr("auth", "blocked:no-standin; fix: set api_key to the gateway's stand-in"));
    let typed = br#"{"type":"error","error":{"type":"authentication_error","message":"x-api-key invalid"}}"#;
    assert_eq!(classify(429, typed, &url9(), Some(Duration::from_secs(3))), perr("auth", "x-api-key invalid"));
}

#[test]
fn error_map_row_4() {
    for s in [408, 409, 425, 429, 500, 502, 503, 504, 529] {
        let e = classify(s, br#"{"error":{"message":"busy"}}"#, &url9(), Some(Duration::from_secs(2)));
        let want = ProviderError { kind: "provider", message: format!("HTTP {s}: busy"), retry: Some(format!("HTTP {s}")), retry_after: Some(Duration::from_secs(2)) };
        assert_eq!(e, want);
    }
    let e = classify(503, b"<h1>Service Unavailable</h1>", &url9(), None);
    assert_eq!((e.message.as_str(), e.retry.as_deref()), ("HTTP 503 from http://127.0.0.1:9/v1: <h1>Service Unavailable</h1>", Some("HTTP 503")));
    let overloaded = br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let e = classify(418, overloaded, &url9(), None);
    assert_eq!((e.kind, e.message.as_str(), e.retry.as_deref()), ("provider", "HTTP 418: Overloaded", Some("overloaded_error")));
    for s in [400, 404, 405, 501, 505] {
        assert_eq!(classify(s, b"", &url9(), Some(Duration::from_secs(1))).retry, None, "{s}");
    }
}

#[test]
fn error_map_row_5() {
    // The timeout rows: the strings come from the limits (exact bytes with the shipped ones: timeout_messages_exact).
    let port = closed_port();
    let url = Url { host: "localhost".into(), port, path: "/v1".into() };
    let e = post(&url, "/chat/completions", &[], b"{}", false, &LIMITS, &slot()).err().unwrap();
    assert!(e.message.starts_with(&format!("could not connect to localhost:{port} (")), "{}", e.message);
    assert!(e.message.ends_with(", 5 s each)"), "{}", e.message);
    let (url, _rx) = fake(vec![(0, format!("{SSE}data: a\n\n").into_bytes())], true);
    let mut r = go(&url, &LIM).unwrap();
    r.event(false, &LIM).unwrap();
    assert_eq!(r.event(false, &LIM).err().unwrap().retry, None);
}

#[test]
fn error_map_row_6() {
    let e = error_object(&json!({"error": {"type": "overloaded_error", "message": "Overloaded"}}), false);
    assert_eq!(e, perr("provider", "Overloaded"));
    let e = error_object(&json!({"error": {"code": 500, "message": "slot unavailable", "type": "server_error"}}), true);
    assert_eq!(e, perr("provider", "slot unavailable"));
}

#[test]
fn error_map_row_7() {
    let e = classify(404, br#"{"error":{"message":"model 'm' not found","type":"not_found_error"}}"#, &url9(), None);
    assert_eq!(e, perr("provider", "model 'm' not found"));
    let e = classify(405, br#"{"type":"error","error":{"type":"invalid_request_error","message":"no such route"}}"#, &url9(), None);
    assert_eq!(e, perr("provider", "no such route"));
    let long = format!("<html>{}</html>", "n".repeat(500));
    let e = classify(404, long.as_bytes(), &url9(), Some(Duration::from_secs(1)));
    assert_eq!(e, perr("provider", format!("HTTP 404 from http://127.0.0.1:9/v1: {}", &long[..200])));
    let e = classify(404, "\u{20ac}".repeat(100).as_bytes(), &url9(), None);
    assert!(e.message.ends_with('\u{fffd}'), "a cut character becomes U+FFFD: {}", e.message);
    assert_eq!(classify(404, br#"{"error":"plain"}"#, &url9(), None).message, "plain");
}

#[test]
fn error_object_first_event_retryable() {
    for ty in ["rate_limit_error", "overloaded_error", "api_error"] {
        let e = error_object(&json!({"type": "error", "error": {"type": ty, "message": "m"}}), true);
        assert_eq!((e.kind, e.message.as_str(), e.retry.as_deref()), ("provider", "m", Some(ty)));
    }
    let e = error_object(&json!({"error": {"type": "authentication_error", "message": "no"}}), true);
    assert_eq!(e, perr("auth", "no"));
    let e = error_object(&json!({"error": {"code": 1}}), true);
    assert_eq!(e, perr("provider", r#"the model sent an error: {"error":{"code":1}}"#));
}

#[test]
fn error_object_later_event_fatal() {
    for ty in ["rate_limit_error", "overloaded_error", "api_error"] {
        let e = error_object(&json!({"error": {"type": ty, "message": "m"}}), false);
        assert_eq!(e, perr("provider", "m"));
    }
}

/// A random, mostly well-formed response: a status line, headers, an SSE-ish body (chunked or not), then a few
/// random byte changes.
fn random_response(next: &mut impl FnMut() -> u64) -> Vec<u8> {
    fn pick<'a>(n: u64, xs: &[&'a str]) -> &'a str {
        xs[(n % xs.len() as u64) as usize]
    }
    let status = ["HTTP/1.1 503 x\r\n", "HTTP/1.0 301\r\n", "HTTP/1.1 99999\r\n", "HTTP/1.1\r\n", "", "\n", "junk\r\n"];
    let mut v = if !next().is_multiple_of(4) { "HTTP/1.1 200 OK\r\n" } else { pick(next(), &status) }.to_string();
    v += pick(next(), &["Content-Type: text/event-stream\r\n", "Content-Type: text/event-stream\r\n", "Content-Type: application/json\r\n", "Content-Type: text/html\r\n", ""]);
    let headers = [
        "Content-Length: 7\r\n",
        "content-length: 99999999999999999999\r\n",
        "Retry-After: 1.5\r\n",
        "Retry-After: -1e999\r\n",
        "Content-Encoding: gzip\r\n",
        "X: \u{1F600}\r\n",
        ":\r\n",
        "Transfer-Encoding: chunked\r\n",
    ];
    for _ in 0..next() % 6 {
        v += pick(next(), &headers);
    }
    let chunked = next().is_multiple_of(2);
    if chunked {
        v += "Transfer-Encoding: chunked\r\n";
    }
    v += "\r\n";
    let lines = ["data: {\"a\":1}\n", "data:\n", "\n", "\r\n", ": ping\n", "event: e\r\n", "data: [DONE]\n\n", "data", "id: 1\n", "data: \u{e9}\n", "\r"];
    let body: String = (0..next() % 12).map(|_| pick(next(), &lines)).collect();
    let mut out = v.into_bytes();
    if chunked {
        let mut b = body.as_bytes();
        while !b.is_empty() {
            let n = (next() % 9 + 1).min(b.len() as u64) as usize;
            let size = match next() % 8 {
                0 => format!("{n:x};x=y"),
                1 => format!("{:x}", n + 3),
                2 => "ffffffffffffffff".into(),
                3 => "zz".into(),
                _ => format!("{n:X}"),
            };
            out.extend_from_slice(format!("{size}\r\n").as_bytes());
            out.extend_from_slice(&b[..n]);
            out.extend_from_slice(b"\r\n");
            b = &b[n..];
        }
        if next().is_multiple_of(2) {
            out.extend_from_slice(b"0\r\n\r\n");
        }
    } else {
        out.extend_from_slice(body.as_bytes());
    }
    for _ in 0..next() % 3 {
        if !out.is_empty() {
            let i = (next() % out.len() as u64) as usize;
            out[i] = next() as u8;
        }
    }
    if next().is_multiple_of(5) {
        out.truncate((next() % (out.len() as u64 + 1)) as usize);
    }
    out
}

#[test]
fn http_never_panics() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let cases: Vec<Vec<u8>> = (0..2000).map(|_| random_response(&mut next)).collect();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url { host: "127.0.0.1".into(), port: l.local_addr().unwrap().port(), path: "/v1".into() };
    let served = cases.clone();
    thread::spawn(move || {
        for c in served {
            let (mut s, _) = l.accept().unwrap();
            read_request(&mut s);
            let _ = s.write_all(&c);
        }
    });
    let lim = Limits { first_byte: Duration::from_secs(5), stall: Duration::from_secs(5), idle_done: Duration::from_millis(20), ..LIM };
    let mut kinds = [0usize; 3];
    for (i, c) in cases.iter().enumerate() {
        match post(&url, "/chat/completions", &[], b"{}", false, &lim, &slot()) {
            Ok(mut r) if r.sse => {
                kinds[0] += 1;
                while let Ok(Some(_)) = r.event(i % 3 == 0, &lim) {}
            }
            Ok(mut r) => {
                kinds[1] += 1;
                assert!(r.body(i % 50).len() <= i % 50);
            }
            Err(_) => kinds[2] += 1,
        }
        let _ = classify((next() % 700) as u16, c, &url, None);
        let v = serde_json::from_slice(c).unwrap_or_else(|_| json!({"error": {"message": String::from_utf8_lossy(c), "type": "api_error"}}));
        let _ = error_object(&v, i % 2 == 0);
        let _ = loopback_literal(&String::from_utf8_lossy(&c[..c.len().min(12)]));
    }
    assert!(kinds.iter().all(|&k| k > 100), "every path is exercised: {kinds:?}");
}
