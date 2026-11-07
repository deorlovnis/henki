//! Tool processes: the child environment, the spawn recipe, bounded capture with spill, and the ledger of process groups
//! that henki kills by group and, on Linux, by sweeping every descendant. Also the user's own `!cmd`, on the terminal.
use crate::{config::Env, sys, term, types::End, types::Ev, types::ToolOutput, types::lock};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, mpsc, mpsc::Sender};
use std::{collections::VecDeque, ffi::OsString, fs::File, os::unix::process::ExitStatusExt, thread};
use std::{io, io::Read, io::Write, path::Path, path::PathBuf, process::Child, process::Command, process::Stdio, time::Duration, time::Instant};

pub const GRACE: Duration = Duration::from_millis(300);
pub const SPILL_MAX: u64 = 8 << 20;
/// The end of a spill file kept free for the dropped-bytes line, so the whole file stays within `SPILL_MAX`.
const ROOM: u64 = 64;
/// The inherited variables a tool child keeps. `BASH_ENV` and `ENV` never pass, not even through `pass_env`.
const ALLOW: &str = "HOME USER LOGNAME SHELL TERM LANG LC_ALL LC_CTYPE LC_MESSAGES TZ TMPDIR NO_COLOR SSH_AUTH_SOCK SSL_CERT_FILE \
                     SSL_CERT_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR http_proxy HTTP_PROXY \
                     https_proxy HTTPS_PROXY no_proxy NO_PROXY";

/// The process groups of calls that may still have members, by leader pid; a group leaves when a signal gives ESRCH.
static LEDGER: Mutex<Vec<i32>> = Mutex::new(Vec::new());
/// The children henki had before its first call or `!cmd`, such as a launcher's background job that came along through
/// exec. henki did not start them, so the count and the sweep leave them and their descendants alone.
static INHERITED: OnceLock<Vec<i32>> = OnceLock::new();

/// Where a call's full output goes once it passes the cap; `shown` is the path as the marker prints it (`$HOME` as `~`).
pub struct Spill {
    pub path: PathBuf,
    pub shown: String,
}

/// Exactly the child environment: the allowlist, `pass_env`, PATH = `path` (the extensions' `bin/` dirs, then the user
/// PATH), HENKI_USER_PATH = `user_path`, HENKI_SESSION, HENKI_VERSION, HENKI_DEPTH + 1. A proxy value with userinfo is
/// dropped.
pub fn child_env(env: &Env, pass_env: &[String], path: &str, user_path: &str, session: &str) -> Vec<(OsString, OsString)> {
    let keep = |k: &str| (ALLOW.split_whitespace().any(|a| a == k) || pass_env.iter().any(|p| p == k)) && k != "BASH_ENV" && k != "ENV";
    let userinfo = |k: &str, v: &OsString| k.to_ascii_lowercase().ends_with("proxy") && v.as_encoded_bytes().contains(&b'@');
    let mut out: Vec<_> = env.0.iter().filter(|(k, v)| k.to_str().is_some_and(|k| keep(k) && !userinfo(k, v))).cloned().collect();
    let henki = [("PATH", path), ("HENKI_USER_PATH", user_path), ("HENKI_SESSION", session), ("HENKI_VERSION", env!("CARGO_PKG_VERSION")), ("HENKI_DEPTH", &depth(env))];
    out.extend(henki.map(|(k, v)| (k.into(), v.into())));
    out
}

/// The `!cmd` environment: henki's inherited one, with HENKI_DEPTH + 1.
pub fn bang_env(env: &Env) -> Vec<(OsString, OsString)> {
    let mut out: Vec<_> = env.0.iter().filter(|(k, _)| k != "HENKI_DEPTH").cloned().collect();
    out.push(("HENKI_DEPTH".into(), depth(env).into()));
    out
}

/// henki's HENKI_DEPTH + 1 (1 when unset or not a number).
fn depth(env: &Env) -> String {
    env.get("HENKI_DEPTH").and_then(|v| v.to_str()?.parse::<u32>().ok()).map_or(1, |d| u64::from(d) + 1).to_string()
}

