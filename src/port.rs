//! The ports: the parts without which there is no agent. Each always has a binding, the std one built in to start
//! with. The kernel calls `history` through its trait and checks every answer the same way, whoever gave it; the other
//! ports keep their typed seams (model: `types::Wire`; context: `Session::elide`; exec: `supervise::spawn` and
//! `files::start`; policy: `tools::needs_approval` and the allow rules; frontend: `repl::run` and the headless branch of
//! `main::run`). A trait mirrors its protocol's method set, one method each, and only henki implements it.
use crate::types::Outcome;
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

/// The bindings of one session.
pub struct Ports {
    pub history: Box<dyn History>,
}
