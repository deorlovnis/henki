//! One session: the step loop of a turn, the only place the main thread waits, and the single way a turn and a session end.
use crate::{config::Config, http, journal, journal::Journal, journal::rec, supervise, term, term::Out, tools, tools::Killed, types::*};
use call::Stuck;
use serde_json::Value;
use std::{convert::Infallible, ffi::OsString, path::Path, sync::mpsc::Receiver, sync::mpsc::Sender, time::Duration, time::Instant};

mod call;
mod request;

pub const ATTEMPTS: u32 = 5; // 4 retries
pub const CALL_MAX_S: u64 = 600;
pub const LATE: Duration = Duration::from_secs(1); // a killed call's result, from MAIN if the waiter is silent
pub const STUCK_RUN: u32 = 3;
pub const SLICE: Duration = Duration::from_millis(50); // how often a kill step re-checks ESRCH
pub const REASONING_TAIL: usize = 500;
// The 1,800 s attempt limit is `http::LIMITS.request`, held in `Session.lim` so a unit test can shorten it.

pub struct Session {
    cfg: Config,
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
    next_h: u32,
    tag: u32,
    /// The attempt in flight: its cancel slot, its streamed text and the names of the calls it opened.
    conn: Option<http::Slot>,
    partial: String,
    opened: Vec<String>,
    /// This step's calls that have no result yet, and whether each started.
    open: Vec<(Call, bool)>,
    /// The running call's tag and id: MAIN waits on the tag, and FINISHING gives the call its output.
    live: Option<(u32, String)>,
    steps: u32,
    usage_turn: Usage,
    usage_proc: Usage,
    text: String,
    stuck: Stuck,
    /// The last reply's input + output tokens and the size of the request that got it.
    ctx: (u64, usize),
    /// Live background processes after the last call.
    bg: usize,
    /// FINISHING or `close`: `wait` ignores the deadline, and a signal only hurries the kill.
    finishing: bool,
    hurry: bool,
}

