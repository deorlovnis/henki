//! An outside port binding: a program in a package that speaks `port/1` (JSON lines on its stdin and stdout, in the
//! manner of LSP) for one port. Started once per session as a kernel child (its own session and process group, the tool
//! environment without the extensions, stderr to the kernel log), greeted with `hello`, ended with `bye` and then the
//! kill of its group. MAIN waits for an answer on this binding's own channel, never longer than the method's contract
//! timeout; for the methods that change nothing a signal or the session deadline cuts the wait short (`interrupted`,
//! `budget`).
//! A timeout, an exit or a broken line kills the binding for the rest of the session: every later call fails at once
//! with that error, and the kernel's floor takes over (history: the turn ends `error journal`).
use crate::port::{self, History, PortError, Record, Taken, err};
use crate::{supervise, sys, term, term::Out, types::Ev, types::Outcome};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::{ffi::OsString, path::Path, process::Child, process::Stdio, thread, time::Duration, time::Instant};

/// Both directions. A record holds up to a 1 MiB typed line or a model reply, JSON-escaped.
pub const LINE_MAX: usize = 16 << 20;
/// What one `read` of a whole session may bring, over all its pages.
const READ_MAX: usize = 256 << 20;
/// The contract timeout of each method in ms, and whether a signal or the deadline ends the wait (side-effect free only).
#[rustfmt::skip]
const METHODS: [(&str, u64, bool); 8] = [("hello", 5_000, true), ("create", 2_000, true), ("open", 5_000, true), ("read", 10_000, true), ("list", 5_000, true),
    ("append", 2_000, false), ("discard", 2_000, false), ("bye", 300, false)];

/// One line from the binding: the object and its length, or why the binding is broken.
type Line = Result<(Value, usize), String>;

pub struct Remote {
    pub port: &'static str,
    /// `PKG ID` from the link's store path (or the path as shown): what notices and records name. Never the binding's own
    /// word, which is `said`.
    pub name: String,
    /// `NAME IMPL` from hello.
    pub said: String,
    pub caps: Vec<String>,
    /// The session deadline (`run --max-seconds`): ends interruptible waits like a signal.
    pub deadline: Option<Instant>,
    /// What every failure of a session's binding ends with: the kernel log and the repair.
    fix: String,
    /// Some until the binding is killed and reaped (supervise::kernel_end takes it), so it is never signalled after.
    child: Option<Child>,
    tx: Sender<String>,
    rx: Receiver<Line>,
    next: u64,
    got: usize,
    dead: Option<PortError>,
}

impl Remote {
    /// Starts `exe` in `cwd` with exactly `env` and `stderr` (the kernel log; the terminal for `henki port`); `fix`
    /// ends every failure's message.
    pub fn start(port: &'static str, exe: &Path, (name, fix): (String, String), cwd: &Path, env: &[(OsString, OsString)], stderr: Stdio) -> Result<Remote, PortError> {
        let ((tx, wrx), (rtx, rx)) = (mpsc::channel::<String>(), mpsc::channel());
        let mut r = Remote { port, name, said: String::new(), caps: Vec::new(), deadline: None, fix, child: None, tx, rx, next: 0, got: 0, dead: None };
        let mut c = supervise::kernel_child(exe, cwd, env, [Stdio::piped(), Stdio::piped(), stderr]).map_err(|e| r.named("crashed", &format!("not started: {e}")))?;
        let (stdin, stdout) = (c.stdin.take(), c.stdout.take());
        r.child = Some(c);
        let (Some(mut w), Some(out)) = (stdin, stdout) else { return Err(r.fail(err("crashed", "no pipes"))) };
        // The writer is a thread: a binding that stops reading fills its pipe, and MAIN must never block in write(2).
        let threads =
            thread::Builder::new().spawn(move || wrx.iter().try_for_each(|l| w.write_all(l.as_bytes()))).and_then(|_| thread::Builder::new().spawn(move || lines(out, &rtx)));
        threads.map_err(|e| r.fail(err("crashed", format!("cannot start a thread: {e}"))))?;
        Ok(r)
    }

    /// `hello` for the session cwd `scope`, naming `state` (the binding's private dir under henki's protected state dir),
    /// offering `versions` of the port's method set over port/1 protocol 1.
    pub fn hello(&mut self, scope: &Path, state: &Path, versions: &[u64]) -> Result<(), PortError> {
        let hello = json!({"protocol": [1], "port": self.port, "versions": versions, "henki": env!("CARGO_PKG_VERSION"), "scope": scope, "state": state});
        // A binding that did not say hello is never used: no bye, so a signal at a slow hello ends the start at once.
        let h = self.call("hello", hello).map_err(|e| self.end(e))?;
        let word = |k: &str| h[k].as_str().filter(|s| (1..=64).contains(&s.len()) && !s.contains(|c: char| c.is_whitespace() || term::escaped(c)));
        let (Some(name), Some(v), true, true) = (word("name"), word("impl"), h["protocol"] == 1, versions.iter().any(|&v| h["version"] == v)) else {
            let ours = format!("henki {} speaks port/1 {}/{versions:?}", env!("CARGO_PKG_VERSION"), self.port);
            return Err(self.fail(err("version", format!("hello answered {}; {ours}", term::json_line(&h)))));
        };
        self.said = format!("{name} {v}");
        self.caps = h["caps"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).take(16).collect();
        Ok(())
    }

