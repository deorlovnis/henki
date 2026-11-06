#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice, unsafe_code)]
//! The end-to-end harness: a `Run` is one throwaway HOME, XDG dirs, a git working directory and a config file; each
//! `Cmd` starts `henki run` there; `Done::finish` checks what must hold after every run: no process with the run's
//! marker survives, nothing that must be escaped reaches an output raw, the journal pairs every call with one result,
//! the records end `turn_end`, the final stop's notices, `result`, `close` with the right byte count, the exit code is
//! the one of the `result` status, and `--json` stdout mirrors the journal.
use crate::mock::{Mock, Step, shell};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The binary under test: this build's, or another one (a static release build) named by `HENKI_TEST_BIN`.
pub fn bin() -> OsString {
    std::env::var_os("HENKI_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_henki").into())
}

pub const SEC: Duration = Duration::from_secs(1);
static N: AtomicUsize = AtomicUsize::new(0);

/// The exit code of a `result` status.
pub fn code_of(status: &str) -> i32 {
    match status {
        "ok" => 0,
        "gave_up" => 1,
        "error" => 2,
        "needs_approval" => 3,
        "budget" => 124,
        "interrupted" => 130,
        "output_closed" => 141,
        s => panic!("unknown status {s:?}"),
    }
}

/// One test's world. Dropping it kills what carries its marker and deletes its directory (a failure message carries
/// the run's output and journal instead).
pub struct Run {
    pub dir: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    /// `3133.<pid><n>` in fixed widths: unique, and a valid `sleep` argument.
    pub marker: String,
    pub config_path: PathBuf,
    keys: Vec<(String, String)>,
    raw: Option<Option<String>>,
    env: BTreeMap<OsString, Option<OsString>>,
}

