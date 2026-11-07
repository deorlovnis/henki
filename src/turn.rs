//! One session: the step loop of a turn, the only place the main thread waits, and the single way a turn and a session end.
use crate::{anthropic, config::Config, ext, http, journal, journal::Journal, journal::rec, supervise, sys, term, term::Out, term::Reader};
use crate::{tools, tools::Killed, types::*};
use call::Stuck;
use serde_json::Value;
use std::{convert::Infallible, ffi::OsString, path::Path, path::PathBuf, sync::mpsc::Receiver, sync::mpsc::Sender, time::Duration, time::Instant};

mod approve;
mod call;
mod request;

pub const ATTEMPTS: u32 = 5; // 4 retries
pub const CALL_MAX_S: u64 = 600;
pub const LATE: Duration = Duration::from_secs(1); // a killed call's result, from MAIN if the waiter is silent
pub const STUCK_RUN: u32 = 3;
pub const SLICE: Duration = Duration::from_millis(50); // how often a kill step re-checks ESRCH
// The 1,800 s attempt limit is `http::LIMITS.request`, held in `Session.lim` so a unit test can shorten it.

pub struct Session {
    /// The REPL reads it (the banner, the allow rules) and replaces `cfg.allow` after removing a rule.
    /// Its `snap` is what the tripwire compares with: the trust files as `config::load` found them.
    pub(crate) cfg: Config,
    /// The signal that ended the last wait: INT and QUIT cancel a REPL turn, TERM and HUP end the REPL.
    pub(crate) sig: Option<i32>,
    j: Journal,
    out: Out,
    /// Exactly the `user`, `assistant` and `tool_result` records, in journal order: what the next request renders.
    hist: Vec<Value>,
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    deadline: Option<Instant>,
    lim: http::Limits,
    /// The tool child's environment, built once.
    env: Vec<(OsString, OsString)>,
    system: String,
    tools: Value,
    /// The REPL's line reader: approvals are asked on the terminal. None in `run`, where they end the turn.
    reader: Option<Reader>,
    next_h: u32,
    tag: u32,
    t: Turn,
    usage_proc: Usage,
    /// The last reply's input + output tokens and the size of the request that got it.
    ctx: (u64, usize),
    /// Live background processes after the last call.
    bg: usize,
    /// FINISHING or `close`: `wait` ignores the deadline, and a signal only hurries the kill.
    finishing: bool,
    hurry: bool,
}

/// The turn in progress, new at its start: its counts, its last text, the stuck rule, and what is in flight.
#[derive(Default)]
struct Turn {
    steps: u32,
    usage: Usage,
    text: String,
    stuck: Stuck,
    /// The attempt in flight: its cancel slot, its streamed text and the names of the calls it opened.
    conn: Option<http::Slot>,
    partial: String,
    opened: Vec<String>,
    /// This step's calls that have no result yet, and whether each started.
    open: Vec<(Call, bool)>,
    /// The running call's tag and id: MAIN waits on the tag, and FINISHING gives the call its output.
    live: Option<(u32, String)>,
}

impl Session {
    /// The child environment from `cfg.env`, `cfg.pass_env`, the extensions' `bin/` dirs, `cfg.user_path` and `j.id()`;
    /// the cwd is `cfg.cwd`. `reader`: Some in the REPL (approvals are asked on the terminal), None in `run`.
    pub fn new(cfg: Config, j: Journal, hist: Vec<Value>, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, deadline: Option<Instant>, reader: Option<Reader>) -> Session {
        let env = supervise::child_env(&cfg.env, &cfg.pass_env, &ext::child_path(&cfg.exts, &cfg.user_path), &cfg.user_path, j.id());
        let system = tools::system(cfg.agents.as_deref(), &ext::prompt(&cfg.exts, &cfg.home));
        // The Anthropic API can be silent for minutes while it buffers a large tool input.
        let lim = http::Limits { stall: if cfg.dialect == "anthropic" { anthropic::STALL } else { http::LIMITS.stall }, ..http::LIMITS };
        let (tools, next_h) = (tools::schema(&cfg.tools), journal::next_h(&hist));
        Session {
            cfg,
            sig: None,
            j,
            out,
            hist,
            tx,
            rx,
            deadline,
            lim,
            env,
            system,
            tools,
            reader,
            next_h,
            tag: 0,
            t: Turn::default(),
            usage_proc: Usage::default(),
            ctx: (0, 0),
            bg: 0,
            finishing: false,
            hurry: false,
        }
    }

