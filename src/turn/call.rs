//! The calls of a step: ids and validation, who approves each (auto, an allow rule, the user), one call with its timeout
//! and escalation, its result, the stuck rule and the kill path.
use super::{CALL_MAX_S, LATE, SLICE, Session, approve::DENIED};
use crate::{files, journal, journal::rec, port::Decision, supervise, supervise::GRACE, sys, term, tools, tools::Killed, tools::Target, types::*};
use serde_json::json;
use std::{time::Duration, time::Instant};

/// A call extends the run when it repeats the previous call (name and arguments) and both failed; another failure
/// starts a run of 1, a success ends it. Identical successes never count.
#[derive(Default)]
pub(super) struct Stuck {
    pub run: u32,
    last: Option<String>, // the previous call's key, when it failed
}

impl Stuck {
    pub(super) fn feed(&mut self, key: String, failed: bool) {
        self.run = if failed && self.last.as_ref() == Some(&key) { self.run + 1 } else { u32::from(failed) };
        self.last = failed.then_some(key);
    }
}

impl Session {
    /// The reply's calls as they are journaled: `h<N>` for a missing or already used id, then validation.
    pub(super) fn prepare(&mut self, mut calls: Vec<Call>) -> Vec<Call> {
        let ids = self.hist.iter().filter_map(|h| h["msg"]["tool_calls"].as_array()).flatten();
        let mut used: Vec<String> = ids.filter_map(|c| c["id"].as_str().map(String::from)).collect();
        for c in &mut calls {
            while c.id.is_empty() || used.contains(&c.id) {
                c.id = format!("h{}", self.next_h);
                self.next_h = self.next_h.wrapping_add(1); // past u32::MAX it starts again at h0; `used` keeps ids unique
            }
            used.push(c.id.clone());
            tools::validate(c, &self.cfg.tools);
        }
        calls
    }

    /// One call of the step and its result: its error; else the tripwire, then who approves it (the policy binding
    /// allows it, or the user decides: `run` ends the turn, the REPL asks), `call_start` with the verdict, the tripwire
    /// right before the run, the run, and the tripwire again.
    pub(super) fn dispatch(&mut self, c: &Call) -> Result<(), Outcome> {
        if let Some(e) = &c.error {
            return self.answer(c, e, true, None, None);
        }
        // A file tool's target is resolved before anything is shown or run.
        let t = match (c.name != "shell").then(|| tools::target(c.args["path"].as_str().unwrap_or_default(), c.name != "read", &self.cfg)) {
            Some(Err(text)) => return self.answer(c, &text, true, None, None),
            t => t.and_then(Result::ok),
        };
        self.tripwire()?;
        let (verdict, rule, head) = match self.ports.policy.decide(c, t.as_ref(), &self.cfg, &self.sid) {
            Decision::Allow { verdict, rule, line } => {
                line.iter().for_each(|l| self.out.line(l));
                self.record(rec::approval(&c.id, verdict, rule))?;
                (verdict, rule, line.is_none())
            }
            Decision::Ask { .. } if self.reader.is_none() => {
                self.record(rec::ask(c, t.as_ref().map(|t| t.shown.as_str())))?;
                return Err(Outcome::NeedsApproval(c.clone()));
            }
            Decision::Ask { always } => match self.ask(c, t.as_ref(), always)? {
                Some((verdict, rule)) => (verdict, rule, true),
                None => return self.answer(c, DENIED, true, None, None),
            },
        };
        self.observe("call_start", &json!({"call": {"args": c.args, "id": c.id, "name": c.name}, "rule": rule, "target": t.as_ref().map(|t| &t.path), "verdict": verdict}))?;
        // Again right before the call starts: the question, and a `call_start.wait` observer, can take time.
        self.tripwire()?;
        self.call(c, t, head)?;
        self.tripwire()
    }

    /// The tripwire: a changed trust file ends the turn, naming the first.
    pub(super) fn tripwire(&mut self) -> Result<(), Outcome> {
        let Some(p) = self.changed().into_iter().next() else { return Ok(()) };
        let m = format!("trust file changed: {}", p.display());
        self.notice(&m).and(Err(Outcome::error("changed", m)))
    }

