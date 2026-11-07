//! The calls of a step: ids and validation, one call with its timeout and escalation, its result, the stuck rule and the kill path.
use super::{CALL_MAX_S, LATE, SLICE, Session};
use crate::{files, journal::rec, supervise, supervise::GRACE, sys, term, tools, tools::Killed, tools::Target, types::*};
use std::time::{Duration, Instant};

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

    /// One call of the step and its result: its error, or the question `ask` mode puts to a human, or the approved run.
    pub(super) fn dispatch(&mut self, c: &Call) -> Result<(), Outcome> {
        if let Some(e) = &c.error {
            return self.answer(c, e, true, None, None);
        }
        // A file tool's target is resolved before anything is shown or run.
        let cx = tools::Ctx { cwd: &self.cfg.cwd, root: &self.cfg.root, home: &self.cfg.home, protected: &self.cfg.protected };
        let t = match (c.name != "shell").then(|| tools::target(c.args["path"].as_str().unwrap_or_default(), c.name != "read", &cx)) {
            Some(Err(text)) => return self.answer(c, &text, true, None, None),
            t => t.and_then(Result::ok),
        };
        if tools::needs_approval(&c.name, t.as_ref(), self.cfg.auto) {
            self.record(rec::ask(c, t.as_ref().map(|t| t.shown.as_str())))?;
            return Err(Outcome::NeedsApproval(c.clone()));
        }
        self.record(rec::approval(&c.id, "auto", None))?;
        self.call(c, t)
    }

    /// Runs an approved call and waits for its output. A shell call past its timeout gets SIGTERM, then after `GRACE` the
    /// kill of its group and session, then up to `LATE` for its output. These are the ordinary waits: a signal or the
    /// deadline returns with the call still live, and FINISHING kills it.
    fn call(&mut self, c: &Call, t: Option<Target>) -> Result<(), Outcome> {
        let (tag, cap, t0) = (self.next_tag(), tools::cap(self.cfg.output_cap, self.cfg.window), Instant::now());
        let secs = c.args["timeout"].as_u64().unwrap_or(self.cfg.tool_timeout_s).min(CALL_MAX_S);
        let (cmd, mut pgid) = (c.args["command"].as_str().unwrap_or_default(), None);
        if let Some(t) = t {
            files::start(c.clone(), t, cap, tag, &self.tx);
        } else {
            match supervise::spawn(&self.cfg.bash, cmd, &self.cfg.cwd, &self.env, cap, self.j.spill(&self.cfg.home), tag, &self.tx) {
                Ok(p) => pgid = Some(p),
                Err(e) => return self.answer(c, &tools::not_started(&e), true, None, None),
            }
        }
        self.open.iter_mut().filter(|o| o.0.id == c.id).for_each(|o| o.1 = true);
        self.live = Some((tag, c.id.clone()));
        let mut ev = self.wait(tag, pgid.map(|_| t0 + Duration::from_secs(secs)))?;
        let killed = ev.is_none().then_some(Killed::Timeout(secs));
        if let (None, Some(pgid)) = (&ev, pgid) {
            sys::kill(-pgid, libc::SIGTERM);
            ev = self.wait(tag, Some(Instant::now() + GRACE))?;
            supervise::kill_call(pgid);
            ev = if ev.is_none() { self.wait(tag, Some(Instant::now() + LATE))? } else { ev };
        }
        self.live = None;
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

    /// Journals a call's result (`out`: what its worker reported, none when it never ran), prints its plain-mode line,
    /// reaps after a call that ran (noticing when the background count changes) and feeds the stuck rule.
    fn answer(&mut self, c: &Call, text: &str, is_error: bool, out: Option<&ToolOutput>, killed: Option<&Killed>) -> Result<(), Outcome> {
        let (ms, bytes) = out.map_or((0, text.len() as u64), |o| (o.ms, o.bytes));
        self.record(rec::tool_result(&c.id, &c.name, text, is_error, ms, bytes))?;
        self.open.retain(|o| o.0.id != c.id);
        self.out.line(&tools::call_line(c, out, killed, is_error));
        let bg = if out.is_some() { supervise::reap() } else { self.bg };
        if bg != self.bg {
            self.bg = bg;
            self.notice(&format!("{} background processes from earlier calls still run", term::num(bg as u64)))?;
        }
        self.stuck.feed(format!("{}\0{}", c.name, c.raw.clone().unwrap_or_else(|| c.args.to_string())), is_error);
        Ok(())
    }

    /// The kill path of every stop and of the session end: SIGTERM to every live group and up to `GRACE` for them to go
    /// (ended early when all are gone, or by a second signal), then SIGKILL and up to five sweeps of what is left; with
    /// nothing live, no wait at all. Returns the live call's output if it reported, and how many processes there were to
    /// stop (macOS: process groups).
    pub(super) fn stop_all(&mut self) -> (Option<ToolOutput>, usize) {
        let (tag, mut got, n) = (self.live.as_ref().map_or(0, |l| l.0), None, supervise::reap());
        let (mut left, end) = (supervise::signal_groups(libc::SIGTERM), Instant::now() + GRACE);
        while left > 0 && !self.hurry && Instant::now() < end {
            self.hold(tag, end.saturating_duration_since(Instant::now()).min(SLICE), &mut got);
            supervise::reap();
            left = supervise::signal_groups(0);
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