    /// One turn: the `user` record (none for `run --resume` without TASK), then steps until an outcome. Every call of the
    /// last step has a result and `turn_end` is written before this returns. Never exits the process.
    pub fn turn(&mut self, task: Option<String>) -> Outcome {
        let Err(o) = self.step_loop(task);
        // A background process may have changed a trust file after the last call's check.
        let o = if o.finished() { self.tripwire().err().unwrap_or(o) } else { o };
        self.finish(o)
    }

    /// Session end: kill and sweep everything henki started (every path, `ok` included), then `result` (when given),
    /// then `close`. A journal failure is the `Err`, and the `--json` reader still gets one result.
    pub fn close(&mut self, result: Option<Value>) -> Result<(), Outcome> {
        self.finishing = true;
        let (_, n) = self.stop_all(false);
        self.end(n, result)
    }

    /// The last records: the kill notice, `result`, `close`; then the result on `--json` stdout, or in plain mode the
    /// answer of an `ok` run. After a failed write the stdout result is the given one when it is already an error (that
    /// error stays the run's end), else one for the failure. A REPL session that got no message leaves no file, and its
    /// kill notice goes to the terminal only.
    fn end(&mut self, killed: usize, result: Option<Value>) -> Result<(), Outcome> {
        let stopped = (killed > 0).then(|| format!("stopped {} background processes at exit", term::num(killed as u64)));
        if self.reader.is_some() && !self.hist.iter().any(|r| r["t"] == "user") && self.j.discard() {
            stopped.iter().for_each(|m| self.out.note(m)); // no journal to write it to
            return Ok(());
        }
        let written = (|| {
            stopped.iter().try_for_each(|m| self.notice(m))?;
            let r = result.clone().map(|r| self.j.write(r)).transpose()?;
            self.j.write(rec::close(self.j.bytes())).map(|_| r)
        })();
        let (r, end) = match written {
            Ok(r) => (r, Ok(())),
            Err(e) => {
                let ms = |r: &Value| (r["duration_s"].as_f64().unwrap_or_default() * 1e3) as u64;
                let failed = |r: Value| if r["status"] == "error" { r } else { self.result(&e, ms(&r)) };
                (result.map(failed), Err(e))
            }
        };
        // Each writes in its own mode only: the record on `--json` stdout, the answer in plain mode.
        r.iter().for_each(|r| self.out.record(r));
        if r.is_some_and(|r| r["status"] == "ok") {
            self.out.answer(&self.t.text);
        }
        end
    }

    /// The terminal, for the outcome line.
    pub fn out(&self) -> &Out {
        &self.out
    }

    /// The run's `result` record: the last assistant text (or ""), this turn's successful replies, this process's usage.
    pub fn result(&self, o: &Outcome, duration_ms: u64) -> Value {
        rec::result(o, &self.t.text, self.usage_proc, Some(self.j.id()), self.t.steps, duration_ms)
    }

    /// For the bounded flush before exit.
    pub fn into_out(self) -> Out {
        self.out
    }

    /// One line from the terminal; None at EOF or without a reader. No deadline applies (the REPL has none). Err: a
    /// signal (also in `sig`) or a closed stdout.
    pub fn line(&mut self) -> Result<Option<String>, Stop> {
        let tag = self.next_tag();
        let Some(r) = &self.reader else { return Ok(None) };
        r.ask(tag);
        let got = self.wait(tag, None);
        self.reader.iter().for_each(|r| r.ask(0)); // a line that ends later is stale
        Ok(got?.and_then(|ev| if let Ev::Line { text, .. } = ev { text } else { None }))
    }

    /// `!CMD` on the terminal: its exit code, None when bash did not start (a notice says why). INT and QUIT reach the
    /// command alone (it runs in henki's process group); TERM, HUP and a closed stdout SIGKILL it and return Err.
    pub fn bang(&mut self, cmd: &str) -> Result<Option<i32>, Stop> {
        let (tag, env) = (self.next_tag(), supervise::bang_env(&self.cfg.env));
        let Ok(pid) = supervise::bang(&self.cfg.bash, cmd, &self.cfg.cwd, &env, tag, &self.tx).map_err(|e| self.out.note(&tools::not_started(&e))) else { return Ok(None) };
        let mut got = self.wait(tag, None);
        while let Err(Stop::Signal(libc::SIGINT | libc::SIGQUIT)) = got {
            got = self.wait(tag, None); // the command got it too
        }
        let done = got.inspect_err(|_| _ = sys::kill(pid, libc::SIGKILL))?;
        Ok(done.and_then(|ev| if let Ev::BangDone { code, .. } = ev { Some(code) } else { None }))
    }

    /// Every trust file whose stamps differ from the snapshot, in `cfg.trust` order.
    pub fn changed(&self) -> Vec<PathBuf> {
        self.cfg.snap.iter().filter(|(p, s)| sys::stamp(p) != *s).map(|(p, _)| p.clone()).collect()
    }