/// MAIN thread only. `bash --noprofile --norc -c CMD` in its own session, no tty, stdin /dev/null, one pipe for stdout
/// and stderr. The group is in the ledger before this returns. A capture and a waiter thread send one `Ev::Tool { tag }`.
pub fn spawn(bash: &Path, cmd: &str, cwd: &Path, env: &[(OsString, OsString)], cap: usize, spill: Spill, tag: u32, tx: &Sender<Ev>) -> io::Result<i32> {
    let t0 = Instant::now();
    let (r, w) = io::pipe()?;
    let shared = Arc::new((Mutex::new(Cap::new(cap, spill)), Condvar::new()));
    let (reader, tx, (give, take)) = (Arc::clone(&shared), tx.clone(), mpsc::channel());
    // Both threads start before the fork, so nothing can fail once bash runs. If the spawn below fails, the write ends
    // close (the capture ends) and `give` drops (the waiter ends).
    thread::Builder::new().spawn(move || capture(r, &reader))?;
    thread::Builder::new().spawn(move || take.recv().map(|child| waiter(child, &shared, t0, tag, &tx)))?;
    let mut c = Command::new(bash);
    c.args(["--noprofile", "--norc", "-c", cmd]).current_dir(cwd).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    c.stdin(Stdio::null()).stdout(w.try_clone()?).stderr(w);
    sys::child_setup(&mut c, std::process::id());
    INHERITED.get_or_init(|| children(&sys::procs()));
    #[allow(clippy::disallowed_methods)] // the one place a tool process starts
    let child = c.spawn()?;
    drop(c); // the parent's write ends close here, or EOF never comes
    let pgid = child.id() as i32;
    lock(&LEDGER).push(pgid);
    let _ = give.send(child); // the waiter is in `recv` until it gets it
    Ok(pgid)
}

/// MAIN thread only. The user's `!cmd`: `bash --noprofile --norc -c CMD` in henki's own process group on the terminal
/// (stdin, stdout and stderr inherited), exactly `env`; not in the ledger, though a `!cmd &` job it leaves is counted and
/// swept like a call's. A waiter thread reaps it and sends one `Ev::BangDone { tag, code }` (128 + N when signal N ended
/// it). Returns the pid.
pub fn bang(bash: &Path, cmd: &str, cwd: &Path, env: &[(OsString, OsString)], tag: u32, tx: &Sender<Ev>) -> io::Result<i32> {
    let (tx, (give, take)) = (tx.clone(), mpsc::channel::<Child>());
    // The waiter starts before the fork, so nothing can fail once the command runs.
    thread::Builder::new().spawn(move || {
        let code = take.recv().map(|mut c| c.wait().map_or(-1, |st| st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0))));
        let _ = code.map(|code| tx.send(Ev::BangDone { tag, code }));
    })?;
    let mut c = Command::new(bash);
    c.args(["--noprofile", "--norc", "-c", cmd]).current_dir(cwd).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    sys::bang_setup(&mut c);
    // As before a call: the children henki had before it started anything are fixed first, so the job is not one of them.
    INHERITED.get_or_init(|| children(&sys::procs()));
    #[allow(clippy::disallowed_methods)] // the user's own command, typed at the prompt
    let child = c.spawn()?;
    let pid = child.id() as i32;
    let _ = give.send(child);
    Ok(pid)
}

/// Sends `sig` (0 probes) to every group in the ledger, drops the groups that are gone (ESRCH), returns how many remain.
pub fn signal_groups(sig: i32) -> usize {
    let mut g = lock(&LEDGER);
    pass(&mut g, sig);
    g.len()
}

/// SIGKILL the call's group and, on Linux, every process whose session is the call's shell and their descendants.
pub fn kill_call(pgid: i32) {
    // Scanned before any kill: the children of a killed process move to a new parent at once, out of the tree.
    let members = sys::tree(&sys::procs(), |p| pgid > 1 && p.sid == pgid);
    sys::kill(-pgid, libc::SIGKILL);
    members.into_iter().for_each(|p| _ = sys::kill(p, libc::SIGKILL));
}

