//! The session journal: one JSON record per line, appended by the main thread alone, byte-counted and synced before the
//! effect it explains; resume reads it back, repairs dangling calls by appending, and checks the call/result pairing.
use crate::config::{Config, Env};
use crate::supervise::Spill;
use crate::types::Outcome;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Journal {
    // private: file, path, bytes, id, spill dir, next spill n
}

pub struct Resumed {
    pub journal: Journal,
    pub hist: Vec<Value>,
    pub from_bytes: u64,
    pub notices: Vec<String>,
}

/// `$XDG_STATE_HOME/henki` (default `~/.local/state/henki`), with `sessions/` and `spill/`: `sys::private_dir` for each;
/// spill files older than 24 h are deleted.
pub fn state_dir(env: &Env, home: &Path) -> Result<PathBuf, String> {
    todo!()
}

impl Journal {
    /// `sessions/<fnv(realpath(cwd))>/<id>.jsonl`: `sys::create_excl(path, 0o600, true)` (O_APPEND), `try_lock` held
    /// for the session.
    pub fn create(state: &Path, cwd: &Path) -> Result<Journal, Outcome> {
        todo!()
    }

    /// Resume through one `sys::open_append` fd: pick, open, lock, read, truncate a torn tail (`set_len`), parse, check,
    /// rebuild, repair (appended through the same O_APPEND fd, synced).
    pub fn resume(state: &Path, cwd: &Path, id: Option<&str>, cfg: &Config) -> Result<Resumed, Outcome> {
        todo!()
    }

    /// Adds `ts`; checks fstat size = bytes written (else `tampered`); one write(2) of `term::json_line` + "\n" on the
    /// O_APPEND fd; fdatasync unless the kind is `ask`, `approval` or `notice`. Returns the record as written.
    pub fn write(&mut self, rec: Value) -> Result<Value, Outcome> {
        todo!()
    }

    pub fn id(&self) -> &str {
        todo!()
    }

    pub fn bytes(&self) -> u64 {
        todo!()
    }

    /// The next spill name, `<id>-<n>.log` (n from the counter, never from a model string).
    pub fn spill(&mut self, home: &Path) -> Spill {
        todo!()
    }
}

/// Over `user`, `assistant` and `tool_result` records only (every other kind is skipped, so this also checks a whole
/// journal file): call ids unique; each call has exactly one result after its assistant record and before the next
/// `assistant` or `user` record; no result without an open call.
pub fn pairing(hist: &[Value]) -> Result<(), String> {
    todo!()
}

/// Every tool_result older than the last two assistant records and not yet elided gets `[output elided: N bytes]`;
/// returns their ids (empty: nothing to elide).
pub fn elide(hist: &mut [Value]) -> Vec<String> {
    todo!()
}

/// 1 + the highest `h<N>` call id in `hist` (1 when none).
pub fn next_h(hist: &[Value]) -> u32 {
    todo!()
}

/// One builder per record kind; each sets "t"; `write` adds "ts".
pub mod rec {
    use crate::config::Config;
    use crate::types::{Call, Outcome, Usage};
    use serde_json::Value;
    use std::path::Path;

    pub fn session(cfg: &Config, id: &str, cwd: &Path, resumed: Option<u64>) -> Value {
        todo!()
    }

    pub fn user(content: &str) -> Value {
        todo!()
    }

    /// "text" | "reasoning"; stdout only.
    pub fn delta(reasoning: bool, text: &str) -> Value {
        todo!()
    }

    pub fn assistant(text: &str, calls: &[Call], finish: &str, usage: Usage) -> Value {
        todo!()
    }

    pub fn ask(c: &Call, target: Option<&str>) -> Value {
        todo!()
    }

    pub fn approval(id: &str, verdict: &str) -> Value {
        todo!()
    }

    pub fn tool_result(id: &str, name: &str, content: &str, is_error: bool, ms: u64, bytes: u64) -> Value {
        todo!()
    }

    pub fn notice(text: &str) -> Value {
        todo!()
    }

    pub fn elide(ids: &[String]) -> Value {
        todo!()
    }

    pub fn turn_end(o: &Outcome, steps: u32, usage: Usage) -> Value {
        todo!()
    }

    pub fn result(o: &Outcome, text: &str, usage: Usage, session: Option<&str>, steps: u32, duration_ms: u64) -> Value {
        todo!()
    }

    pub fn close(bytes: u64) -> Value {
        todo!()
    }
}