    /// A new stamp of the allow file, after henki's own write to it (`always`, `/allow rm`) and right after a `changed()`
    /// that was empty. Every other trust file keeps its stamp, so no outside change is ever absorbed.
    pub fn rearm(&mut self) {
        self.cfg.snap.iter_mut().filter(|(p, _)| *p == self.cfg.allow.file).for_each(|(p, s)| *s = sys::stamp(p));
    }

    /// Requests and calls until an outcome; `Err` is the only way out.
    fn step_loop(&mut self, task: Option<String>) -> Result<Infallible, Outcome> {
        (self.t, self.sig) = (Turn::default(), None);
        // The REPL's banner printed them already.
        std::mem::take(&mut self.cfg.notices).iter().try_for_each(|n| if self.reader.is_some() { self.record(rec::notice(n)) } else { self.notice(n) })?;
        match task {
            // Where and when the model works goes on the session's first task only.
            Some(t) => {
                let head = if self.hist.iter().any(|r| r["t"] == "user") { String::new() } else { head(&self.cfg.cwd) };
                self.record(rec::user(&(head + &t)))?;
            }
            // Only a task or a tool result leaves something for the model to continue. A request may not end with an
            // assistant message, so a reply that was cut short (interrupted or failed) needs a task as well.
            None if !matches!(self.hist.last().and_then(|r| r["t"].as_str()), Some("user" | "tool_result")) => {
                let cut = self.hist.last().is_some_and(|r| r["finish"] == "interrupted" || r["finish"] == "error");
                return Err(Outcome::error("config", if cut { "the last reply was cut short; give a TASK" } else { "nothing to continue; give a TASK" }));
            }
            None => {}
        }
        loop {
            let mut r = self.request()?;
            let calls = self.prepare(std::mem::take(&mut r.calls));
            // A renamed id goes into the replayed content too: each tool_use block is one call, in order.
            r.blocks.iter_mut().flat_map(Value::as_array_mut).flatten().filter(|b| b["type"] == "tool_use").zip(&calls).for_each(|(b, c)| b["id"] = c.id.as_str().into());
            self.record(rec::assistant(&r.text, &calls, &r.finish, r.usage, r.blocks.as_ref()))?;
            (self.t.steps, self.t.text) = (self.t.steps + 1, r.text);
            self.t.usage.add(r.usage);
            self.usage_proc.add(r.usage);
            if calls.is_empty() {
                (r.finish == "length").then(|| self.notice("answer cut at max_tokens")).transpose()?;
                if !self.t.text.trim().is_empty() {
                    return Err(Outcome::Ok);
                }
                self.notice("the model ended without an answer")?;
                return Err(Outcome::GaveUp("empty_answer"));
            }
            self.t.open = calls.iter().map(|c| (c.clone(), false)).collect();
            for c in &calls {
                self.poll()?; // calls that never wait still end at the deadline or a signal
                self.dispatch(c)?;
            }
            let why = if self.t.stuck.run >= STUCK_RUN { Some("stuck") } else { (self.t.steps >= self.cfg.max_steps).then_some("max_steps") };
            why.map_or(Ok(()), |w| Err(Outcome::GaveUp(w)))?;
        }
    }

    /// FINISHING, the only exit of a turn: shut a request in flight, kill everything when the outcome stops the run or a
    /// call is live, give every call of the step a result, then `turn_end`. A journal failure here wins unless the
    /// outcome is already an error. A Ctrl-C that cancels a REPL turn sends the tools SIGINT first, as a shell would.
    fn finish(&mut self, o: Outcome) -> Outcome {
        self.t.conn.take().iter().for_each(http::cancel);
        self.finishing = true;
        let int = self.reader.is_some() && o == Outcome::Interrupted && matches!(self.sig, Some(libc::SIGINT | libc::SIGQUIT));
        let got = if o.stops() || self.t.live.is_some() { self.stop_all(int).0 } else { None };
        let r = self.settle(&o, got);
        // A second signal hurries this kill only; the next turn's cancel gets its grace again.
        (self.finishing, self.hurry) = (false, false);
        r.err().filter(|_| !matches!(o, Outcome::Error { .. })).unwrap_or(o)
    }