/// After each call; kills nothing. Reaps zombie children by pid (never a ledger leader); returns the number of live
/// background descendants (macOS: live ledger groups of earlier calls).
pub fn reap() -> usize {
    let mut g = lock(&LEDGER);
    let live = pass(&mut g, 0);
    if cfg!(target_os = "linux") { live.len() } else { g.len() }
}

/// One sweep pass: reap zombie children by pid, SIGKILL every live descendant of henki and every ledger group; returns
/// how many it signalled (0: nothing is left). macOS: the groups only.
pub fn sweep_pass() -> usize {
    sweep(&mut lock(&LEDGER))
}

/// The panic hook's kill: try_lock the ledger, SIGKILL every group, sweep passes without waiting.
pub fn panic_kill() {
    // The panicking thread may hold the ledger itself; the sweep still finds every process through the process tree.
    let mut g = LEDGER.try_lock().map(|g| g.clone()).unwrap_or_default();
    _ = (0..5).all(|_| sweep(&mut g) > 0); // up to five passes, until one finds nothing
}

fn sweep(groups: &mut Vec<i32>) -> usize {
    let live = pass(groups, libc::SIGKILL);
    if cfg!(target_os = "linux") { live.into_iter().filter(|&p| sys::kill(p, libc::SIGKILL)).count() } else { groups.len() }
}

/// Reaps henki's zombie children by pid, except group leaders (their waiters own them), then sends `sig` to every group
/// and drops the gone ones (ESRCH); returns the live descendants (Linux; none elsewhere). The reap comes first because a
/// zombie still answers for its group: a group whose members have all ended would never give ESRCH.
fn pass(groups: &mut Vec<i32>, sig: i32) -> Vec<i32> {
    let (ps, me) = (sys::procs(), std::process::id() as i32);
    let inherited = INHERITED.get_or_init(|| children(&ps));
    ps.iter().filter(|p| p.zombie && p.ppid == me && !groups.contains(&p.pid)).for_each(|p| _ = sys::reap(p.pid));
    groups.retain(|&pg| sys::kill(-pg, sig));
    sys::tree(&ps, |p| p.ppid == me && !inherited.contains(&p.pid))
}

/// henki's direct children.
fn children(ps: &[sys::Proc]) -> Vec<i32> {
    ps.iter().filter(|p| p.ppid == std::process::id() as i32).map(|p| p.pid).collect()
}

type Shared = (Mutex<Cap>, Condvar);

/// Reads the call's pipe to EOF. henki installs no signal handler, so a read is never interrupted.
fn capture(mut r: io::PipeReader, s: &Shared) {
    let mut buf = vec![0; 64 << 10];
    while let Ok(n @ 1..) = r.read(&mut buf) {
        lock(&s.0).push(&buf[..n]);
    }
    lock(&s.0).eof = true;
    s.1.notify_all();
}

/// Owns the shell: its exit, then the pipe's EOF for at most `GRACE` (a background process may hold the pipe), then the
/// one `Ev::Tool`. From then on the capture drains to nowhere.
fn waiter(mut child: Child, s: &Shared, t0: Instant, tag: u32, tx: &Sender<Ev>) {
    let end = child.wait().map_or(End::Exit(-1), |st| st.code().map_or_else(|| End::Signal(st.signal().unwrap_or(0)), End::Exit));
    let mut c = s.1.wait_timeout_while(lock(&s.0), GRACE, |c| !c.eof).unwrap_or_else(PoisonError::into_inner).0;
    let _ = tx.send(Ev::Tool { tag, out: ToolOutput { text: c.text(), bytes: c.total, ms: t0.elapsed().as_millis() as u64, end } });
}

/// One call's output: the first cap/2 bytes, a ring of the last cap - cap/2, the total, and the spill past the cap.
struct Cap {
    cap: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: u64,
    spill: Spill,
    sink: Sink,
    eof: bool,
    done: bool,
}

