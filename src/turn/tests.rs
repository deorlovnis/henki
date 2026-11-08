#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::approve::DENIED;
use super::call::Stuck;
use super::request::ctx_use;
use super::*;
use crate::{allow::Rule, ext::Ext, term::Mode};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, mpsc};

/// A terminal input at its end: a pipe whose write end is closed.
fn ended() -> std::io::PipeReader {
    std::io::pipe().unwrap().0
}

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

/// `Config::test(d)` with the model at `127.0.0.1:port`.
fn cfg(d: &Path, port: u16) -> Config {
    let mut cfg = Config::test(d);
    cfg.url.port = port;
    cfg
}

/// A session over `cfg` with no history, its stdout and stderr captured; a REPL session when `input` is given (the
/// reader reads it). The trust files are stamped first, as `config::load` stamps them at its end.
fn open(mut cfg: Config, mode: Mode, input: Option<std::io::PipeReader>) -> (Session, Buf, Buf) {
    cfg.snap = cfg.trust.iter().map(|p| (p.clone(), sys::stamp(p))).collect();
    journal::state_dir(&cfg.state).unwrap();
    let (ports, sid) = std_ports(&cfg);
    let (tx, rx) = mpsc::channel();
    let (out, err) = (Buf::default(), Buf::default());
    let o = Out::with(mode, tx.clone(), Box::new(out.clone()), Box::new(err.clone()), false);
    let reader = input.map(|i| Reader::with(i, tx.clone()).unwrap());
    (Session::new(cfg, ports, sid, Vec::new(), o, (tx, rx), None, reader), out, err)
}

/// The std bindings, with a new session held in the history.
fn std_ports(cfg: &Config) -> (Ports, String) {
    let mut ports = Ports { history: Box::new(crate::journal::Journal::new(&cfg.state)), policy: Box::new(crate::allow::Std), bound: Vec::new() };
    let sid = ports.history.create(&cfg.cwd, None).unwrap();
    (ports, sid)
}

/// A `--json` session in `d` with no history, its stdout captured, and the model at `127.0.0.1:port`.
fn session(d: &Path, port: u16) -> (Session, Buf) {
    let (s, out, _) = open(cfg(d, port), Mode::Json, None);
    (s, out)
}

/// A model server on loopback: request N gets `replies[N]` (a whole HTTP response), then the connection closes.
fn serve(replies: Vec<String>) -> u16 {
    serve_at(replies.into_iter().map(|r| (None, r)).collect()).0
}

/// `serve`, where a reply may come with a file that gets a line appended before the reply is sent. Also returns the
/// requests as they came.
fn serve_at(replies: Vec<(Option<PathBuf>, String)>) -> (u16, Arc<Mutex<Vec<Vec<u8>>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let (port, log) = (l.local_addr().unwrap().port(), Arc::new(Mutex::new(Vec::new())));
    let seen = Arc::clone(&log);
    std::thread::spawn(move || {
        for (mut c, (touch, reply)) in l.incoming().map(Result::unwrap).zip(replies) {
            seen.lock().unwrap().push(read_request(&mut c));
            touch.iter().for_each(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap().write_all(b"x\n").unwrap());
            c.write_all(reply.as_bytes()).unwrap();
        }
    });
    (port, log)
}

