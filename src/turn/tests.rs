#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::call::Stuck;
use super::request::ctx_use;
use super::*;
use crate::config::Env;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

/// A fresh directory for one test, removed first if an earlier run left it.
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("henki-turn-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Everything written to a test's stdout.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `--json` session in `d` with no history, its stdout captured, and the model at `127.0.0.1:port`.
fn session(d: &Path, port: u16) -> (Session, Buf) {
    let mut cfg = Config::test(d);
    cfg.url.port = port;
    let state = journal::state_dir(&Env::default(), d).unwrap();
    let j = Journal::create(&state, d).unwrap();
    let (tx, rx) = mpsc::channel();
    let buf = Buf::default();
    let out = Out::with(true, tx.clone(), Box::new(buf.clone()), Box::new(std::io::sink()));
    (Session::new(cfg, j, Vec::new(), out, tx, rx, None), buf)
}

/// A model server on loopback: request N gets `replies[N]` (a whole HTTP response), then the connection closes.
fn serve(replies: Vec<String>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for (mut c, reply) in l.incoming().map(Result::unwrap).zip(replies) {
            let mut req = Vec::new();
            let mut b = [0; 65536];
            // The head, then Content-Length bytes of body.
            while !req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() < body_end(&req) {
                let n = c.read(&mut b).unwrap();
                assert!(n > 0, "the client closed the request early");
                req.extend_from_slice(&b[..n]);
            }
            c.write_all(reply.as_bytes()).unwrap();
        }
    });
    port
}

fn body_end(req: &[u8]) -> usize {
    let head = String::from_utf8_lossy(req);
    let start = head.find("\r\n\r\n").unwrap() + 4;
    let len = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()));
    start + len.unwrap_or(0)
}

/// A 200 SSE response of these chunks, then `[DONE]`.
fn sse(chunks: &[Value]) -> String {
    let events: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{events}data: [DONE]\n\n")
}

fn delta(d: Value, finish: Option<&str>) -> Value {
    serde_json::json!({"choices": [{"index": 0, "delta": d, "finish_reason": finish}]})
}