/// The spill: not needed yet (the output is within the cap), open with the bytes written, or not kept (why).
enum Sink {
    Later,
    Open(File, u64),
    Lost(&'static str),
}

impl Sink {
    /// Appends what fits. A failed write removes the partial file at `path` and keeps nothing more.
    fn put(&mut self, b: &[u8], path: &Path) {
        if let Sink::Open(f, n) = self {
            let k = b.len().min((SPILL_MAX - ROOM - *n) as usize);
            if f.write_all(&b[..k]).is_ok() {
                *n += k as u64;
            } else {
                let _ = std::fs::remove_file(path);
                *self = Sink::Lost("could not write");
            }
        }
    }
}

impl Cap {
    /// Both halves allocated once, so a call holds at most the cap plus the 64 KiB read buffer.
    fn new(cap: usize, spill: Spill) -> Cap {
        Cap { cap, head: Vec::with_capacity(cap / 2), tail: VecDeque::with_capacity(cap - cap / 2), total: 0, spill, sink: Sink::Later, eof: false, done: false }
    }

    fn push(&mut self, b: &[u8]) {
        if self.done {
            return;
        }
        if matches!(self.sink, Sink::Later) && self.total + b.len() as u64 > self.cap as u64 {
            self.sink = match sys::create_excl(&self.spill.path, 0o600, false) {
                Ok(f) => Sink::Open(f, 0),
                Err(e) => Sink::Lost(if e.kind() == io::ErrorKind::AlreadyExists { "name in use" } else { "could not write" }),
            };
            let (a, c) = self.tail.as_slices();
            [&self.head[..], a, c].into_iter().for_each(|part| self.sink.put(part, &self.spill.path));
        }
        self.sink.put(b, &self.spill.path);
        self.total += b.len() as u64;
        let (head, rest) = b.split_at(b.len().min(self.cap / 2 - self.head.len()));
        self.head.extend_from_slice(head);
        let keep = self.cap - self.cap / 2;
        let rest = &rest[rest.len().saturating_sub(keep)..];
        self.tail.drain(..(self.tail.len() + rest.len()).saturating_sub(keep));
        self.tail.extend(rest);
    }

    /// Detaches the capture and returns the text: all of it, or the head, the marker and the tail cut on UTF-8 boundaries.
    fn text(&mut self) -> String {
        self.done = true;
        let kept = match std::mem::replace(&mut self.sink, Sink::Later) {
            Sink::Later => None,
            Sink::Open(mut f, n) => {
                let _ = (self.total > n).then(|| write!(f, "\n[henki: {} more bytes dropped]\n", term::num(self.total - n)));
                Some(format!("full output: {}", self.spill.shown))
            }
            Sink::Lost(why) => Some(format!("full output not kept ({why})")),
        };
        let tail = self.tail.make_contiguous();
        let Some(kept) = kept else { return String::from_utf8_lossy(&[&self.head[..], tail].concat()).into_owned() };
        let (h, t) = (head_end(&self.head), tail_start(tail));
        let cut = self.total - (h + tail.len() - t) as u64;
        let (head, tail) = (String::from_utf8_lossy(&self.head[..h]), String::from_utf8_lossy(&tail[t..]));
        format!("{head}\n[... {} of {} bytes cut; {kept} ...]\n{tail}", term::num(cut), term::num(self.total))
    }
}

/// The length of `b` without a code point cut short at its end.
fn head_end(b: &[u8]) -> usize {
    let n = b.len();
    match (n.saturating_sub(3)..n).rev().find(|&i| b[i] & 0xC0 != 0x80) {
        Some(i) if n - i < b[i].leading_ones().max(1) as usize => i, // a lead byte's leading ones are its width
        _ => n,
    }
}

/// How many bytes at the start of `b` continue a code point that began before it.
fn tail_start(b: &[u8]) -> usize {
    b.iter().take(3).take_while(|&&c| c & 0xC0 == 0x80).count()
}

#[cfg(test)]
mod tests;