    /// Runs an approved call and waits for its output; the REPL shows its head line as it starts (a rule's line is one).
    /// A shell call past its timeout gets SIGTERM, then after `GRACE` the kill of its group and session, then up to `LATE`
    /// for its output. These are the ordinary waits: a signal or the deadline returns with the call still live, and
    /// FINISHING kills it.
    fn call(&mut self, c: &Call, t: Option<Target>, head: bool) -> Result<(), Outcome> {
        let (tag, cap, t0) = (self.next_tag(), tools::cap(self.cfg.output_cap, self.cfg.window), Instant::now());
        let secs = c.args["timeout"].as_u64().unwrap_or(self.cfg.tool_timeout_s).min(CALL_MAX_S);
        let (cmd, mut pgid) = (c.args["command"].as_str().unwrap_or_default(), None);
        (head && self.out.repl()).then(|| self.out.line(&tools::head(c)));
        if let Some(t) = t {
            files::start(c.clone(), t, cap, tag, &self.tx);
        } else {
            let spill = journal::spill(&self.cfg.state, &self.sid, &mut self.spill_n, &self.cfg.home);
            match supervise::spawn(&self.cfg.bash, cmd, &self.cfg.cwd, &self.env, cap, spill, tag, &self.tx) {
                Ok(p) => pgid = Some(p),
                Err(e) => return self.answer(c, &tools::not_started(&e), true, None, None),
            }
        }
        self.t.open.iter_mut().filter(|o| o.0.id == c.id).for_each(|o| o.1 = true);
        self.t.live = Some((tag, c.id.clone()));
        let mut ev = self.wait(tag, pgid.map(|_| t0 + Duration::from_secs(secs)))?;
        let killed = ev.is_none().then_some(Killed::Timeout(secs));
        if let (None, Some(pgid)) = (&ev, pgid) {
            sys::kill(-pgid, libc::SIGTERM);
            ev = self.wait(tag, Some(Instant::now() + GRACE))?;
            supervise::kill_call(pgid);
            ev = if ev.is_none() { self.wait(tag, Some(Instant::now() + LATE))? } else { ev };
        }
        self.t.live = None;
        match ev {
            Some(Ev::Tool { out, .. }) => {
                let (text, is_error) = tools::result_text(&c.name, &out, killed.as_ref());
                self.answer(c, &text, is_error, Some(&out), killed.as_ref())
            }
            _ => {
                let out = ToolOutput { text: String::new(), bytes: 0, ms: t0.elapsed().as_millis() as u64, end: End::Signal(libc::SIGKILL) };
                self.answer(c, &format!("[timed out after {secs} s; process group killed]"), true, Some(&out), killed.as_ref())
            }
        }
    }

    /// Journals a call's result (`out`: what its worker reported, none when it never ran), prints its call line, reaps
    /// after a call that ran (noticing when the background count changes) and feeds the stuck rule, which a denied call
    /// resets.
    pub(super) fn answer(&mut self, c: &Call, text: &str, is_error: bool, out: Option<&ToolOutput>, killed: Option<&Killed>) -> Result<(), Outcome> {
        let (ms, bytes) = out.map_or((0, text.len() as u64), |o| (o.ms, o.bytes));
        self.record(rec::tool_result(&c.id, &c.name, text, is_error, ms, bytes))?;
        self.t.open.retain(|o| o.0.id != c.id);
        self.out.line(&tools::call_line(c, out, killed, is_error));
        let bg = if out.is_some() { supervise::reap() } else { self.bg };
        if bg != self.bg {
            self.bg = bg;
            self.notice(&format!("{} background processes from earlier calls still run", term::num(bg as u64)))?;
        }
        self.t.stuck.feed(format!("{}\0{}", c.name, c.raw.clone().unwrap_or_else(|| c.args.to_string())), is_error && text != DENIED);
        Ok(())
    }

    /// The kill path of every stop and of the session end: SIGINT first when `int`, then SIGTERM, each to every live group
    /// with up to `GRACE` for them to go (ended early when all are gone, or by a second signal), then SIGKILL and up to
    /// five sweeps of what is left; with nothing live, no wait at all. Returns the live call's output if it reported, and
    /// how many processes there were to stop (macOS: process groups).
    pub(super) fn stop_all(&mut self, int: bool) -> (Option<ToolOutput>, usize) {
        let (tag, mut got, n) = (self.t.live.as_ref().map_or(0, |l| l.0), None, supervise::reap());
        for sig in [libc::SIGINT, libc::SIGTERM].into_iter().skip(usize::from(!int)) {
            let (mut left, end) = (supervise::signal_groups(sig), Instant::now() + GRACE);
            while left > 0 && !self.hurry && Instant::now() < end {
                self.hold(tag, end.saturating_duration_since(Instant::now()).min(SLICE), &mut got);
                supervise::reap();
                left = supervise::signal_groups(0);
            }
        }
        let mut swept = supervise::signal_groups(libc::SIGKILL);
        for _ in 0..5 {
            self.hold(tag, Duration::from_millis(if swept > 0 { 10 } else { 0 }), &mut got);
            swept = supervise::sweep_pass();
            if swept == 0 {
                break;
            }
        }
        if tag != 0 && got.is_none() && !self.hurry {
            self.hold(tag, GRACE + Duration::from_millis(100), &mut got);
        }
        let _ = (swept > 0).then(|| self.notice(&format!("{} processes survived the kill sweep", term::num(swept as u64))));
        (got, n)
    }

    /// Waits up to `d` (finishing: no deadline), keeping the live call's output if it comes.
    fn hold(&mut self, tag: u32, d: Duration, got: &mut Option<ToolOutput>) {
        if let Ok(Some(Ev::Tool { out, .. })) = self.wait(tag, Some(Instant::now() + d)) {
            *got = Some(out);
        }
    }
}