    /// The live call keeps its output if it reported in time; another started call gets the repair text, the rest
    /// `not run: REASON`.
    fn settle(&mut self, o: &Outcome, got: Option<ToolOutput>) -> Result<(), Outcome> {
        if !self.t.opened.is_empty() {
            let names = std::mem::take(&mut self.t.opened).join(", ");
            self.notice(&format!("stream ended; dropped calls: {names}"))?;
        }
        let live = self.t.live.take();
        for (c, started) in std::mem::take(&mut self.t.open) {
            let (text, ms, bytes) = match (&got, &live) {
                (Some(out), Some(l)) if l.1 == c.id => (tools::result_text(&c.name, out, Some(&Killed::Interrupted)).0, out.ms, out.bytes),
                _ if started => (tools::REPAIR.to_string(), 0, 0),
                _ => (format!("not run: {}", o.not_run()), 0, 0),
            };
            self.record(rec::tool_result(&c.id, &c.name, &text, true, ms, bytes))?;
        }
        self.record(rec::turn_end(o, self.t.steps, self.t.usage))
    }

    /// The only place MAIN waits in a session. Ok(Some): the tagged event with `tag` (its `Part`s are handled on the
    /// way); Ok(None): `until` passed; Err: a signal (kept in `sig`), the deadline or a closed stdout. Both clocks are
    /// checked before every receive, so a stream of events cannot hold off the deadline. With `finishing` set there is no
    /// deadline, a signal sets `hurry`, and a closed stdout is not an error.
    fn wait(&mut self, tag: u32, until: Option<Instant>) -> Result<Option<Ev>, Stop> {
        let deadline = self.deadline.filter(|_| !self.finishing);
        loop {
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return Err(Stop::Budget);
            }
            if until.is_some_and(|u| now >= u) {
                return Ok(None);
            }
            // The session holds a sender, so the channel never disconnects.
            let ev = if let Some(e) = until.into_iter().chain(deadline).min() { self.rx.recv_timeout(e - now).ok() } else { self.rx.recv().ok() };
            match ev {
                Some(Ev::Signal(s)) => {
                    (self.hurry, self.sig) = (self.hurry || self.finishing, Some(s));
                    return Err(Stop::Signal(s));
                }
                Some(Ev::OutputClosed) if !self.finishing => return Err(Stop::Closed),
                Some(Ev::Part { tag: t, part }) if t == tag => self.part(part),
                Some(ev @ (Ev::Reply { tag: t, .. } | Ev::Tool { tag: t, .. } | Ev::Line { tag: t, .. } | Ev::BangDone { tag: t, .. })) if t == tag => return Ok(Some(ev)),
                _ => {} // stale: an ended attempt, a timer, an answered call, a line nobody waits for
            }
        }
    }

    /// `wait` without waiting, between calls: the deadline, then a signal or a closed stdout already queued. Everything
    /// else queued is stale here and dropped.
    fn poll(&mut self) -> Result<(), Stop> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Stop::Budget);
        }
        let stop = |ev| if let Ev::Signal(s) = ev { Some(Stop::Signal(s)) } else { matches!(ev, Ev::OutputClosed).then_some(Stop::Closed) };
        let found = self.rx.try_iter().find_map(stop);
        self.sig = found.and_then(|f| if let Stop::Signal(s) = f { Some(s) } else { None }).or(self.sig);
        found.map_or(Ok(()), Err)
    }

    /// The journal first, then the `--json` line; only `user`, `assistant` and `tool_result` enter the history.
    fn record(&mut self, rec: Value) -> Result<(), Outcome> {
        let rec = self.j.write(rec)?;
        self.out.record(&rec);
        if matches!(rec["t"].as_str(), Some("user" | "assistant" | "tool_result")) {
            self.hist.push(rec);
        }
        Ok(())
    }

    /// A notice: `henki: TEXT` on stderr (plain and REPL), a record in the journal and on `--json` stdout.
    fn notice(&mut self, text: &str) -> Result<(), Outcome> {
        self.out.note(text);
        self.record(rec::notice(text))
    }

    /// Tags start at 1: one per attempt, backoff timer, call, line and `!cmd`.
    fn next_tag(&mut self) -> u32 {
        self.tag += 1;
        self.tag
    }
}

/// The line for an end other than `ok`: `(gave up: stuck)`, `(error auth: MSG)`, `(needs approval: NAME)`, `(budget)`.
pub fn outcome_line(o: &Outcome) -> String {
    match o {
        Outcome::GaveUp(r) => format!("(gave up: {r})"),
        Outcome::Error { kind, message } => format!("(error {kind}: {message})"),
        Outcome::NeedsApproval(c) => format!("(needs approval: {})", c.name),
        o => format!("({})", o.status().replace('_', " ")),
    }
}

/// What a session's first task starts with: where and when the model works.
fn head(cwd: &Path) -> String {
    format!("cwd: {}\nos: {} {}\ndate: {}\n\n", cwd.display(), std::env::consts::OS, std::env::consts::ARCH, sys::local_date())
}

#[cfg(test)]
mod tests;
