//! The ports: the parts without which there is no agent. Each always has a binding, the std one built in to start
//! with. The kernel calls `history` and `policy` through these traits and checks every answer the same way, whoever
//! gave it; the other four ports keep their typed seams (model: `types::Wire`; context: `Session::elide`; exec:
//! `supervise::spawn` and `files::start`; frontend: `repl::run` and the headless branch of `main::run`). Each trait
//! mirrors its protocol's method set, one method each, and only henki implements them.
use crate::{config::Config, tools::Target, types::Call, types::Outcome};
use serde_json::Value;
use std::path::Path;

/// A `journal/1` record: one JSON object with `t`.
pub type Record = Value;

/// A port call's failure: `code` from the contract's list, a message naming the session or the binding.
#[derive(Clone, Debug, PartialEq)]
pub struct PortError {
    pub code: &'static str,
    pub message: String,
}

pub fn err(code: &'static str, message: impl Into<String>) -> PortError {
    PortError { code, message: message.into() }
}

/// A session id as `history/1` allows it: 1 to 64 of A-Z a-z 0-9 . _ -, not starting with `.` or `-`. It names the
/// spill files, HENKI_SESSION and `--resume=ID`.
pub fn session_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && !s.starts_with(['.', '-']) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A history failure as the start's or the turn's end: `tampered` stays `tampered`, a signal is Interrupted, the rest
/// is `journal`.
pub fn outcome(e: PortError) -> Outcome {
    match e.code {
        "interrupted" => Outcome::Interrupted,
        "tampered" => Outcome::error("tampered", e.message),
        _ => Outcome::error("journal", e.message),
    }
}

/// What `History::read` took: the records in order, the binding's own notices (a cut torn tail), and how a message names
/// the session (std: the file's path).
pub struct Taken {
    pub records: Vec<Record>,
    pub notices: Vec<String>,
    pub place: String,
}

/// `history/1`: where sessions are kept. A held session is appended to by this binding alone until it ends; `create`
/// and `open` release the one held before.
pub trait History {
    /// A new held session in `scope` (the cwd); `hint`: the id to use (an import), `exists` when it is taken.
    fn create(&mut self, scope: &Path, hint: Option<&str>) -> Result<String, PortError>;
    /// Holds `session` (looked up in every scope), or the newest session of `scope` that nothing holds: `not_found`, `busy`.
    fn open(&mut self, scope: &Path, session: Option<&str>) -> Result<String, PortError>;
    /// Every record of `session` in order, held or not.
    fn read(&mut self, session: &str) -> Result<Taken, PortError>;
    /// One record to the held session, durable before Ok; the record as kept (std fills `close.bytes` and
    /// `resumed.from_bytes`).
    fn append(&mut self, session: &str, rec: Record) -> Result<Record, PortError>;
    /// Removes the held session this binding created when it got no `user` record; Ok(false) when it is kept.
    fn discard(&mut self, session: &str) -> Result<bool, PortError>;
}

/// The policy port's answer for one call.
pub enum Decision {
    /// Runs without a question: the journal verdict (`auto`, `rule`), the rule's number, and the line shown in place of
    /// the call's head (None: the head, as for auto).
    Allow { verdict: &'static str, rule: Option<usize>, line: Option<String> },
    /// The person decides (`run`: the turn ends needs_approval); `always`: a saved rule may be offered.
    Ask { always: bool },
}

/// `policy/1`: who may run a call. Asked after the kernel's floor: validation, the target and its protected-write
/// refusal, the tripwire.
pub trait Policy {
    fn decide(&mut self, c: &Call, t: Option<&Target>, cfg: &Config, session: &str) -> Decision;
    /// The person confirmed `always` for `c`: Ok((the line shown, the rule's number)); Err: why nothing was kept.
    fn record(&mut self, c: &Call, cfg: &mut Config) -> Result<(String, Option<usize>), String>;
}

/// The bindings of one session.
pub struct Ports {
    pub history: Box<dyn History>,
    pub policy: Box<dyn Policy>,
}

// The other ports have one binding each, so they stay seams until a second one exists: a trait with one impl costs
// lines and buys nothing (measured: the three shells below, with their std impls, cost 34 lines). Their shapes are
// fixed, so opening one reshapes nothing:
// - context: `trait Context { fn fit(&mut self, hist: &[Record], budget: &Budget) -> Result<Fit, PortError>; }` with
//   `Budget { window, used, overflowed }` and `Fit { records, journal }`. The kernel keeps the history unmutated (today
//   `journal::elide` mutates it), checks the pairing and the budget of what comes back, and falls back to the std
//   elision with a notice. The one call site is `Session::elide`.
// - exec: data, not a trait: the realpath of `CONFIG/port/exec`, put in front of `bash --noprofile --norc -c CMD` inside
//   `supervise::spawn`, so the child stays in its own session, in the ledger, killed and swept. `spawn` takes eight
//   arguments already: bundle bash, cwd, env and the prefix into one struct first.
// - frontend: `enum Front { Repl, Headless(Option<String>), Serve }`; the frontends stay built in, and `Serve` reads a
//   command stream and writes the `--json` records. Once a command can hand back a restart, `repl::run` returns the
//   exit code and the argv to exec.