fn lines(buf: &Buf) -> Vec<Value> {
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn kinds(recs: &[Value]) -> Vec<&str> {
    recs.iter().map(|r| r["t"].as_str().unwrap()).collect()
}

fn key(name: &str, args: &str) -> String {
    format!("{name}\0{args}")
}

#[test]
fn stuck_spec_rule() {
    let mut s = Stuck::default();
    for run in 1..=3 {
        s.feed(key("read", r#"{"path":"x"}"#), true);
        assert_eq!(s.run, run);
    }
}

#[test]
fn stuck_success_resets() {
    let mut s = Stuck::default();
    s.feed(key("read", "{}"), true);
    s.feed(key("read", "{}"), true);
    s.feed(key("read", "{}"), false);
    assert_eq!(s.run, 0);
    s.feed(key("read", "{}"), true);
    assert_eq!(s.run, 1, "a failure after a success starts a new run");
}

#[test]
fn stuck_identical_successes_never_count() {
    let mut s = Stuck::default();
    for _ in 0..5 {
        s.feed(key("shell", r#"{"command":"cat status"}"#), false);
        assert_eq!(s.run, 0);
    }
}

#[test]
fn stuck_reset_by_other_call() {
    let mut s = Stuck::default();
    s.feed(key("read", "{}"), true);
    s.feed(key("read", "{}"), true);
    s.feed(key("shell", "{}"), true);
    assert_eq!(s.run, 1);
    s.feed(key("read", "{}"), true);
    assert_eq!(s.run, 1, "the run compares with the previous call only");
}

#[test]
fn stuck_reset_per_turn() {
    let d = dir("stuck");
    let (mut s, _) = session(&d, 9);
    s.stuck.feed(key("read", "{}"), true);
    s.stuck.feed(key("read", "{}"), true);
    assert_eq!(s.stuck.run, 2);
    // An empty history has nothing to continue, so the turn ends before any request; the stuck run is gone anyway.
    assert_eq!(s.turn(None), Outcome::error("config", "nothing to continue; give a TASK"));
    assert_eq!(s.stuck.run, 0);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn head_format() {
    let h = head(Path::new("/src/app"));
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    assert_eq!(h, format!("cwd: /src/app\nos: {os} {arch}\ndate: {}\n\n", crate::sys::local_date()));
    let date = h.lines().nth(2).unwrap().strip_prefix("date: ").unwrap();
    assert!(date.len() == 10 && date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }), "{date}");
}

#[test]
fn ctx_use_estimate() {
    assert_eq!(ctx_use((0, 0), 4_000), 1_000, "before any usage: a token per 4 bytes");
    assert_eq!(ctx_use((1_500, 4_000), 8_000), 2_500, "the last reply's tokens plus the bytes added since");
    assert_eq!(ctx_use((1_500, 4_000), 3_000), 1_500, "a smaller request adds nothing");
}

#[test]
fn reasoning_tail_500() {
    let long = "a".repeat(600) + "the end";
    assert_eq!(tail(&long, REASONING_TAIL).len(), 500);
    assert!(tail(&long, REASONING_TAIL).ends_with("the end"));
    assert_eq!(tail("short", REASONING_TAIL), "short");
    // A cut inside a character moves forward to the next one.
    let wide = "é".repeat(300);
    let t = tail(&wide, REASONING_TAIL);
    assert_eq!(t.len(), 500);
    assert_eq!(tail(&format!("x{wide}"), 501).len(), 500);
}

#[test]
fn record_hist_kinds() {
    let d = dir("hist");
    let (mut s, buf) = session(&d, 9);
    s.record(rec::approval("h1", "auto")).unwrap();
    s.notice("a notice").unwrap();
    s.record(rec::tool_result("h1", "shell", "ok", false, 1, 2)).unwrap();
    assert_eq!(kinds(&s.hist), ["tool_result"]);
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert_eq!(kinds(&lines(&buf)), ["approval", "notice", "tool_result"], "every record reaches --json stdout");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_request_timeout_cancels() {
    let d = dir("timeout");
    let l = TcpListener::bind("127.0.0.1:0").unwrap(); // accepts (the kernel does) and never answers
    let (mut s, _) = session(&d, l.local_addr().unwrap().port());
    s.lim.request = Duration::from_millis(200);
    assert_eq!(s.turn(Some("hi".into())), Outcome::error("provider", "request took over 0 s"));
    assert!(s.live.is_none() && s.conn.is_none() && s.open.is_empty());
    let (mut c, _) = l.accept().unwrap();
    c.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut req = Vec::new();
    c.read_to_end(&mut req).expect("the attempt's socket is shut within 1 s");
    assert!(req.starts_with(b"POST /v1/chat/completions HTTP/1.1\r\n"));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_plain_answer() {
    let d = dir("answer");
    let port = serve(vec![sse(&[delta(serde_json::json!({"content": "Hi"}), None), delta(serde_json::json!({}), Some("stop"))])]);
    let (mut s, buf) = session(&d, port);
    assert_eq!(s.turn(Some("say hi".into())), Outcome::Ok);
    assert_eq!((s.text(), s.steps()), ("Hi", 1));
    assert_eq!(kinds(&s.hist), ["user", "assistant"]);
    assert!(s.hist[0]["msg"]["content"].as_str().unwrap().ends_with("\n\nsay hi"));
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert_eq!(kinds(&lines(&buf)), ["user", "text", "assistant", "turn_end"], "journal order is stdout order");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_empty_answer_gives_up() {
    let d = dir("empty");
    let port = serve(vec![sse(&[delta(serde_json::json!({"reasoning_content": "thinking about it"}), None), delta(serde_json::json!({}), Some("stop"))])]);
    let (mut s, buf) = session(&d, port);
    assert_eq!(s.turn(Some("x".into())), Outcome::GaveUp("empty_answer"));
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let recs = lines(&buf);
    let notice = recs.iter().find(|r| r["t"] == "notice").unwrap();
    assert_eq!(notice["text"], "the model ended without an answer; its reasoning ended: thinking about it");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_retry_then_answer() {
    let d = dir("retry");
    let busy = "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n".to_string();
    let port = serve(vec![busy, sse(&[delta(serde_json::json!({"content": "ok"}), Some("stop"))])]);
    let (mut s, buf) = session(&d, port);
    assert_eq!(s.turn(Some("x".into())), Outcome::Ok);
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let recs = lines(&buf);
    assert_eq!(recs.iter().find(|r| r["t"] == "notice").unwrap()["text"], "HTTP 503; retry 1/4 in 0.0 s");
    assert_eq!(kinds(&recs), ["user", "notice", "text", "assistant", "turn_end"]);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_needs_approval_ids_and_not_run() {
    let d = dir("ask");
    let call = |id: &str, cmd: &str, i: u32| serde_json::json!({"index": i, "id": id, "type": "function", "function": {"name": "shell", "arguments": format!("{{\"command\":\"{cmd}\"}}")}});
    let reply = sse(&[delta(serde_json::json!({"tool_calls": [call("", "ls", 0), call("", "pwd", 1)]}), None), delta(serde_json::json!({}), Some("tool_calls"))]);
    let (mut s, buf) = session(&d, serve(vec![reply]));
    let o = s.turn(Some("x".into()));
    let Outcome::NeedsApproval(c) = &o else { panic!("{o:?}") };
    assert_eq!((c.id.as_str(), c.name.as_str()), ("h1", "shell"));
    assert_eq!(kinds(&s.hist), ["user", "assistant", "tool_result", "tool_result"]);
    assert_eq!(s.hist[1]["msg"]["tool_calls"][1]["id"], "h2", "an empty id gets the next h<N>");
    assert!(s.hist[2..].iter().all(|r| r["content"] == "not run: needs approval" && r["is_error"] == true));
    journal::pairing(&s.hist).unwrap();
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let recs = lines(&buf);
    assert_eq!(kinds(&recs), ["user", "assistant", "ask", "tool_result", "tool_result", "turn_end"]);
    assert_eq!(recs[2]["args"]["command"], "ls");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn wait_deadline_before_queued_events() {
    let d = dir("wait");
    let (mut s, _) = session(&d, 9);
    let out = ToolOutput { text: String::new(), bytes: 0, ms: 0, end: End::Exit(0) };
    s.tx.send(Ev::Tool { tag: 7, out }).unwrap();
    s.deadline = Some(Instant::now());
    assert_eq!(s.wait(7, None).unwrap_err(), Stop::Budget, "a queued event never holds off the deadline");
    s.finishing = true;
    assert!(matches!(s.wait(7, None), Ok(Some(Ev::Tool { tag: 7, .. }))), "finishing ignores the deadline");
    s.tx.send(Ev::OutputClosed).unwrap();
    s.tx.send(Ev::Signal(2)).unwrap();
    assert_eq!(s.wait(7, None).unwrap_err(), Stop::Signal(2), "finishing drops a closed stdout; a signal still ends the wait");
    assert!(s.hurry);
    std::fs::remove_dir_all(d).unwrap();
}
