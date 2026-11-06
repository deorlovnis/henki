//! Tool processes: the child environment, the spawn recipe, bounded capture with spill, and the ledger of process groups
//! that henki kills by group and, on Linux, by sweeping every descendant.
use crate::config::Env;
use crate::types::Ev;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub const GRACE: Duration = Duration::from_millis(300);
pub const SPILL_MAX: u64 = 8 << 20;

/// Where a call's full output goes once it passes the cap; `shown` is the path as the marker prints it (`$HOME` as `~`).
pub struct Spill {
    pub path: PathBuf,
    pub shown: String,
}

/// Exactly the child environment: the allowlist, `pass_env`, PATH = `user_path`, HENKI_USER_PATH, HENKI_SESSION,
/// HENKI_VERSION, HENKI_DEPTH + 1. A proxy value with userinfo is dropped.
pub fn child_env(env: &Env, pass_env: &[String], user_path: &str, session: &str) -> Vec<(OsString, OsString)> {
    todo!()
}

/// MAIN thread only. `bash --noprofile --norc -c CMD` in its own session, no tty, stdin /dev/null, one pipe for stdout
/// and stderr. The group is in the ledger before this returns. A capture and a waiter thread send one `Ev::Tool { tag }`.
pub fn spawn(bash: &Path, cmd: &str, cwd: &Path, env: &[(OsString, OsString)], cap: usize, spill: Spill, tag: u32, tx: &Sender<Ev>) -> io::Result<i32> {
    todo!()
}

/// Sends `sig` (0 probes) to every group in the ledger, drops the groups that are gone (ESRCH), returns how many remain.
pub fn signal_groups(sig: i32) -> usize {
    todo!()
}

/// SIGKILL the call's group and, on Linux, every process whose session is the call's shell and their descendants.
pub fn kill_call(pgid: i32) {
    todo!()
}

/// After each call; kills nothing. Reaps zombie children by pid (never a ledger leader); returns the number of live
/// background descendants (macOS: live ledger groups of earlier calls).
pub fn reap() -> usize {
    todo!()
}

/// One sweep pass: reap zombie children by pid, SIGKILL every live descendant of henki and every ledger group; returns
/// how many it signalled (0: nothing is left). macOS: the groups only.
pub fn sweep_pass() -> usize {
    todo!()
}

/// The panic hook's kill: try_lock the ledger, SIGKILL every group, sweep passes without waiting.
pub fn panic_kill() {
    todo!()
}