impl Run {
    /// A run whose default config points at `url`, with `approve = "auto"` and a known context window.
    pub fn new(url: &str) -> Run {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(format!("/tmp/henki-e2e-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (home, cwd) = (dir.join("home"), dir.join("work"));
        for d in [&home, &cwd, &dir.join("xdg/state"), &dir.join("xdg/config"), &dir.join("xdg/data"), &dir.join("xdg/cache")] {
            fs::create_dir_all(d).unwrap();
        }
        git_init(&cwd);
        let config_path = home.join(".config/henki/config.toml");
        let mut env = BTreeMap::new();
        for (k, v) in [("HOME", home.clone()), ("PATH", "/usr/local/bin:/usr/bin:/bin".into()), ("XDG_STATE_HOME", dir.join("xdg/state"))] {
            env.insert(k.into(), Some(v.into_os_string()));
        }
        for k in ["config", "data", "cache"] {
            env.insert(format!("XDG_{}_HOME", k.to_uppercase()).into(), Some(dir.join("xdg").join(k).into_os_string()));
        }
        let keys = [("model", "\"mock-model\"".into()), ("base_url", format!("\"{url}\"")), ("approve", "\"auto\"".into()), ("context_window", "32768".into())];
        let keys = keys.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        let marker = format!("3133.{:07}{n:04}", std::process::id());
        Run { dir, home, cwd, marker, config_path, keys, raw: None, env }
    }

    /// Points `base_url` at `url`.
    pub fn at(&mut self, url: &str) -> &mut Run {
        self.set("base_url", &format!("\"{url}\""))
    }

    /// Sets a config key; `value` is TOML text (`"\"ask\""`, `"3"`, `"[\"read\"]"`).
    pub fn set(&mut self, key: &str, value: &str) -> &mut Run {
        match self.keys.iter_mut().find(|(k, _)| k == key) {
            Some(kv) => kv.1 = value.into(),
            None => self.keys.push((key.into(), value.into())),
        }
        self
    }

    pub fn unset(&mut self, key: &str) -> &mut Run {
        self.keys.retain(|(k, _)| k != key);
        self
    }

    /// Some(text): the config file holds exactly `text`; None: there is no config file.
    pub fn config_text(&mut self, text: Option<&str>) -> &mut Run {
        self.raw = Some(text.map(String::from));
        self
    }

    pub fn env(&mut self, k: &str, v: &str) -> &mut Run {
        self.env.insert(k.into(), Some(v.into()));
        self
    }

    pub fn unenv(&mut self, k: &str) -> &mut Run {
        self.env.insert(k.into(), None);
        self
    }

    /// `sleep MARKER; :`: a call that runs until something kills it. bash forks the sleep, and a fork does not keep the
    /// parent-death signal, so when henki exits only henki's own kill can have ended it.
    pub fn sleep(&self) -> String {
        format!("sleep {}; :", self.marker)
    }

    /// `exec sleep MARKER`: the sleep is henki's direct child, which the parent-death signal ends with henki (Linux).
    pub fn exec_sleep(&self) -> String {
        format!("exec sleep {}", self.marker)
    }

    /// The state dir henki uses: `$XDG_STATE_HOME/henki`, else `~/.local/state/henki`.
    pub fn state(&self) -> PathBuf {
        match self.env.get(&OsString::from("XDG_STATE_HOME")) {
            Some(Some(x)) => PathBuf::from(x).join("henki"),
            _ => self.home.join(".local/state/henki"),
        }
    }

    pub fn journals(&self) -> Vec<PathBuf> {
        journals_in(&self.state())
    }

    fn write_config(&self) {
        let text = match &self.raw {
            Some(t) => t.clone(),
            None => Some(self.keys.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()),
        };
        match text {
            Some(t) => {
                let dir = self.config_path.parent().unwrap();
                fs::create_dir_all(dir).unwrap();
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(&self.config_path, t).unwrap();
                fs::set_permissions(&self.config_path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            None => {
                let _ = fs::remove_file(&self.config_path);
            }
        }
    }

    /// `henki run ARGS` in the working directory.
    pub fn cmd<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Cmd<'_> {
        Cmd {
            run: self,
            args: args.iter().map(|a| a.as_ref().to_os_string()).collect(),
            stdin: In::Null,
            cwd: None,
            timeout: Duration::from_secs(20),
            close_first: false,
            unread: false,
            default_signals: false,
        }
    }

    /// `henki run --json TASK`, to its end.
    pub fn json(&self, task: &str) -> Done {
        self.cmd(&["--json", task]).go()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.cwd.join(rel)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        kill_marked(&self.marker);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn git_init(cwd: &Path) {
    let ok = Command::new("git").args(["init", "-q"]).current_dir(cwd).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        fs::create_dir_all(cwd.join(".git")).unwrap();
    }
}

pub enum In {
    Null,
    Bytes(Vec<u8>),
    Open,
    File(fs::File),
}

pub struct Cmd<'a> {
    run: &'a Run,
    args: Vec<OsString>,
    stdin: In,
    cwd: Option<PathBuf>,
    timeout: Duration,
    close_first: bool,
    unread: bool,
    default_signals: bool,
}

impl<'a> Cmd<'a> {
    /// henki starts with HUP, INT, QUIT, PIPE and TERM at their default action, whatever the test process ignores.
    #[cfg(target_os = "linux")]
    pub fn default_signals(mut self) -> Cmd<'a> {
        self.default_signals = true;
        self
    }

    pub fn stdin(mut self, i: In) -> Cmd<'a> {
        self.stdin = i;
        self
    }

    pub fn cwd(mut self, d: &Path) -> Cmd<'a> {
        self.cwd = Some(d.into());
        self
    }

    pub fn timeout(mut self, t: Duration) -> Cmd<'a> {
        self.timeout = t;
        self
    }

    /// The reader closes stdout after the first line.
    pub fn close_after_first_line(mut self) -> Cmd<'a> {
        self.close_first = true;
        self
    }

    /// Nobody reads stdout while henki runs.
    pub fn unread_stdout(mut self) -> Cmd<'a> {
        self.unread = true;
        self
    }

    pub fn go(self) -> Done {
        self.spawn().wait()
    }

    pub fn spawn(self) -> Live {
        let r = self.run;
        r.write_config();
        let before: HashMap<PathBuf, u64> = r.journals().into_iter().map(|p| (p.clone(), fs::metadata(&p).map(|m| m.len()).unwrap_or(0))).collect();
        let mut c = Command::new(bin());
        c.arg("run").args(&self.args).current_dir(self.cwd.as_deref().unwrap_or(&r.cwd)).env_clear();
        for (k, v) in &r.env {
            if let Some(v) = v {
                c.env(k, v);
            }
        }
        let (stdio, file, bytes) = match self.stdin {
            In::Null => (Stdio::null(), None, None),
            In::File(f) => (Stdio::from(f.try_clone().unwrap()), Some(f), None),
            In::Bytes(b) => (Stdio::piped(), None, Some(b)),
            In::Open => (Stdio::piped(), None, None),
        };
        c.stdin(stdio).stdout(Stdio::piped()).stderr(Stdio::piped());
        if self.default_signals {
            use std::os::unix::process::CommandExt;
            // SAFETY: runs in the forked child before exec; signal(2) is async-signal-safe and touches no shared memory.
            unsafe {
                c.pre_exec(|| {
                    for s in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGPIPE, libc::SIGTERM] {
                        libc::signal(s, libc::SIG_DFL);
                    }
                    Ok(())
                });
            }
        }
        let started = Instant::now();
        let mut child = c.spawn().expect("start henki");
        let mut stdin = child.stdin.take();
        if let Some(b) = bytes {
            let mut w = stdin.take().unwrap();
            thread::spawn(move || {
                use std::io::Write;
                let _ = w.write_all(&b);
            });
        }
        let out = Pipe::new();
        let err = Pipe::new();
        let mut unread = None;
        let so = child.stdout.take().unwrap();
        if self.unread {
            unread = Some(OwnedFd::from(so));
        } else {
            reader(OwnedFd::from(so), out.clone(), self.close_first);
        }
        reader(OwnedFd::from(child.stderr.take().unwrap()), err.clone(), false);
        let json = self.args.iter().any(|a| a == "--json");
        Live {
            pid: child.id() as i32,
            child,
            stdin,
            out,
            err,
            started,
            exit: None,
            killed: false,
            unread,
            json,
            timeout: self.timeout,
            before,
            marker: r.marker.clone(),
            state: r.state(),
            stdin_file: file,
        }
    }
}

