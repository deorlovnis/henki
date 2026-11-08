//! The ports: the parts without which there is no agent. Each always has one binding: the std one built in, or a program
//! that `CONFIG/port/NAME` links to (written by `henki mods swap`, never by henki). The kernel calls `history` and `policy` through these traits and checks every answer the same way, whoever
//! gave it; the other four ports keep their typed seams (model: `types::Wire`; context: `Session::elide`; exec:
//! `supervise::spawn` and `files::start`; frontend: `repl::run` and the headless branch of `main::run`). Each trait
//! mirrors its protocol's method set (`port/1`, `history/1` in CONTRACTS.md), one method each, and only henki
//! implements them.
use crate::{allow, allow::Std, config, config::Config, journal::Journal, remote::Remote, supervise, sys, tools::Target, types::Call, types::Outcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::{fs, os::unix::ffi::OsStrExt, os::unix::fs::MetadataExt, process::Stdio, time::Instant};

/// A `journal/1` record: one JSON object with `t`.
pub type Record = Value;

/// What `--port NAME=std` accepts (all six, so a script stays valid as more ports open), then what a link can bind now.
pub const PORTS: [&str; 6] = ["model", "history", "context", "exec", "policy", "frontend"];
pub const LINKED: [&str; 1] = ["history"];
/// The codes a binding may send; henki itself adds timeout, crashed, protocol, interrupted and budget.
const CODES: [&str; 10] = ["invalid", "unsupported", "io", "not_found", "busy", "exists", "tampered", "corrupt", "version", "other"];

/// A port call's failure: `code` from the contract's list (an unknown one arrives as `other`), a message naming the
/// session or the binding.
#[derive(Clone, Debug, PartialEq)]
pub struct PortError {
    pub code: &'static str,
    pub message: String,
}

pub fn err(code: &'static str, message: impl Into<String>) -> PortError {
    PortError { code, message: message.into() }
}

/// A code as a binding sent it, as one of `CODES`.
pub fn code(c: &str) -> &'static str {
    CODES.iter().find(|k| **k == c).copied().unwrap_or("other")
}

/// A session id as `history/1` allows it: 1 to 64 of A-Z a-z 0-9 . _ -, not starting with `.` or `-`. It names the
/// spill files, HENKI_SESSION and `--resume=ID`.
pub fn session_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && !s.starts_with(['.', '-']) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// A history failure as the start's or the turn's end: `tampered` stays `tampered`, a signal is Interrupted, the
/// session deadline Budget, the rest `journal`.
pub fn outcome(e: PortError) -> Outcome {
    match e.code {
        "interrupted" => Outcome::Interrupted,
        "budget" => Outcome::Budget,
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

/// The bindings of one session; `bound`: (port, display name) of each one that is not std, for the `session` record.
/// Dropped after the session's last record: each outside binding gets `bye`, then the kill.
pub struct Ports {
    pub history: Box<dyn History>,
    pub policy: Box<dyn Policy>,
    pub bound: Vec<(&'static str, String)>,
}

/// The links in `CONFIG/port/` that `--port NAME=std` (in `std`) does not override, before any check: (port, the
/// program's realpath, its display name). Fail closed: an entry for a port this henki cannot bind, a link that names
/// nothing runnable, or a program (or its directory) another user can change stops the start. Dot entries are the
/// manager's temporary names.
pub fn links(dir: &Path, std: &[String], home: &Path) -> Result<Vec<(&'static str, PathBuf, String)>, String> {
    let (d, mut out) = (dir.join("port"), Vec::new());
    for n in fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.file_name()).filter(|n| !n.as_bytes().starts_with(b".")) {
        let at = d.join(&n);
        if std.iter().any(|s| n == **s) {
            continue;
        }
        let Some(p) = LINKED.into_iter().find(|p| *p == n) else {
            return Err(format!("{}: this henki cannot bind that port; fix: henki mods swap {} std, or upgrade henki", at.display(), n.to_string_lossy()));
        };
        let fix = format!("fix: henki mods swap {p} std, or start with --port {p}=std");
        let exe = fs::canonicalize(&at).ok().filter(|e| fs::metadata(e).is_ok_and(|m| m.is_file() && m.mode() & 0o100 != 0));
        let exe = exe.ok_or_else(|| format!("{}: not a link to a program; {fix}", at.display()))?;
        for (f, is_dir) in [(exe.as_path(), false), (exe.parent().unwrap_or(Path::new("/")), true)] {
            let m = fs::metadata(f).map_err(|e| format!("{}: {e}; {fix}", f.display()))?;
            config::owner_mode(f, m.mode(), m.uid(), sys::euid(), is_dir).map_err(|e| format!("{e}, or henki mods swap {p} std"))?;
        }
        // `DATA/henki/mods/PKG/ID/port/PORT` shows as `PKG ID`; anything else as its path.
        let tail: Vec<_> = exe.iter().rev().take(5).map(|c| c.to_string_lossy()).collect();
        let name = if tail.len() == 5 && tail[1] == "port" && tail[4] == "mods" { format!("{} {}", tail[3], tail[2]) } else { allow::shown(&exe, home) };
        out.push((p, exe, name));
    }
    Ok(out)
}

/// Binds every port for this session: std, or the program `cfg.links` names, started and greeted. A binding that does
/// not start or answer `hello` stops the start with the repair; each bound one becomes a notice (the REPL banner shows
/// it, `run` journals it). Started before any session record; on an error the ones already started drop (bye, kill),
/// so a refused start leaves no kernel child.
pub fn bind(cfg: &mut Config, deadline: Option<Instant>) -> Result<Ports, Outcome> {
    let mut ports = Ports { history: Box::new(Journal::new(&cfg.state)), policy: Box::new(Std), bound: Vec::new() };
    let (env, lp) = (supervise::child_env(&cfg.env, &cfg.pass_env, &cfg.user_path, &cfg.user_path, ""), kernel_log(cfg));
    let bad = |e: String| Outcome::error("config", e);
    for (p, exe, name) in cfg.links.clone() {
        let log = sys::create_excl(&lp, 0o600, true).or_else(|_| sys::open_append(&lp)).map_err(|e| bad(format!("{}: {e}", lp.display())))?;
        let state = cfg.state.join("port").join(p);
        sys::private_dir(&state).map_err(|e| bad(format!("{}: {e}", state.display())))?;
        let fix = format!("log {}; fix: henki --port {p}=std for this session, or henki mods swap {p} std", allow::shown(&lp, &cfg.home));
        let started = Remote::start(p, &exe, (name.clone(), fix), &cfg.cwd, &env, Stdio::from(log));
        // The session deadline ends a slow hello as a signal does.
        let greeted = started.and_then(|mut r| (r.deadline = deadline, r.hello(&cfg.cwd, &state, &[1])).1.map(|()| r));
        let r = greeted.map_err(|e| if matches!(e.code, "interrupted" | "budget") { outcome(e) } else { bad(e.message) })?;
        cfg.notices.push(format!("{p}: {name} (port/1, {}); henki --port {p}=std runs a session without it", r.said));
        ports.bound.push((p, name));
        ports.history = Box::new(r);
    }
    Ok(ports)
}

/// The kernel log of this process: bindings' stderr. In the spill dir, so the 24 h sweep removes it; a dot name, so it
/// never matches a session's spill name.
pub fn kernel_log(cfg: &Config) -> PathBuf {
    cfg.state.join("spill").join(format!(".kernel-{}.log", std::process::id()))
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