    /// One request and its answer within the method's timeout: the result, or the binding's error (the binding lives
    /// on), or the kernel's (`timeout`, `crashed`, `protocol`: the binding is dead; `interrupted`: it is not, and a late
    /// answer is dropped as stale).
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, PortError> {
        if let Some(e) = &self.dead {
            return Err(e.clone());
        }
        let &(_, ms, cut) = METHODS.iter().find(|m| m.0 == method).unwrap_or(&("", 5_000, false));
        let line = term::json_line(&json!({"id": self.next, "method": method, "params": params})) + "\n";
        if line.len() > LINE_MAX {
            return Err(err("invalid", format!("{} binding {}: {method}: a line over {} bytes", self.port, self.name, term::num(LINE_MAX as u64))));
        }
        let (id, end, seen) = (self.next, Instant::now() + Duration::from_millis(ms), sys::seen());
        self.next += 1;
        let _ = self.tx.send(line);
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if cut && (sys::seen() != seen || self.deadline.is_some_and(|d| Instant::now() >= d)) {
                return Err(err(if sys::seen() != seen { "interrupted" } else { "budget" }, format!("{method}: interrupted")));
            }
            match self.rx.recv_timeout(left.min(Duration::from_millis(50))) {
                Ok(Ok((v, n))) if v["id"] == id => {
                    self.got += n;
                    return match (v.get("result"), v.get("error").map(|e| e["code"].as_str())) {
                        (Some(r), None) => Ok(r.clone()),
                        (None, Some(Some(c))) => {
                            Err(self.named(port::code(c), &format!("{}: {}", term::escape(c), term::escape(v["error"]["message"].as_str().unwrap_or_default()))))
                        }
                        _ => Err(self.fail(err("protocol", "an answer holds one of result and error"))),
                    };
                }
                Ok(Ok(_)) => {} // the late answer to an interrupted call
                Ok(Err(why)) => return Err(self.fail(err("protocol", why))),
                Err(RecvTimeoutError::Disconnected) => return Err(self.fail(err("crashed", "it closed its stdout or exited"))),
                Err(_) if left.is_zero() => return Err(self.fail(err("timeout", format!("no answer to {method} in {} s", term::secs(ms))))),
                Err(_) => {}
            }
        }
    }

    /// The binding is gone for this session: killed and reaped, and `e`, named after it, answers every later call.
    fn fail(&mut self, e: PortError) -> PortError {
        let e = self.named(e.code, &format!("{}: {}", e.code, e.message));
        self.end(e)
    }

    fn end(&mut self, e: PortError) -> PortError {
        supervise::kernel_end(self.child.take());
        self.dead = Some(e.clone());
        e
    }

    /// `code`, with a message naming the binding and ending with the fix.
    fn named(&self, code: &'static str, what: &str) -> PortError {
        err(code, format!("{} binding {}: {what}{}{}", self.port, self.name, if self.fix.is_empty() { "" } else { "; " }, self.fix))
    }
}

/// Session end (the Session drops its ports after `close` wrote the last record), a refused start, `henki port`'s end:
/// `bye` with up to 300 ms for its answer, then the kill of its group and session, also after a clean exit.
impl Drop for Remote {
    fn drop(&mut self) {
        if self.dead.is_none() && self.child.is_some() {
            let _ = self.call("bye", json!({}));
        }
        supervise::kernel_end(self.child.take());
    }
}

/// The binding's stdout: each answer to MAIN with its length; lines without an id are notifications, which port/1 v1
/// reserves and skips; the first broken line (over `LINE_MAX`, no newline at EOF, not a JSON object) ends the binding.
fn lines(out: impl Read, tx: &Sender<Line>) {
    let (mut r, mut line) = (BufReader::new(out), Vec::new());
    loop {
        line.clear();
        match (&mut r).take(LINE_MAX as u64 + 1).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if !line.ends_with(b"\n") => return drop(tx.send(Err(format!("a line over {} bytes, or cut", term::num(LINE_MAX as u64))))),
            Ok(_) => {}
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(v) if v.get("id").is_some_and(|i| !i.is_null()) => {
                if tx.send(Ok((v, line.len()))).is_err() {
                    return;
                }
            }
            Ok(v) if v.is_object() => {}
            _ => return drop(tx.send(Err("a line that is not a JSON object".into()))),
        }
    }
}