impl Session {
    /// Takes the child environment from `cfg.env`, `cfg.pass_env`, `cfg.user_path` and `j.id()`; the cwd is `cfg.cwd`.
    pub fn new(cfg: Config, j: Journal, hist: Vec<Value>, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, deadline: Option<Instant>) -> Session {
        let env = supervise::child_env(&cfg.env, &cfg.pass_env, &cfg.user_path, j.id());
        let (system, tools, next_h) = (tools::system(cfg.agents.as_deref()), tools::schema(&cfg.tools), journal::next_h(&hist));
        Session {
            cfg,
            j,
            out,
            hist,
            tx,
            rx,
            deadline,
            lim: http::LIMITS,
            env,
            system,
            tools,
            next_h,
            tag: 0,
            conn: None,
            partial: String::new(),
            opened: Vec::new(),
            open: Vec::new(),
            live: None,
            steps: 0,
            usage_turn: Usage::default(),
            usage_proc: Usage::default(),
            text: String::new(),
            stuck: Stuck::default(),
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
        self.finish(o)
    }

    /// Session end: kill and sweep everything henki started (every path, `ok` included), then `result` (when given),
    /// then `close`. A journal failure is the `Err`, and the `--json` reader still gets one result.
    pub fn close(&mut self, result: Option<Value>) -> Result<(), Outcome> {
        self.finishing = true;
        let (_, n) = self.stop_all();
        self.end(n, result)
    }

    /// The last records: the kill notice, `result`, `close`; then the result on `--json` stdout, or in plain mode the
    /// answer of an `ok` run. After a failed write the stdout result is the given one when it is already an error (that
    /// error stays the run's end), else one for the failure.
    fn end(&mut self, killed: usize, result: Option<Value>) -> Result<(), Outcome> {
        let written = (|| {
            if killed > 0 {
                self.notice(&format!("stopped {} background processes at exit", term::num(killed as u64)))?;
            }
            let r = result.clone().map(|r| self.j.write(r)).transpose()?;
            self.j.write(rec::close(self.j.bytes())).map(|_| r)
        })();
        let (r, end) = match written {
            Ok(r) => (r, Ok(())),
            Err(e) => {
                let ms = |r: &Value| (r["duration_s"].as_f64().unwrap_or_default() * 1e3) as u64;
                let failed = |r: Value| if r["status"] == "error" { r } else { rec::result(&e, &self.text, self.usage_proc, Some(self.j.id()), self.steps, ms(&r)) };
                (result.map(failed), Err(e))
            }
        };
        if let Some(r) = r {
            if self.out.json() {
                self.out.record(&r);
            } else if r["status"] == "ok" {
                self.out.answer(&self.text);
            }
        }
        end
    }

    pub fn id(&self) -> &str {
        self.j.id()
    }

    /// The terminal, for `main`'s outcome line.
    pub fn out(&self) -> &Out {
        &self.out
    }

    /// The last assistant text, or "".
    pub fn text(&self) -> &str {
        &self.text
    }

    /// This turn's successful replies.
    pub fn steps(&self) -> u32 {
        self.steps
    }

    /// This process's requests.
    pub fn usage(&self) -> Usage {
        self.usage_proc
    }

    /// For the bounded flush before exit.
    pub fn into_out(self) -> Out {
        self.out
    }

    /// Requests and calls until an outcome; `Err` is the only way out.
    fn step_loop(&mut self, task: Option<String>) -> Result<Infallible, Outcome> {
        (self.steps, self.usage_turn, self.stuck) = (0, Usage::default(), Stuck::default());
        for n in std::mem::take(&mut self.cfg.notices) {
            self.notice(&n)?;
        }
        match task {
            Some(t) => self.record(rec::user(&(head(&self.cfg.cwd) + &t)))?,
            // Only a task or a tool result leaves something for the model to continue.
            None if !matches!(self.hist.last().and_then(|r| r["t"].as_str()), Some("user" | "tool_result")) => {
                return Err(Outcome::error("config", "nothing to continue; give a TASK"));
            }
            None => {}
        }
        loop {
            let mut r = self.request()?;
            let calls = self.prepare(std::mem::take(&mut r.calls));
            self.record(rec::assistant(&r.text, &calls, &r.finish, r.usage))?;
            (self.steps, self.text) = (self.steps + 1, r.text);
            self.usage_turn.add(r.usage);
            self.usage_proc.add(r.usage);
            if calls.is_empty() {
                if r.finish == "length" {
                    self.notice("answer cut at max_tokens")?;
                }
                if !self.text.trim().is_empty() {
                    return Err(Outcome::Ok);
                }
                let why = tail(r.reasoning.trim_end(), REASONING_TAIL);
                let why = if why.is_empty() { String::new() } else { format!("; its reasoning ended: {why}") };
                self.notice(&format!("the model ended without an answer{why}"))?;
                return Err(Outcome::GaveUp("empty_answer"));
            }
            self.open = calls.iter().map(|c| (c.clone(), false)).collect();
            for c in &calls {
                self.poll()?; // calls that never wait still end at the deadline or a signal
                self.dispatch(c)?;
            }
            if self.stuck.run >= STUCK_RUN {
                return Err(Outcome::GaveUp("stuck"));
            }
            if self.steps >= self.cfg.max_steps {
                return Err(Outcome::GaveUp("max_steps"));
            }
        }
    }

    /// FINISHING, the only exit of a turn: shut a request in flight, kill everything when the outcome stops the run or a
    /// call is live, give every call of the step a result, then `turn_end`. A journal failure here wins unless the
    /// outcome is already an error.
    fn finish(&mut self, o: Outcome) -> Outcome {
        if let Some(slot) = self.conn.take() {
            http::cancel(&slot);
        }
        self.finishing = true;
        let got = if o.stops() || self.live.is_some() { self.stop_all().0 } else { None };
        let r = self.settle(&o, got);
        self.finishing = false;
        match r {
            Err(e) if !matches!(o, Outcome::Error { .. }) => e,
            _ => o,
        }
    }

    /// The live call keeps its output if it reported in time; another started call gets the repair text, the rest
    /// `not run: REASON`.
    fn settle(&mut self, o: &Outcome, got: Option<ToolOutput>) -> Result<(), Outcome> {
        if !self.opened.is_empty() {
            let names = std::mem::take(&mut self.opened).join(", ");
            self.notice(&format!("stream ended; dropped calls: {names}"))?;
        }
        let live = self.live.take();
        for (c, started) in std::mem::take(&mut self.open) {
            let (text, ms, bytes) = match (&got, &live) {
                (Some(out), Some(l)) if l.1 == c.id => (tools::result_text(&c.name, out, Some(&Killed::Interrupted)).0, out.ms, out.bytes),
                _ if started => (tools::REPAIR.to_string(), 0, 0),
                _ => (format!("not run: {}", o.not_run()), 0, 0),
            };
            self.record(rec::tool_result(&c.id, &c.name, &text, true, ms, bytes))?;
        }
        self.record(rec::turn_end(o, self.steps, self.usage_turn))
    }

    /// The only place MAIN waits in a session. Ok(Some): the `Reply` or `Tool` with `tag` (its `Part`s are handled on the
    /// way); Ok(None): `until` passed; Err: a signal, the deadline or a closed stdout. Both clocks are checked before
    /// every receive, so a stream of events cannot hold off the deadline. With `finishing` set there is no deadline, a
    /// signal sets `hurry`, and a closed stdout is not an error.
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
            let ev = match until.into_iter().chain(deadline).min() {
                Some(e) => self.rx.recv_timeout(e - now).ok(),
                None => self.rx.recv().ok(),
            };
            match ev {
                Some(Ev::Signal(s)) => {
                    self.hurry |= self.finishing;
                    return Err(Stop::Signal(s));
                }
                Some(Ev::OutputClosed) if !self.finishing => return Err(Stop::Closed),
                Some(Ev::Part { tag: t, part }) if t == tag => self.part(part),
                Some(ev @ (Ev::Reply { tag: t, .. } | Ev::Tool { tag: t, .. })) if t == tag => return Ok(Some(ev)),
                _ => {} // stale: an ended attempt, a timer, an answered call
            }
        }
    }

    /// `wait` without waiting, between calls: the deadline, then a signal or a closed stdout already queued. Everything
    /// else queued is stale here and dropped.
    fn poll(&mut self) -> Result<(), Stop> {
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Stop::Budget);
        }
        let stop = |ev| match ev {
            Ev::Signal(s) => Some(Stop::Signal(s)),
            Ev::OutputClosed => Some(Stop::Closed),
            _ => None,
        };
        self.rx.try_iter().find_map(stop).map_or(Ok(()), Err)
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

    /// A notice: stderr in plain mode, a record in the journal and on `--json` stdout.
    fn notice(&mut self, text: &str) -> Result<(), Outcome> {
        self.out.note(text);
        self.record(rec::notice(text))
    }

    /// Tags start at 1: one per attempt, backoff timer and call.
    fn next_tag(&mut self) -> u32 {
        self.tag += 1;
        self.tag
    }
}

/// What every task starts with: where and when the model works.
fn head(cwd: &Path) -> String {
    format!("cwd: {}\nos: {} {}\ndate: {}\n\n", cwd.display(), std::env::consts::OS, std::env::consts::ARCH, crate::sys::local_date())
}

/// The last `n` bytes of `s` at most, starting on a character.
fn tail(s: &str, n: usize) -> &str {
    let mut i = s.len().saturating_sub(n);
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s.get(i..).unwrap_or_default()
}

#[cfg(test)]
mod tests;
