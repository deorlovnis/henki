//! One session: the step loop of a turn, the only place the main thread waits, and the single way a turn and a session end.
use crate::{config::Config, journal::Journal, term::Out, types::*};
use serde_json::Value;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

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
    // private: cfg (which carries the env snapshot and the cwd), journal, out, hist, channel, deadline, limits, child
    // env, system text, tool schemas, offered names, next_h, tag counter, the request in flight (slot, partial text,
    // opened calls), this step's open calls, the live call, steps, usage (turn and process), last text, stuck state,
    // ctx, bg count, finishing, hurry
}

impl Session {
    /// Takes the child environment from `cfg.env`, `cfg.pass_env`, `cfg.user_path` and `j.id()`; the cwd is `cfg.cwd`.
    pub fn new(cfg: Config, j: Journal, hist: Vec<Value>, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, deadline: Option<Instant>) -> Session {
        todo!()
    }

    /// One turn: the `user` record (none for `run --resume` without TASK), then steps until an outcome. Every call of the
    /// last step has a result and `turn_end` is written before this returns. Never exits the process.
    pub fn turn(&mut self, task: Option<String>) -> Outcome {
        todo!()
    }

    /// Session end: kill and sweep everything henki started (every path, `ok` included), then `result` (when given),
    /// then `close`.
    pub fn close(&mut self, result: Option<Value>) -> Result<(), Outcome> {
        todo!()
    }

    pub fn id(&self) -> &str {
        todo!()
    }

    /// The last assistant text, or "".
    pub fn text(&self) -> &str {
        todo!()
    }

    /// This turn's successful replies.
    pub fn steps(&self) -> u32 {
        todo!()
    }

    /// This process's requests.
    pub fn usage(&self) -> Usage {
        todo!()
    }

    /// For the bounded flush before exit.
    pub fn into_out(self) -> Out {
        todo!()
    }
}