/// The session id in an answer, checked as history/1 allows it.
fn id(v: &Value) -> Result<String, PortError> {
    v["session"].as_str().filter(|s| port::session_id(s)).map(String::from).ok_or_else(|| err("protocol", "no valid session id in the answer"))
}

impl History for Remote {
    fn create(&mut self, scope: &Path, hint: Option<&str>) -> Result<String, PortError> {
        let a = self.call("create", json!({"scope": scope, "hint": hint}))?;
        id(&a).map_err(|e| self.fail(e))
    }

    fn open(&mut self, scope: &Path, session: Option<&str>) -> Result<String, PortError> {
        let got = id(&self.call("open", json!({"scope": scope, "session": session}))?).map_err(|e| self.fail(e))?;
        if session.is_some_and(|s| s != got) {
            return Err(self.fail(err("protocol", format!("open {session:?} answered {got}"))));
        }
        Ok(got)
    }

    /// Page by page: `next`, the index of the next record, as long as it grows; at most READ_MAX bytes in all.
    fn read(&mut self, session: &str) -> Result<Taken, PortError> {
        let (mut records, mut notices, mut from, start) = (Vec::new(), Vec::new(), 0, self.got);
        loop {
            let p = self.call("read", json!({"session": session, "from": from}))?;
            let page = p["records"].as_array().filter(|a| a.iter().all(|r| r["t"].is_string()) && self.got - start <= READ_MAX);
            let Some(page) = page else { return Err(self.fail(err("protocol", "read: records is not a list of records, or over 256 MiB"))) };
            records.extend(page.iter().cloned());
            notices.extend(p["notices"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|n| term::escape(n).into_owned()));
            match p["next"].as_u64() {
                Some(n) if n > from && n == records.len() as u64 => from = n,
                Some(_) => return Err(self.fail(err("protocol", "read: next must be the count of records so far"))),
                None => return Ok(Taken { records, notices, place: format!("{} binding {}: session {session}", self.port, self.name) }),
            }
        }
    }

    fn append(&mut self, session: &str, rec: Record) -> Result<Record, PortError> {
        self.call("append", json!({"session": session, "record": &rec})).map(|_| rec)
    }

    fn discard(&mut self, session: &str) -> Result<bool, PortError> {
        Ok(self.call("discard", json!({"session": session}))?["removed"] == true)
    }
}

/// `henki port PORT [--offer V]... -- PROGRAM` (hidden; CONTRACTS.md documents it): the hello result as the first line,
/// then for each request line on stdin (`{"method": M, "params": P}`, or `{"kill": true}`: SIGKILL, no bye, and the
/// end) one canonical answer line, `{"result": R}` or `{"error": {"code", "message"}}`. The conformance runner of
/// `henki mods test`, the import and export path between history bindings, and a debugging tool. A signal ends it
/// (`interrupted`); a binding that does not start or greet is error `port` (exit 2).
pub fn pipe(port: &'static str, exe: &Path, state: &Path, env: &[(OsString, OsString)], offer: &[u64], out: &Out, (tx, rx): (Sender<Ev>, Receiver<Ev>)) -> Outcome {
    let line = |v: Value| out.answer(&term::json_line(&v));
    let fail = |e: PortError| json!({"error": {"code": e.code, "message": e.message}});
    let cwd = std::env::current_dir().unwrap_or_default();
    let dir = sys::private_dir(state).map_err(|e| err("io", format!("{}: {e}", state.display())));
    let started = dir.and_then(|()| Remote::start(port, exe, (exe.display().to_string(), String::new()), &cwd, env, Stdio::inherit()));
    let mut r = match started.and_then(|mut r| r.hello(&cwd, state, offer).map(|()| r)) {
        Ok(r) => r,
        Err(e) => return (line(fail(e.clone())), Outcome::error("port", e.message)).1,
    };
    line(json!({"result": {"name": r.said, "caps": r.caps}}));
    // stdin on its own thread, so that a signal still ends the wait.
    let read = move || {
        let _ = std::io::stdin().lock().lines().map_while(Result::ok).try_for_each(|l| tx.send(Ev::Input { tag: 0, bytes: Some(l.into_bytes()) }));
        tx.send(Ev::Input { tag: 0, bytes: None })
    };
    let _ = thread::Builder::new().spawn(read);
    while let Ok(Ev::Input { bytes: Some(b), .. }) = rx.recv() {
        let q: Value = serde_json::from_slice(&b).unwrap_or_default();
        if q["kill"] == true {
            return (supervise::kernel_end(r.child.take()), Outcome::Ok).1;
        }
        line(r.call(q["method"].as_str().unwrap_or_default(), q["params"].clone()).map_or_else(fail, |v| json!({"result": v})));
    }
    if sys::seen() > 0 { Outcome::Interrupted } else { Outcome::Ok }
}

#[cfg(test)]
mod tests;