struct Pipe {
    buf: Mutex<Vec<u8>>,
    close: AtomicBool,
    closed: AtomicBool,
    eof: AtomicBool,
}

impl Pipe {
    fn new() -> Arc<Pipe> {
        Arc::new(Pipe { buf: Mutex::new(Vec::new()), close: AtomicBool::new(false), closed: AtomicBool::new(false), eof: AtomicBool::new(false) })
    }
}

fn nonblocking(fd: &OwnedFd) {
    // SAFETY: fcntl on a descriptor this function's caller owns; F_GETFL/F_SETFL touch only its flags.
    unsafe {
        let fl = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK);
    }
}

/// Reads a pipe into `p` until EOF, polling so it can close the read end on request (or after the first line).
fn reader(fd: OwnedFd, p: Arc<Pipe>, close_first: bool) {
    nonblocking(&fd);
    thread::spawn(move || {
        let mut f = fs::File::from(fd);
        let mut b = vec![0u8; 65536];
        loop {
            if p.close.load(Ordering::SeqCst) {
                drop(f);
                p.closed.store(true, Ordering::SeqCst);
                return;
            }
            let mut pfd = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd on the stack, count 1, a 5 ms timeout.
            unsafe { libc::poll(&mut pfd, 1, 5) };
            match f.read(&mut b) {
                Ok(0) => {
                    p.eof.store(true, Ordering::SeqCst);
                    return;
                }
                Ok(k) => {
                    let mut buf = p.buf.lock().unwrap();
                    buf.extend_from_slice(&b[..k]);
                    if close_first && buf.contains(&b'\n') {
                        p.close.store(true, Ordering::SeqCst);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    p.eof.store(true, Ordering::SeqCst);
                    return;
                }
            }
        }
    });
}

/// A henki process still running.
pub struct Live {
    pub pid: i32,
    child: Child,
    /// Keeps a piped stdin open; only the Linux interrupt matrix writes to it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    stdin: Option<ChildStdin>,
    out: Arc<Pipe>,
    err: Arc<Pipe>,
    pub started: Instant,
    exit: Option<(i32, i64, Instant)>,
    killed: bool,
    unread: Option<OwnedFd>,
    json: bool,
    timeout: Duration,
    before: HashMap<PathBuf, u64>,
    marker: String,
    state: PathBuf,
    stdin_file: Option<fs::File>,
}

impl Live {
    /// The exit status, once: (raw wait status, maxrss in KiB, when it was seen).
    fn poll(&mut self) -> bool {
        if self.exit.is_none() {
            let mut status = 0;
            // SAFETY: an all-zero rusage is a valid value for the kernel to overwrite.
            let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
            // SAFETY: wait4 on our own child's pid with valid out-pointers; WNOHANG never blocks.
            let r = unsafe { libc::wait4(self.pid, &mut status, libc::WNOHANG, &mut ru) };
            if r == self.pid {
                // macOS counts ru_maxrss in bytes, Linux in KiB.
                let kb = if cfg!(target_os = "macos") { ru.ru_maxrss / 1024 } else { ru.ru_maxrss };
                self.exit = Some((status, kb, Instant::now()));
            }
        }
        self.exit.is_some()
    }

    pub fn running(&mut self) -> bool {
        !self.poll()
    }

    pub fn signal(&mut self, sig: i32) {
        assert!(self.running(), "henki ended before signal {sig}:\n{}", self.diag());
        // SAFETY: kill(2) on our own, unreaped child.
        unsafe { libc::kill(self.pid, sig) };
    }

    /// SIGKILL: the run has no `result`, and dangling calls are allowed at the journal's end.
    pub fn kill9(&mut self) {
        self.killed = true;
        // SAFETY: kill(2) on our own, unreaped child.
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux interrupt matrix uses it
    pub fn close_stdout(&self) {
        self.out.close.store(true, Ordering::SeqCst);
        let end = Instant::now() + SEC;
        while !self.out.closed.load(Ordering::SeqCst) && !self.out.eof.load(Ordering::SeqCst) && Instant::now() < end {
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux interrupt matrix uses it
    pub fn write_stdin(&mut self, b: &[u8]) {
        use std::io::Write;
        let _ = self.stdin.as_mut().expect("stdin is open").write_all(b);
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // only the Linux interrupt matrix uses it
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.buf.lock().unwrap()).into_owned()
    }

    pub fn stdout_records(&self) -> Vec<Value> {
        lines(&self.stdout())
    }

    fn diag(&self) -> String {
        format!("stdout:\n{}\nstderr:\n{}", self.stdout(), String::from_utf8_lossy(&self.err.buf.lock().unwrap()))
    }

    /// Polls `cond` while henki runs; panics if henki ends first or after `within`.
    pub fn until(&mut self, what: &str, within: Duration, mut cond: impl FnMut(&mut Live) -> bool) {
        let end = Instant::now() + within;
        loop {
            if cond(self) {
                return;
            }
            assert!(!self.poll(), "henki ended while waiting for {what}:\n{}", self.diag());
            assert!(Instant::now() < end, "no {what} within {within:?}:\n{}", self.diag());
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Until the main thread blocks INT, QUIT, TERM and HUP: from then on a signal is henki's to handle.
    #[cfg(target_os = "linux")]
    pub fn ready(&mut self) {
        let pid = self.pid;
        self.until("the signal mask", 5 * SEC, |_| sigblk(pid) & 0x4007 == 0x4007);
    }

    /// Until henki has a second thread: henki blocks INT, QUIT, TERM and HUP before it starts any thread.
    #[cfg(not(target_os = "linux"))]
    pub fn ready(&mut self) {
        let pid = self.pid.to_string();
        self.until("a second thread", 5 * SEC, |_| {
            let out = Command::new("ps").args(["-M", "-p", &pid]).output().expect("ps");
            String::from_utf8_lossy(&out.stdout).lines().count() > 2
        });
    }

    /// Until a stdout record satisfies `pred`; returns it.
    pub fn wait_record(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Value {
        let mut found = None;
        self.until("a matching stdout record", within, |l| {
            found = l.stdout_records().into_iter().find(|r| pred(r));
            found.is_some()
        });
        found.unwrap()
    }

    /// Until a record this run appended to its journal satisfies `pred`; returns it.
    pub fn wait_journal(&mut self, within: Duration, pred: impl Fn(&Value) -> bool) -> Value {
        let mut found = None;
        self.until("a matching journal record", within, |l| {
            found = l.appended().into_iter().find(|r| pred(r));
            found.is_some()
        });
        found.unwrap()
    }

    /// Until a process with the marker in its command line exists; returns its pid.
    pub fn wait_marked(&mut self, within: Duration) -> i32 {
        let m = self.marker.clone();
        let mut pid = 0;
        self.until("a process with the marker", within, |_| {
            pid = marked(&m).first().map_or(0, |p| p.0);
            pid != 0
        });
        pid
    }

    /// The journal this run writes: a new file, or one that grew since the start.
    fn journal(&self) -> Option<PathBuf> {
        let mut changed: Vec<PathBuf> = journals_in(&self.state).into_iter().filter(|p| self.before.get(p) != fs::metadata(p).ok().map(|m| m.len()).as_ref()).collect();
        assert!(changed.len() <= 1, "more than one journal changed: {changed:?}");
        changed.pop()
    }

    /// The complete records this run appended so far.
    fn appended(&self) -> Vec<Value> {
        let Some(p) = self.journal() else { return Vec::new() };
        let b = fs::read(&p).unwrap_or_default();
        let from = (*self.before.get(&p).unwrap_or(&0) as usize).min(b.len());
        let text = String::from_utf8_lossy(&b[from..]);
        text.split_inclusive('\n').filter(|l| l.ends_with('\n')).filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// To the end (the timeout kills henki and fails the test).
    pub fn wait(mut self) -> Done {
        let end = self.started + self.timeout;
        while !self.poll() {
            if Instant::now() > end {
                self.kill9();
                let _ = self.child.wait();
                panic!("henki did not end within {:?}:\n{}", self.timeout, self.diag());
            }
            thread::sleep(Duration::from_millis(1));
        }
        let (status, maxrss, at) = self.exit.unwrap();
        let mut stdout_complete = !self.out.closed.load(Ordering::SeqCst);
        if let Some(fd) = self.unread.take() {
            stdout_complete = false;
            nonblocking(&fd);
            let mut b = Vec::new();
            let _ = fs::File::from(fd).read_to_end(&mut b);
            *self.out.buf.lock().unwrap() = b;
        } else {
            let end = Instant::now() + 2 * SEC;
            while !self.out.eof.load(Ordering::SeqCst) && !self.out.closed.load(Ordering::SeqCst) && Instant::now() < end {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let end = Instant::now() + 2 * SEC;
        while !self.err.eof.load(Ordering::SeqCst) && Instant::now() < end {
            thread::sleep(Duration::from_millis(1));
        }
        let exited = libc::WIFEXITED(status);
        let journal = self.journal();
        Done {
            code: exited.then(|| libc::WEXITSTATUS(status)),
            signal: libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)),
            stdout: self.out.buf.lock().unwrap().clone(),
            stderr: self.err.buf.lock().unwrap().clone(),
            elapsed: at - self.started,
            exited: at,
            maxrss_kb: maxrss,
            json: self.json,
            journal_bytes: journal.as_ref().map(|p| fs::read(p).unwrap_or_default()).unwrap_or_default(),
            journal,
            killed: self.killed,
            closed_stdout: self.out.closed.load(Ordering::SeqCst),
            stdout_complete,
            marker: self.marker.clone(),
            stdin_file: self.stdin_file.take(),
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        if !self.poll() {
            // SAFETY: kill(2) on our own, unreaped child.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }
}

fn journals_in(state: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for scope in fs::read_dir(state.join("sessions")).into_iter().flatten().flatten() {
        for f in fs::read_dir(scope.path()).into_iter().flatten().flatten() {
            if f.path().extension().is_some_and(|e| e == "jsonl") {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}

/// The JSON lines of `text` (lines that do not parse are dropped).
pub fn lines(text: &str) -> Vec<Value> {
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// A finished run.
pub struct Done {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
    pub exited: Instant,
    pub maxrss_kb: i64,
    pub json: bool,
    pub journal: Option<PathBuf>,
    /// The journal file as the run left it.
    pub journal_bytes: Vec<u8>,
    killed: bool,
    closed_stdout: bool,
    stdout_complete: bool,
    marker: String,
    pub stdin_file: Option<fs::File>,
}

impl Done {
    pub fn out(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn err(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// The `--json` stdout records.
    pub fn records(&self) -> Vec<Value> {
        lines(&self.out())
    }

    /// Every record of the journal file.
    pub fn journal_all(&self) -> Vec<Value> {
        lines(&String::from_utf8_lossy(&self.journal_bytes))
    }

    /// The records after the last `session` record: this run's.
    pub fn segment(&self) -> Vec<Value> {
        let all = self.journal_all();
        let i = all.iter().rposition(|r| r["t"] == "session").unwrap_or(0);
        all[i..].to_vec()
    }

    pub fn kinds(&self) -> Vec<String> {
        self.segment().iter().map(|r| r["t"].as_str().unwrap_or("").to_string()).collect()
    }

    /// This run's journal records of kind `t`.
    pub fn of(&self, t: &str) -> Vec<Value> {
        self.segment().into_iter().filter(|r| r["t"] == t).collect()
    }

    pub fn notices(&self) -> Vec<String> {
        self.of("notice").iter().map(|r| r["text"].as_str().unwrap_or("").to_string()).collect()
    }

    pub fn results(&self) -> Vec<Value> {
        self.of("tool_result")
    }

    /// The `result` record: the journal's, else (before a session) the one on stdout.
    pub fn result(&self) -> Value {
        if let Some(r) = self.of("result").pop() {
            return r;
        }
        self.records().into_iter().rfind(|r| r["t"] == "result").unwrap_or_else(|| panic!("no result record\n{}", self.diag()))
    }

    pub fn status(&self) -> String {
        self.result()["status"].as_str().unwrap_or("").to_string()
    }

    pub fn id(&self) -> String {
        self.result()["session"].as_str().unwrap_or("").to_string()
    }

    pub fn diag(&self) -> String {
        let journal = String::from_utf8_lossy(&self.journal_bytes);
        format!("exit {:?} signal {:?} after {:?}\nstdout:\n{}\nstderr:\n{}\njournal:\n{journal}", self.code, self.signal, self.elapsed, self.out(), self.err())
    }

    /// Asserts `cond`, showing the run on failure.
    pub fn check(&self, cond: bool, what: &str) {
        assert!(cond, "{what}\n{}", self.diag());
    }

    /// The universal postconditions.
    pub fn finish(&self) {
        if self.killed && !cfg!(target_os = "linux") {
            // Without a parent-death signal the shell outlives a SIGKILLed henki; that is expected there.
            kill_marked(&self.marker);
        }
        let survivors = survivors(&self.marker);
        self.check(survivors.is_empty(), &format!("processes with the marker survived: {survivors:?}"));
        self.check(self.killed || self.code.is_some(), "henki died of a signal the test did not send");
        for (name, b) in [("stdout", &self.stdout), ("stderr", &self.stderr), ("the journal", &self.journal_bytes)] {
            if let Some(c) = raw_escapable(b) {
                self.check(false, &format!("a raw U+{:04X} reached {name}", c as u32));
            }
        }
        if self.journal.is_some() {
            let bytes = &self.journal_bytes;
            self.check(bytes.ends_with(b"\n"), "the journal ends inside a line");
            let mut offset = 0;
            let mut recs = Vec::new();
            for l in bytes.split_inclusive(|&b| b == b'\n') {
                let r: Value = serde_json::from_slice(l).unwrap_or_else(|e| panic!("journal line at {offset} does not parse: {e}\n{}", self.diag()));
                if r["t"] == "close" {
                    self.check(r["bytes"].as_u64() == Some(offset as u64), &format!("close.bytes {} is not its offset {offset}", r["bytes"]));
                }
                offset += l.len();
                recs.push(r);
            }
            if let Err(e) = pairing(&recs, self.killed) {
                panic!("pairing: {e}\n{}", self.diag());
            }
            let kinds = self.kinds();
            self.check(kinds.first().map(String::as_str) == Some("session"), "the run's records do not start with session");
            if !self.killed {
                // The session end: `turn_end`, notices of the final stop (`stopped N background processes at exit`), then
                // `result` and `close`.
                self.check(kinds.iter().filter(|k| *k == "turn_end").count() == 1, "not exactly one turn_end");
                let tail = kinds.iter().position(|k| k == "turn_end").map_or(&kinds[..0], |t| &kinds[t + 1..]);
                let order = tail.len() >= 2 && tail[tail.len() - 2..] == ["result", "close"] && tail[..tail.len() - 2].iter().all(|k| k == "notice");
                self.check(order, &format!("the run's records do not end turn_end, notices, result, close: {kinds:?}"));
            }
        }
        if self.killed {
            return;
        }
        let code = self.code.unwrap();
        let has_result = self.journal.is_some() || self.json && !self.closed_stdout;
        if has_result {
            let r = self.result();
            let status = r["status"].as_str().unwrap_or("").to_string();
            self.check(r["exit"].as_i64() == Some(code_of(&status) as i64), "the result's exit is not its status's code");
            let closed_ok = code == 141 && self.closed_stdout && ["ok", "gave_up", "needs_approval"].contains(&status.as_str());
            self.check(code == code_of(&status) || closed_ok, &format!("exit {code} for status {status}"));
            if self.journal.is_none() {
                self.check(r["session"].is_null(), "a result without a journal names a session");
            }
        } else {
            self.check([2, 124, 130, 141].contains(&code), &format!("exit {code} without a result"));
        }
        if self.json {
            self.check(self.stderr.is_empty(), "--json wrote to stderr");
        }
        if self.json && self.stdout_complete && !self.closed_stdout {
            let out = self.records();
            self.check(out.last().is_some_and(|r| r["t"] == "result"), "stdout does not end with the result");
            self.check(out.iter().filter(|r| r["t"] == "result").count() == 1, "stdout holds more than one result");
            let shown: Vec<Value> = out.into_iter().filter(|r| r["t"] != "text" && r["t"] != "reasoning").collect();
            let journaled: Vec<Value> = self.segment().into_iter().filter(|r| r["t"] != "close").collect();
            if self.journal.is_some() {
                // From `session` on, stdout is the journal; before it (resume notices) only journaled records.
                let k = shown.iter().position(|r| r["t"] == "session").unwrap_or(0);
                self.check(shown[k..] == journaled[..], "the --json records differ from the journal's");
                self.check(shown[..k].iter().all(|r| self.journal_all().contains(r)), "stdout holds a record the journal lacks");
            } else {
                self.check(shown.len() == 1, "stdout holds more than the result before a session");
            }
        }
    }
}

/// Over `user`, `assistant` and `tool_result` records only: call ids unique; each call has exactly one result after
/// its assistant record and before the next `assistant` or `user` record; no result without an open call.
/// `open_tail`: the run was killed, so calls of the last assistant record may lack results.
pub fn pairing(recs: &[Value], open_tail: bool) -> Result<(), String> {
    let mut seen = HashSet::new();
    let mut open: Vec<String> = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        match r["t"].as_str() {
            Some(t @ ("user" | "assistant")) => {
                if !open.is_empty() {
                    return Err(format!("record {i} ({t}): calls {open:?} have no result"));
                }
                for c in r["msg"]["tool_calls"].as_array().into_iter().flatten() {
                    let id = c["id"].as_str().unwrap_or("").to_string();
                    if !seen.insert(id.clone()) {
                        return Err(format!("record {i}: call id {id:?} is used twice"));
                    }
                    open.push(id);
                }
            }
            Some("tool_result") => {
                let id = r["id"].as_str().unwrap_or("");
                let Some(k) = open.iter().position(|o| o == id) else { return Err(format!("record {i}: result {id:?} has no open call")) };
                open.remove(k);
            }
            _ => {}
        }
    }
    if open.is_empty() || open_tail { Ok(()) } else { Err(format!("calls {open:?} have no result")) }
}

/// The chat messages the journal's history renders to (after the system message): what a request must hold.
pub fn render(recs: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for r in recs {
        match r["t"].as_str() {
            Some("user") => out.push(serde_json::json!({"content": r["msg"]["content"], "role": "user"})),
            Some("assistant") => {
                let mut m = serde_json::json!({"content": r["msg"]["content"], "role": "assistant"});
                let calls: Vec<Value> = r["msg"]["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| serde_json::json!({"function": {"arguments": c["args"].to_string(), "name": c["name"]}, "id": c["id"], "type": "function"}))
                    .collect();
                if !calls.is_empty() {
                    m["tool_calls"] = calls.into();
                }
                out.push(m);
            }
            Some("tool_result") => out.push(serde_json::json!({"content": r["content"], "role": "tool", "tool_call_id": r["id"]})),
            _ => {}
        }
    }
    out
}

/// A request body up to the end of its last message (`],"model":` follows the messages array in sorted keys).
pub fn messages_prefix(body: &str) -> &str {
    let i = body.find("],\"model\":").unwrap_or_else(|| panic!("no messages array in {body}"));
    &body[..i]
}

/// Every live process whose command line holds `marker`, with that command line. Panics when /proc cannot be listed,
/// so a missing process table never passes for "no survivors".
#[cfg(target_os = "linux")]
pub fn marked(marker: &str) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    let mut me = false;
    for e in fs::read_dir("/proc").expect("/proc").flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        me |= pid == std::process::id() as i32;
        let Ok(cmd) = fs::read(e.path().join("cmdline")) else { continue };
        let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ");
        if cmd.contains(marker) && state(pid).is_some_and(|s| s != 'Z') {
            out.push((pid, cmd));
        }
    }
    assert!(me, "/proc does not list this test process");
    out
}

#[cfg(not(target_os = "linux"))]
pub fn marked(marker: &str) -> Vec<(i32, String)> {
    marked_ps(marker)
}

/// `marked` through `ps` (macOS has no /proc). Panics when the listing lacks this test process.
pub fn marked_ps(marker: &str) -> Vec<(i32, String)> {
    let out = Command::new("ps").args(["-A", "-ww", "-o", "pid=", "-o", "stat=", "-o", "command="]).output().expect("ps");
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(i32, &str, &str)> = text
        .lines()
        .filter_map(|l| {
            let (pid, rest) = l.trim_start().split_once(' ')?;
            let (stat, cmd) = rest.trim_start().split_once(' ').unwrap_or((rest.trim_start(), ""));
            Some((pid.parse().ok()?, stat, cmd.trim_start()))
        })
        .collect();
    assert!(out.status.success() && rows.iter().any(|r| r.0 == std::process::id() as i32), "ps does not list this test process:\n{text}");
    rows.into_iter().filter(|(_, stat, cmd)| cmd.contains(marker) && !stat.starts_with('Z')).map(|(p, _, c)| (p, c.to_string())).collect()
}

/// SIGKILL to every live process with the marker (what PDEATHSIG does not reach after henki is killed).
pub fn kill_marked(marker: &str) {
    for (pid, _) in marked(marker) {
        // SAFETY: kill(2) with a pid read from /proc; no memory is shared.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// The marked processes still alive after up to 1 s; they are killed before this returns.
fn survivors(marker: &str) -> Vec<(i32, String)> {
    let end = Instant::now() + SEC;
    loop {
        let m = marked(marker);
        if m.is_empty() || Instant::now() > end {
            kill_marked(marker);
            return m;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The state letter of /proc/PID/stat (after the last ')').
#[cfg(target_os = "linux")]
pub fn state(pid: i32) -> Option<char> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    s.rsplit_once(')')?.1.trim_start().chars().next()
}

/// The parent pid of a process.
#[cfg(target_os = "linux")]
pub fn ppid(pid: i32) -> Option<i32> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    s.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// A hex mask line of /proc/PID/status (`SigBlk`, `SigIgn`); 0 when the process is gone.
#[cfg(target_os = "linux")]
pub fn sigmask(pid: i32, field: &str) -> u64 {
    let s = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines().find_map(|l| l.strip_prefix(field)?.strip_prefix(':')).and_then(|h| u64::from_str_radix(h.trim(), 16).ok()).unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn sigblk(pid: i32) -> u64 {
    sigmask(pid, "SigBlk")
}

/// The code points henki escapes on every output path: C0 except tab and newline, DEL, C1, the format and separator
/// characters and the default ignorables. None of them may reach stdout, stderr or the journal raw.
#[rustfmt::skip]
const ESCAPED: &[(u32, u32)] = &[
    (0x00, 0x08), (0x0B, 0x1F), (0x7F, 0x9F),
    (0xA0, 0xA0), (0xAD, 0xAD), (0x34F, 0x34F), (0x600, 0x605), (0x61C, 0x61C), (0x6DD, 0x6DD), (0x70F, 0x70F), (0x890, 0x891), (0x8E2, 0x8E2),
    (0x115F, 0x1160), (0x1680, 0x1680), (0x17B4, 0x17B5), (0x180B, 0x180F), (0x2000, 0x200F), (0x2028, 0x202F), (0x205F, 0x206F), (0x3000, 0x3000),
    (0x3164, 0x3164), (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFFB), (0x110BD, 0x110BD), (0x110CD, 0x110CD),
    (0x13430, 0x1343F), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF),
];

/// The first code point of `b` that henki must never write raw.
pub fn raw_escapable(b: &[u8]) -> Option<char> {
    String::from_utf8_lossy(b).chars().find(|&c| ESCAPED.iter().any(|r| (r.0..=r.1).contains(&(c as u32))))
}

/// `2,941,512`: how numbers appear in text the model sees.
pub fn num(n: usize) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Waits for `cond` up to `within`; false on timeout.
pub fn poll(within: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + within;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// A shell call that leaves a background process ignoring TERM. Any later stop then waits out the grace before its
/// SIGKILL, so henki lives on for a while after it has shut the model socket.
pub fn term_proof(r: &Run) -> Step {
    shell(&format!("trap '' TERM; sleep {} & echo started", r.marker))
}

/// henki shut request `n`'s socket itself, at least 150 ms before it exited (an exit closes every socket anyway).
pub fn closed_before_exit(m: &Mock, n: usize, d: &Done) {
    d.check(m.wait_closed(n, SEC), &format!("the socket of request {} stayed open", n + 1));
    let done = m.log()[n].done.unwrap();
    let ahead = d.exited.saturating_duration_since(done);
    d.check(ahead >= Duration::from_millis(150), &format!("request {}'s socket closed only {ahead:?} before the exit", n + 1));
}

/// A port nothing listens on (bound, then released).
pub fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