/// One whole request: the head, then Content-Length bytes of body.
fn read_request(c: &mut std::net::TcpStream) -> Vec<u8> {
    let (mut req, mut b) = (Vec::new(), [0; 65536]);
    while !req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() < body_end(&req) {
        let n = c.read(&mut b).unwrap();
        assert!(n > 0, "the client closed the request early");
        req.extend_from_slice(&b[..n]);
    }
    req
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
    json!({"choices": [{"index": 0, "delta": d, "finish_reason": finish}]})
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
fn stuck_three_identical_failures() {
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
    s.t.stuck.feed(key("read", "{}"), true);
    s.t.stuck.feed(key("read", "{}"), true);
    assert_eq!(s.t.stuck.run, 2);
    // An empty history has nothing to continue, so the turn ends before any request; the stuck run is gone anyway.
    assert_eq!(s.turn(None), Outcome::error("config", "nothing to continue; give a TASK"));
    assert_eq!(s.t.stuck.run, 0);
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
fn record_hist_kinds() {
    let d = dir("hist");
    let (mut s, buf) = session(&d, 9);
    s.record(rec::approval("h1", "auto", None)).unwrap();
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
    assert!(s.t.live.is_none() && s.t.conn.is_none() && s.t.open.is_empty());
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
    let port = serve(vec![sse(&[delta(json!({"content": "Hi"}), None), delta(json!({}), Some("stop"))])]);
    let (mut s, buf) = session(&d, port);
    assert_eq!(s.turn(Some("say hi".into())), Outcome::Ok);
    assert_eq!((s.t.text.as_str(), s.t.steps), ("Hi", 1));
    assert_eq!(kinds(&s.hist), ["user", "assistant"]);
    assert!(s.hist[0]["msg"]["content"].as_str().unwrap().ends_with("\n\nsay hi"));
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert_eq!(kinds(&lines(&buf)), ["user", "text", "assistant", "turn_end"], "journal order is stdout order");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn empty_answer_notice() {
    let d = dir("empty");
    let port = serve(vec![sse(&[delta(json!({"reasoning_content": "thinking about it"}), None), delta(json!({}), Some("stop"))])]);
    let (mut s, buf) = session(&d, port);
    assert_eq!(s.turn(Some("x".into())), Outcome::GaveUp("empty_answer"));
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let recs = lines(&buf);
    let notice = recs.iter().find(|r| r["t"] == "notice").unwrap();
    assert_eq!(notice["text"], "the model ended without an answer", "no reasoning tail");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn turn_retry_then_answer() {
    let d = dir("retry");
    let busy = "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n".to_string();
    let port = serve(vec![busy, sse(&[delta(json!({"content": "ok"}), Some("stop"))])]);
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
    let call = |id: &str, cmd: &str, i: u32| json!({"index": i, "id": id, "type": "function", "function": {"name": "shell", "arguments": format!("{{\"command\":\"{cmd}\"}}")}});
    let reply = sse(&[delta(json!({"tool_calls": [call("", "ls", 0), call("", "pwd", 1)]}), None), delta(json!({}), Some("tool_calls"))]);
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

#[test]
fn finish_clears_hurry() {
    let d = dir("hurry");
    let (mut s, _) = session(&d, 9);
    s.hurry = true; // a second signal during the last kill
    assert_eq!(s.finish(Outcome::Ok), Outcome::Ok);
    assert!(!s.hurry && !s.finishing, "the next turn's kill waits its grace again");
    std::fs::remove_dir_all(d).unwrap();
}

/// The session file under the test's state dir.
fn journal_file(d: &Path) -> PathBuf {
    let sessions = d.join("state/sessions");
    let scope = std::fs::read_dir(sessions).unwrap().next().unwrap().unwrap().path();
    std::fs::read_dir(scope).unwrap().map(|e| e.unwrap().path()).find(|p| p.extension().is_some_and(|x| x == "jsonl")).unwrap()
}

#[test]
fn end_writes_result_then_close() {
    let d = dir("end");
    let (mut s, buf) = session(&d, 9);
    s.t.text = "the answer".into();
    let id = s.sid.clone();
    s.end(0, Some(rec::result(&Outcome::Ok, "the answer", Usage::default(), Some(&id), 1, 1_234))).unwrap();
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let out = lines(&buf);
    assert_eq!(kinds(&out), ["result"]);
    assert_eq!((out[0]["status"].as_str(), out[0]["duration_s"].as_f64()), (Some("ok"), Some(1.2)));
    let journal = std::fs::read_to_string(journal_file(&d)).unwrap();
    let recs: Vec<Value> = journal.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(kinds(&recs), ["result", "close"]);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn end_journal_failure_json_result() {
    let d = dir("endfail");
    let (mut s, buf) = session(&d, 9);
    let id = s.sid.clone();
    // The file changes behind henki's back: every later write fails the size check.
    std::fs::OpenOptions::new().append(true).open(journal_file(&d)).unwrap().write_all(b"{\"t\":\"x\"}\n").unwrap();
    let e = s.end(2, Some(rec::result(&Outcome::Ok, "", Usage { input: 7, output: 3, cached: 0 }, Some(&id), 4, 2_500))).unwrap_err();
    let Outcome::Error { kind: "tampered", .. } = e else { panic!("{e:?}") };
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    let out = lines(&buf);
    assert_eq!(kinds(&out), ["result"], "no notice reached stdout, and exactly one result did");
    let r = &out[0];
    assert_eq!((r["status"].as_str(), r["exit"].as_i64(), r["error"]["kind"].as_str()), (Some("error"), Some(2), Some("tampered")));
    assert_eq!((r["session"].as_str(), r["steps"].as_u64(), r["duration_s"].as_f64(), r["usage"]["in"].as_u64()), (Some(id.as_str()), Some(0), Some(2.5), Some(0)));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn end_journal_failure_keeps_turn_error() {
    let d = dir("endfail2");
    let (mut s, buf) = session(&d, 9);
    let id = s.sid.clone();
    std::fs::OpenOptions::new().append(true).open(journal_file(&d)).unwrap().write_all(b"{\"t\":\"x\"}\n").unwrap();
    let auth = Outcome::error("auth", "bad key");
    let given = rec::result(&auth, "", Usage::default(), Some(&id), 0, 100);
    assert!(s.end(0, Some(given.clone())).is_err());
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert_eq!(lines(&buf), [given], "the turn's own error stays the result");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn end_journal_failure_plain_no_answer() {
    let d = dir("endplain");
    let (mut s, _) = session(&d, 9);
    let (tx, rx) = mpsc::channel();
    let (out, err) = (Buf::default(), Buf::default());
    s.out = Out::with(Mode::Plain, tx, Box::new(out.clone()), Box::new(err.clone()), false);
    drop(rx);
    s.t.text = "an answer".into();
    let id = s.sid.clone();
    std::fs::OpenOptions::new().append(true).open(journal_file(&d)).unwrap().write_all(b"{\"t\":\"x\"}\n").unwrap();
    assert!(s.end(0, Some(rec::result(&Outcome::Ok, "an answer", Usage::default(), Some(&id), 1, 0))).is_err());
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert!(out.0.lock().unwrap().is_empty(), "no answer after a failed result");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn poll_between_calls() {
    let d = dir("poll");
    let (mut s, _) = session(&d, 9);
    assert_eq!(s.poll(), Ok(()), "nothing queued");
    let out = ToolOutput { text: String::new(), bytes: 0, ms: 0, end: End::Exit(0) };
    s.tx.send(Ev::Part { tag: 3, part: Part::Text("old".into()) }).unwrap();
    s.tx.send(Ev::Tool { tag: 5, out }).unwrap();
    s.tx.send(Ev::Signal(15)).unwrap();
    assert_eq!(s.poll(), Err(Stop::Signal(15)), "a signal behind stale events");
    assert_eq!(s.poll(), Ok(()), "the stale events are gone");
    s.tx.send(Ev::OutputClosed).unwrap();
    assert_eq!(s.poll(), Err(Stop::Closed));
    s.deadline = Some(Instant::now());
    assert_eq!(s.poll(), Err(Stop::Budget));
    std::fs::remove_dir_all(d).unwrap();
}

/// A Ctrl-C that lands between calls is kept in `sig` too, so the cancel it causes sends SIGINT first.
#[test]
fn poll_sets_sig() {
    let d = dir("pollsig");
    let (mut s, _) = session(&d, 9);
    s.tx.send(Ev::Signal(libc::SIGINT)).unwrap();
    assert_eq!((s.poll(), s.sig), (Err(Stop::Signal(libc::SIGINT)), Some(libc::SIGINT)));
    s.tx.send(Ev::OutputClosed).unwrap();
    assert_eq!((s.poll(), s.sig), (Err(Stop::Closed), Some(libc::SIGINT)), "a closed stdout leaves the last signal");
    std::fs::remove_dir_all(d).unwrap();
}

/// A call to `read` a file that does not exist: an error result without running anything.
fn read_missing(i: u32) -> Value {
    json!({"index": i, "id": format!("c{i}"), "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"missing\"}"}})
}

fn calls_reply(calls: Vec<Value>, usage: Value) -> String {
    sse(&[delta(json!({"tool_calls": calls}), None), json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}], "usage": usage})])
}

#[test]
fn stuck_reset_per_turn_across_requests() {
    let d = dir("stuck2");
    let say = |t: &str| sse(&[delta(json!({"content": t}), Some("stop"))]);
    let fail = |i| calls_reply(vec![read_missing(i)], Value::Null);
    let (mut s, _) = session(&d, serve(vec![fail(0), fail(1), say("no"), fail(2), say("still no")]));
    assert_eq!(s.turn(Some("read it".into())), Outcome::Ok);
    assert_eq!(s.t.stuck.run, 2, "two failed identical calls");
    assert_eq!(s.turn(Some("again".into())), Outcome::Ok, "a third identical failure in a new turn starts a new run");
    assert_eq!(s.t.stuck.run, 1);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn usage_overflow_no_panic() {
    let d = dir("usage");
    let max = json!({"prompt_tokens": u64::MAX, "completion_tokens": 5});
    let answer = sse(&[delta(json!({"content": "done"}), None), json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": max})]);
    let (mut s, _) = session(&d, serve(vec![calls_reply(vec![read_missing(0)], max.clone()), answer]));
    assert_eq!(s.turn(Some("count".into())), Outcome::Ok);
    assert_eq!(s.usage_proc, Usage { input: 2 << 32, output: 10, cached: 0 }, "each reply's counts are capped at 2^32");
    assert_eq!(s.ctx.0, (1 << 32) + 5);
    assert_eq!(s.hist[1]["usage"]["in"], 1u64 << 32, "the journal holds the capped count");
    std::fs::remove_dir_all(d).unwrap();
}

/// An assistant record with calls of these ids.
fn assistant_with(ids: &[&str]) -> Value {
    let calls: Vec<Call> = ids.iter().map(|id| Call { id: (*id).into(), name: "shell".into(), args: json!({"command": "ls"}), ..Call::default() }).collect();
    rec::assistant("", &calls, "tool_calls", Usage::default(), None)
}

fn shell_call(id: &str) -> Call {
    Call { id: id.into(), name: "shell".into(), args: json!({"command": "ls"}), ..Call::default() }
}

#[test]
fn prepare_reuses_model_ids() {
    let d = dir("ids");
    let (mut s, _) = session(&d, 9);
    s.hist = vec![assistant_with(&["call_1", "h3"])];
    s.next_h = journal::next_h(&s.hist);
    let calls = s.prepare(vec![shell_call("call_1"), shell_call(""), shell_call("x"), shell_call("x")]);
    let ids: Vec<&str> = calls.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["h4", "h5", "x", "h6"], "an id used earlier or in this reply gets the next h<N>");
    assert!(calls.iter().all(|c| c.error.is_none()), "validated");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn prepare_id_near_u32_max() {
    let d = dir("idmax");
    let (mut s, _) = session(&d, 9);
    s.hist = vec![assistant_with(&["h4294967295"])];
    s.next_h = journal::next_h(&s.hist);
    assert_eq!(s.next_h, u32::MAX);
    let calls = s.prepare(vec![shell_call(""), shell_call("")]);
    assert_eq!((calls[0].id.as_str(), calls[1].id.as_str()), ("h0", "h1"));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn gave_up_fix_message() {
    let d = dir("fix");
    let (mut s, _) = session(&d, 9);
    let down =
        |host: &str| ProviderError { kind: "provider", message: format!("could not connect to {host}"), retry: Some(format!("could not connect to {host}")), retry_after: None };
    let msg = |s: &Session, e| match s.gave_up(e, 5) {
        Outcome::Error { message, .. } => message,
        o => panic!("{o:?}"),
    };
    assert_eq!(msg(&s, down("127.0.0.1:9")), "could not connect to 127.0.0.1:9 (gave up after 5 attempts); fix: start the model server on 127.0.0.1:9 or set HENKI_BASE_URL");
    s.cfg.file = Some(d.join("config.toml"));
    let file = d.join("config.toml").display().to_string();
    assert_eq!(msg(&s, down("x")), format!("could not connect to x (gave up after 5 attempts); fix: start the model server on 127.0.0.1:9 or set base_url in {file}"));
    s.cfg.url_from = "HENKI_BASE_URL";
    assert!(msg(&s, down("x")).ends_with("or set HENKI_BASE_URL"), "the URL came from the environment");
    s.cfg.url.host = "::1".into();
    assert!(msg(&s, down("[::1]:9")).contains("; fix: start the model server on [::1]:9 or"));
    s.cfg.url.host = "10.1.2.3".into();
    assert_eq!(msg(&s, down("10.1.2.3:9")), "could not connect to 10.1.2.3:9 (gave up after 5 attempts)", "no fix for a remote server");
    s.cfg.url.host = "127.0.0.1".into();
    let busy = ProviderError { kind: "provider", message: "HTTP 503: busy".into(), retry: Some("HTTP 503".into()), retry_after: None };
    assert_eq!(msg(&s, busy), "HTTP 503: busy (gave up after 5 attempts)", "no fix for another cause");
    assert_eq!(s.gave_up(http::perr("auth", "bad key"), 1), Outcome::error("auth", "bad key"), "not retryable: as sent");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn request_timeout_cancels_every_attempt() {
    let d = dir("attempts");
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let first = l.try_clone().unwrap();
    // Attempt 1 gets a retryable error; attempt 2 is accepted by the kernel and never answered.
    let busy = std::thread::spawn(move || {
        let (mut c, _) = first.accept().unwrap();
        read_request(&mut c);
        c.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n").unwrap();
    });
    let (mut s, _) = session(&d, port);
    s.lim.request = Duration::from_millis(200);
    s.hist.push(rec::user("hi"));
    assert_eq!(s.request().unwrap_err(), Outcome::error("provider", "request took over 0 s"));
    assert!(s.t.conn.is_none(), "the timed-out attempt's slot was taken");
    busy.join().unwrap();
    let (mut c, _) = l.accept().unwrap();
    c.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut req = Vec::new();
    c.read_to_end(&mut req).expect("the request itself shut the timed-out attempt's socket within 1 s");
    assert!(req.starts_with(b"POST /v1/chat/completions HTTP/1.1\r\n"));
    std::fs::remove_dir_all(d).unwrap();
}

/// Every record of the session file under `d`'s state dir, in order.
fn journal(d: &Path) -> Vec<Value> {
    std::fs::read_to_string(journal_file(d)).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn say(t: &str) -> String {
    sse(&[delta(json!({"content": t}), Some("stop"))])
}

/// A reply with one shell call.
fn shell_reply(cmd: &str) -> String {
    let call = json!({"index": 0, "id": "c1", "type": "function", "function": {"name": "shell", "arguments": json!({"command": cmd}).to_string()}});
    calls_reply(vec![call], Value::Null)
}

fn text(buf: &Buf) -> String {
    String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn head_first_message_only() {
    let d = dir("head1");
    let (mut s, _) = session(&d, serve(vec![say("one"), say("two")]));
    assert_eq!(s.turn(Some("first".into())), Outcome::Ok);
    assert_eq!(s.turn(Some("second".into())), Outcome::Ok);
    let users: Vec<&str> = s.hist.iter().filter(|r| r["t"] == "user").map(|r| r["msg"]["content"].as_str().unwrap()).collect();
    assert!(users[0].starts_with(&format!("cwd: {}\n", d.display())) && users[0].ends_with("\n\nfirst"), "{}", users[0]);
    assert_eq!(users[1], "second", "a later message carries no head");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn stuck_denied_resets() {
    let d = dir("denied");
    let (mut s, _) = session(&d, 9);
    let c = shell_call("h1");
    let k = key("shell", &c.args.to_string());
    s.t.stuck.feed(k.clone(), true);
    s.t.stuck.feed(k.clone(), true);
    assert_eq!(s.t.stuck.run, 2);
    s.answer(&c, DENIED, true, None, None).unwrap();
    assert_eq!((s.t.stuck.run, s.hist[0]["content"].as_str(), s.hist[0]["is_error"].as_bool()), (0, Some(DENIED), Some(true)), "a denied call is in no run");
    s.t.stuck.feed(k, true);
    assert_eq!(s.t.stuck.run, 1, "the next failure starts a new run");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn body_by_dialect() {
    let d = dir("body");
    for dialect in ["openai", "anthropic"] {
        let mut c = Config::test(&d);
        c.dialect = dialect;
        let (mut s, _, _) = open(c, Mode::Json, None);
        s.hist.push(rec::user("hi"));
        let render = if dialect == "anthropic" { anthropic::body } else { crate::openai::body };
        assert_eq!(s.body(), render("m", 4_096, &s.system, &s.tools, &s.hist), "{dialect}");
    }
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn stall_by_dialect() {
    let d = dir("stall");
    let (s, _) = session(&d, 9);
    assert_eq!(s.lim.stall, Duration::from_secs(60));
    let mut c = Config::test(&d);
    c.dialect = "anthropic";
    let (s, _, _) = open(c, Mode::Json, None);
    assert_eq!((s.lim.stall, anthropic::STALL), (Duration::from_secs(300), Duration::from_secs(300)));
    assert_eq!((s.lim.first_byte, s.lim.request), (http::LIMITS.first_byte, http::LIMITS.request), "only the stall differs");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn elision_openai_only() {
    let d = dir("elide");
    let result = |id: &str, text: &str| rec::tool_result(id, "shell", text, false, 1, text.len() as u64);
    let hist = vec![rec::user("x"), assistant_with(&["c1"]), result("c1", "old output"), assistant_with(&["c2"]), result("c2", "new"), assistant_with(&[])];
    for (dialect, elided) in [("openai", true), ("anthropic", false)] {
        let mut c = Config::test(&d);
        c.dialect = dialect;
        let (mut s, _, _) = open(c, Mode::Json, None);
        s.hist.clone_from(&hist);
        assert_eq!(s.elide().unwrap(), elided, "{dialect}");
        assert_eq!(s.hist[2]["content"] == "old output", !elided, "{dialect}: {}", s.hist[2]);
    }
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn line_stale_tag_dropped() {
    let d = dir("line");
    let (r, mut w) = std::io::pipe().unwrap();
    let (mut s, _, _) = open(Config::test(&d), Mode::Repl, Some(r));
    w.write_all(b"one\r").unwrap();
    assert_eq!(s.line("> "), Ok(Some(("one".into(), true))));
    // Input read for an earlier question, or after MAIN stopped waiting (tag 0), is not this line's.
    s.tx.send(Ev::Input { tag: s.tag, bytes: Some(b"old\r".to_vec()) }).unwrap();
    s.tx.send(Ev::Input { tag: 0, bytes: Some(b"stale\r".to_vec()) }).unwrap();
    // Two writes 100 ms apart: two reads. Whether more input waits is asked of the reader's own input, never of fd 0.
    let late = std::thread::spawn(move || {
        w.write_all(b"tw").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        w.write_all(b"o\n").unwrap();
        w
    });
    assert_eq!(s.line("> "), Ok(Some(("two".into(), true))), "a line over two reads");
    let mut w = late.join().unwrap();
    w.write_all(b"\x1b[200~p\nq\x1b[201~\r").unwrap();
    assert_eq!(s.line("> "), Ok(Some(("p\nq".into(), false))), "a paste is not typed");
    drop(w);
    assert_eq!(s.line("> "), Ok(None), "EOF");
    let (mut run, _) = session(&d, 9);
    assert_eq!(run.line("> "), Ok(None), "no reader");
    std::fs::remove_dir_all(d).unwrap();
}

/// A session whose trust files are `a` and `allow` (both exist; `allow` is its allow file) and `c` (absent) in `d`.
fn trusting(d: &Path) -> (Session, [PathBuf; 3]) {
    let paths = ["a", "allow", "c"].map(|n| d.join(n));
    paths[..2].iter().for_each(|p| std::fs::write(p, "1").unwrap());
    let mut c = Config::test(d);
    c.trust = paths.to_vec();
    (open(c, Mode::Json, None).0, paths)
}

#[test]
fn changed_lists_every_path() {
    let d = dir("changed");
    let (s, [a, _, c]) = trusting(&d);
    assert_eq!(s.changed(), Vec::<PathBuf>::new());
    std::fs::write(&c, "new").unwrap();
    std::fs::write(&a, "22").unwrap();
    assert_eq!(s.changed(), [a, c], "every changed path, in trust order; a created file is a change");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn rearm_clears() {
    let d = dir("rearm");
    let (mut s, [_, allow, _]) = trusting(&d);
    std::fs::write(&allow, "henki's own write").unwrap();
    assert_eq!(s.changed(), std::slice::from_ref(&allow));
    s.rearm();
    assert_eq!(s.changed(), Vec::<PathBuf>::new(), "a new stamp of the allow file");
    std::fs::remove_file(&allow).unwrap();
    assert_eq!(s.changed(), [allow], "a removed file is a change too");
    std::fs::remove_dir_all(d).unwrap();
}

/// henki's own write to the allow file never takes in a change to another trust file made meanwhile.
#[test]
fn rearm_keeps_other_changes() {
    let d = dir("rearm2");
    let (mut s, [a, allow, c]) = trusting(&d);
    std::fs::write(&a, "an outside change").unwrap();
    std::fs::write(&c, "created outside").unwrap();
    std::fs::write(&allow, "henki's own write").unwrap();
    s.rearm();
    assert_eq!(s.changed(), [a, c], "only the allow file is stamped again");
    std::fs::remove_dir_all(d).unwrap();
}

/// The snapshot is the one `config` took when it read the files: a change made after that and before the session
/// starts (while `run -` reads its task from stdin, say) is found at the first check.
#[test]
fn snapshot_from_config() {
    let d = dir("snapcfg");
    let agents = d.join("AGENTS.md");
    std::fs::write(&agents, "read at start\n").unwrap();
    let mut c = Config::test(&d);
    c.trust = vec![agents.clone()];
    c.snap = vec![(agents.clone(), sys::stamp(&agents))];
    std::fs::write(&agents, "changed before the session started\n").unwrap();
    journal::state_dir(&c.state).unwrap();
    let (ports, sid) = std_ports(&c);
    let (tx, rx) = mpsc::channel();
    let o = Out::with(Mode::Json, tx.clone(), Box::new(Buf::default()), Box::new(Buf::default()), false);
    let s = Session::new(c, ports, sid, Vec::new(), o, (tx, rx), None, None);
    assert_eq!(s.changed(), [agents]);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn finishing_tripwire() {
    let d = dir("finishing");
    let agents = d.join("AGENTS.md");
    // The server changes the trust file after the call's checks, before it sends the plain answer.
    let (port, _) = serve_at(vec![(None, calls_reply(vec![read_missing(0)], Value::Null)), (Some(agents.clone()), say("done"))]);
    let mut c = cfg(&d, port);
    c.trust = vec![agents.clone()];
    let (mut s, _, _) = open(c, Mode::Json, None);
    let m = format!("trust file changed: {}", agents.display());
    assert_eq!(s.turn(Some("x".into())), Outcome::error("changed", m.clone()));
    let recs = journal(&d);
    assert_eq!(kinds(&recs), ["user", "assistant", "tool_result", "assistant", "notice", "turn_end"]);
    assert_eq!((recs[4]["text"].as_str(), recs[5]["status"].as_str()), (Some(m.as_str()), Some("error")));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn tripwire_before_a_call() {
    let d = dir("trip");
    let agents = d.join("AGENTS.md");
    std::fs::write(d.join("x.txt"), "x\n").unwrap();
    let read = json!({"index": 0, "id": "c0", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"x.txt\"}"}});
    let (port, _) = serve_at(vec![(Some(agents.clone()), calls_reply(vec![read, read_missing(1)], Value::Null))]);
    let mut c = cfg(&d, port);
    c.trust = vec![agents.clone()];
    let (mut s, _, _) = open(c, Mode::Json, None);
    let m = format!("trust file changed: {}", agents.display());
    assert_eq!(s.turn(Some("x".into())), Outcome::error("changed", m.clone()), "checked before the first call is approved");
    let recs = journal(&d);
    assert_eq!(kinds(&recs), ["user", "assistant", "notice", "tool_result", "tool_result", "turn_end"], "nothing approved, nothing run");
    assert!(recs[3..5].iter().all(|r| r["content"] == "not run: trust file changed"));
    std::fs::remove_dir_all(d).unwrap();
}

/// `Config::test(d)` with the session in `d/app`, which is created.
fn in_app(d: &Path, port: u16) -> Config {
    let mut c = cfg(d, port);
    (c.cwd, c.root) = (d.join("app"), d.join("app"));
    std::fs::create_dir_all(&c.cwd).unwrap();
    c
}

#[test]
fn rule_dispatch_records() {
    let d = dir("rule");
    let mut c = in_app(&d, serve(vec![shell_reply("git status"), say("done")]));
    c.bash = d.join("no-bash"); // approved by the rule, then not started: a unit test runs no process
    c.allow.rules = vec![Rule { n: 1, root: d.join("app").display().to_string(), cmd: "git status".into() }];
    let (mut s, _, err) = open(c, Mode::Plain, None);
    assert_eq!(s.turn(Some("x".into())), Outcome::Ok);
    let recs = journal(&d);
    assert_eq!(kinds(&recs), ["user", "assistant", "approval", "tool_result", "assistant", "turn_end"], "no ask");
    assert_eq!((recs[2]["verdict"].as_str(), recs[2]["rule"].as_u64()), (Some("rule"), Some(1)));
    assert_eq!(recs[3]["content"], "[not started: ENOENT bash]");
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert_eq!(text(&err), "$ git status  (allowed by rule 1 for ~/app, ~/allow)\n$ git status -> error\n");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn rule_other_root_needs_approval() {
    let d = dir("rule2");
    let mut c = in_app(&d, serve(vec![shell_reply("git status")]));
    c.allow.rules = vec![Rule { n: 1, root: d.display().to_string(), cmd: "git status".into() }];
    let (mut s, _, _) = open(c, Mode::Json, None);
    let o = s.turn(Some("x".into()));
    assert!(matches!(&o, Outcome::NeedsApproval(c) if c.args["command"] == "git status"), "{o:?}");
    assert_eq!(kinds(&journal(&d)), ["user", "assistant", "ask", "tool_result", "turn_end"], "a rule for another root asks");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn free_read_ext_doc_auto_approval() {
    let d = dir("extdoc");
    let (doc, other) = (d.join("ext/tool/doc.md"), d.join("ext/tool/notes.md"));
    std::fs::create_dir_all(d.join("ext/tool")).unwrap();
    std::fs::write(&doc, "# tool: does things\n").unwrap();
    std::fs::write(&other, "notes\n").unwrap();
    let mut c = in_app(&d, 9);
    let ext = Ext {
        name: "tool".into(),
        line: "tool: does things".into(),
        dir: d.join("ext/tool"),
        doc: doc.clone(),
        bin: None,
        cmds: Vec::new(),
        user: Vec::new(),
        on: Vec::new(),
        prompt: true,
        shadows: Vec::new(),
        paths: Vec::new(),
        targets: Vec::new(),
    };
    c.exts = vec![ext];
    let (mut s, _, _) = open(c, Mode::Json, None);
    // Ask mode and outside the root: the doc is approved and started; the wait then ends at the deadline, so nothing is
    // reaped in a unit test.
    s.deadline = Some(Instant::now());
    let read = |id: &str, p: &Path| Call { id: id.into(), name: "read".into(), args: json!({"path": p}), ..Call::default() };
    assert_eq!(s.dispatch(&read("h1", &doc)), Err(Outcome::Budget));
    let o = s.dispatch(&read("h2", &other));
    assert!(matches!(o, Err(Outcome::NeedsApproval(_))), "another file of the extension asks: {o:?}");
    let recs = journal(&d);
    assert_eq!(kinds(&recs), ["approval", "ask"]);
    assert_eq!((recs[0]["id"].as_str(), recs[0]["verdict"].as_str()), (Some("h1"), Some("auto")));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn outcome_line_forms() {
    assert_eq!(outcome_line(&Outcome::GaveUp("stuck")), "(gave up: stuck)");
    assert_eq!(outcome_line(&Outcome::error("auth", "bad key")), "(error auth: bad key)");
    assert_eq!(outcome_line(&Outcome::error("changed", "trust file changed: /c")), "(error changed: trust file changed: /c)");
    assert_eq!(outcome_line(&Outcome::NeedsApproval(shell_call("h1"))), "(needs approval: $ ls; henki --resume continues this session in the REPL)");
    assert_eq!(outcome_line(&Outcome::Budget), "(budget)");
    assert_eq!(outcome_line(&Outcome::Interrupted), "(interrupted)");
    assert_eq!(outcome_line(&Outcome::OutputClosed), "(output closed)");
}

#[test]
fn discard_only_without_user() {
    let repl = |d: &Path| open(Config::test(d), Mode::Repl, Some(ended())).0;
    let d = dir("discard1");
    let mut s = repl(&d);
    let file = journal_file(&d);
    s.end(0, None).unwrap();
    assert!(!file.exists(), "a REPL that got no message leaves no session");
    let d2 = dir("discard2");
    let mut s = repl(&d2);
    s.record(rec::user("hi")).unwrap();
    s.end(0, None).unwrap();
    assert_eq!(kinds(&journal(&d2)), ["user", "close"]);
    let d3 = dir("discard3");
    let (mut s, _) = session(&d3, 9);
    s.end(0, None).unwrap();
    assert_eq!(kinds(&journal(&d3)), ["close"], "run keeps its session");
    [d, d2, d3].into_iter().for_each(|d| std::fs::remove_dir_all(d).unwrap());
}

/// A REPL session that got no message has no journal, but the processes the session end stopped are still named.
#[test]
fn discard_notes_stopped_processes() {
    let d = dir("discard4");
    let (mut s, out, err) = open(Config::test(&d), Mode::Repl, Some(ended()));
    let file = journal_file(&d);
    s.end(2, None).unwrap();
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert!(!file.exists(), "no session file");
    assert_eq!((text(&out), text(&err)), (String::new(), "henki: stopped 2 background processes at exit\n".to_string()));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn notices_repl_journaled_only() {
    let d = dir("notices");
    let mut c = cfg(&d, serve(vec![say("hi")]));
    c.notices = vec!["a notice".into()];
    let (mut s, out, err) = open(c, Mode::Repl, Some(ended()));
    assert_eq!(s.turn(Some("x".into())), Outcome::Ok);
    s.into_out().flush(Instant::now() + Duration::from_secs(1));
    assert!(text(&out).starts_with("hi"), "the answer streams to stdout");
    assert_eq!(text(&err), "", "the banner printed the notice already");
    assert_eq!((journal(&d)[0]["t"].as_str(), journal(&d)[0]["text"].as_str()), (Some("notice"), Some("a notice")));
    std::fs::remove_dir_all(d).unwrap();
}

/// An Anthropic stream of one text block.
fn anthropic_sse(text: &str) -> String {
    let events = [
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 2}}),
        json!({"type": "message_stop"}),
    ];
    let body: String = events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect();
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{body}")
}

#[test]
fn turn_anthropic_seam() {
    let d = dir("anthropic");
    let (port, log) = serve_at(vec![(None, anthropic_sse("Hi"))]);
    let mut c = cfg(&d, port);
    (c.dialect, c.url.path, c.api_key) = ("anthropic", "/model".into(), "cred-managed".into());
    let (mut s, _, _) = open(c, Mode::Json, None);
    assert_eq!(s.turn(Some("say hi".into())), Outcome::Ok);
    assert_eq!(s.t.text, "Hi");
    assert_eq!(s.hist[1]["blocks"], json!([{"text": "Hi", "type": "text"}]), "the blocks are journaled for replay");
    let req = String::from_utf8_lossy(&log.lock().unwrap()[0]).to_ascii_lowercase();
    assert!(req.starts_with("post /model/v1/messages http/1.1\r\n"), "{req}");
    assert!(req.contains("\r\nanthropic-version: 2023-06-01\r\n") && req.contains("\r\nx-api-key: cred-managed\r\n"), "{req}");
    std::fs::remove_dir_all(d).unwrap();
}

/// An Anthropic stream of `read` calls of missing files, one tool_use block per id (as sent: maybe empty or repeated).
fn anthropic_reads(ids: &[&str]) -> String {
    let mut events = vec![json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}})];
    for (i, id) in ids.iter().enumerate() {
        events.push(json!({"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": id, "name": "read", "input": {}}}));
        events.push(json!({"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": format!("{{\"path\":\"missing{i}\"}}")}}));
        events.push(json!({"type": "content_block_stop", "index": i}));
    }
    events.push(json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 2}}));
    events.push(json!({"type": "message_stop"}));
    let body: String = events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap())).collect();
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{body}")
}

/// The tool_use ids and the tool_result ids of an Anthropic request body, in order.
fn anthropic_ids(req: &[u8]) -> (Vec<String>, Vec<String>) {
    let start = String::from_utf8_lossy(req).find("\r\n\r\n").unwrap() + 4;
    let body: Value = serde_json::from_slice(&req[start..body_end(req)]).unwrap();
    let blocks: Vec<Value> = body["messages"].as_array().unwrap().iter().flat_map(|m| m["content"].as_array().unwrap().clone()).collect();
    let ids = |ty: &str, k: &str| blocks.iter().filter(|b| b["type"] == ty).map(|b| b[k].as_str().unwrap().to_string()).collect();
    (ids("tool_use", "id"), ids("tool_result", "tool_use_id"))
}

/// A call id the reply left empty or used twice gets a new name, and the replayed tool_use blocks carry it, so every
/// tool_result answers a tool_use of the same id (the API refuses a request where they differ).
#[test]
fn anthropic_renamed_ids_replayed() {
    let d = dir("anthropic-ids");
    let (port, log) = serve_at(vec![(None, anthropic_reads(&["dup", "dup", ""])), (None, anthropic_sse("done"))]);
    let mut c = cfg(&d, port);
    c.dialect = "anthropic";
    let (mut s, _, _) = open(c, Mode::Json, None);
    assert_eq!(s.turn(Some("read it".into())), Outcome::Ok);
    let (uses, results) = anthropic_ids(&log.lock().unwrap()[1]);
    assert_eq!(uses, results, "every tool_use id is answered by the tool_result after it");
    assert_eq!((uses[0].as_str(), uses[1] != "dup" && !uses[2].is_empty() && uses[1] != uses[2]), ("dup", true), "{uses:?}");
    let journaled: Vec<&str> = s.hist[1]["blocks"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(journaled, uses, "the journal replays the same ids after a resume");
    std::fs::remove_dir_all(d).unwrap();
}
