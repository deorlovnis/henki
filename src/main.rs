//! henki: a small terminal agent. `henki` is a line REPL on the terminal it was started from; `henki run [--json] TASK`
//! does one task headless. Both drive a model through a shell and three file tools.

mod allow;
mod anthropic;
mod config;
mod ext;
mod files;
mod http;
mod journal;
mod openai;
mod repl;
mod supervise;
mod sys;
mod term;
mod tools;
mod turn;
mod types;

use crate::types::{Ev, Outcome, Usage};
use crate::{config::Config, config::Env, config::Flags, journal::Journal, journal::rec, term::Mode, term::Out, term::Reader, turn::Session, turn::outcome_line};
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, Sender};
use std::{io, io::Read, path::PathBuf, process::ExitCode, time::Duration, time::Instant};

const USAGE: &str = "usage: henki [--config FILE] [--approve ask|auto] [--model NAME] [--resume[=ID]] [--nested]
       henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki allow [--config FILE] [rm N]
       henki --version | --help";
/// The most `run -` takes from stdin.
const TASK_MAX: u64 = 1 << 20;

/// The arguments of the REPL (`mode` Repl), `run` (Plain or Json) and `allow`.
struct Args {
    mode: Mode,
    allow: bool,
    flags: Flags,
    resume: Option<Option<String>>,
    nested: bool,
    task: Option<String>,
    rm: Option<usize>,
}

fn main() -> ExitCode {
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel();
    // First, while this is the only thread: every thread started later inherits the blocked signals.
    let early = sys::signals(tx.clone()).and_then(|()| sys::harden());
    std::panic::set_hook(Box::new(|info| {
        supervise::panic_kill();
        sys::raw(false); // the terminal modes the REPL started with
        term::panic_line(&info.to_string());
    }));
    let mut a = Args { mode: Mode::Plain, allow: false, flags: Flags::default(), resume: None, nested: false, task: None, rm: None };
    let cmd = parse(&mut a);
    let out = Out::start(a.mode, tx.clone());
    ExitCode::from(match cmd {
        Ok(None) if a.allow => allow(a, out),
        Ok(None) => run(a, out, tx, rx, t0, early),
        Ok(Some(text)) => {
            out.answer(&text);
            code(out, &Outcome::Ok, None)
        }
        Err(e) => {
            let o = Outcome::error("config", e.to_string());
            refuse(&out, &o, t0);
            out.line(USAGE);
            code(out, &o, None)
        }
    })
}

/// The command line, parsed before any output: the text of `--version` or `--help`, or None for the command `a` names
/// with its arguments. On an error, `a.mode` says whether `--json` came before the bad argument.
fn parse(a: &mut Args) -> Result<Option<String>, lexopt::Error> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_env();
    let (version, arch, os) = (env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS);
    let text = match p.next()? {
        Some(Long("version")) => format!("henki {version} ({}, {arch}-{os})", option_env!("HENKI_COMMIT").unwrap_or("unknown")),
        Some(Long("help")) => format!("henki {version}: a small terminal agent\n\n{USAGE}"),
        Some(Value(v)) if v == "run" || v == "allow" => {
            a.allow = v == "allow";
            String::new()
        }
        // The REPL, whose flags start at the first argument.
        _ => {
            (p, a.mode) = (lexopt::Parser::from_env(), Mode::Repl);
            String::new()
        }
    };
    if !text.is_empty() {
        return p.next()?.map_or(Ok(Some(text)), |arg| Err(arg.unexpected()));
    }
    let (run, session) = (a.mode != Mode::Repl && !a.allow, !a.allow);
    while let Some(arg) = p.next()? {
        match arg {
            Long("config") => a.flags.config = Some(p.value()?.into()),
            Long("json") if run => a.mode = Mode::Json,
            Long("max-steps") if run => a.flags.max_steps = Some(p.value()?.parse()?),
            Long("max-seconds") if run => a.flags.max_seconds = Some(p.value()?.parse()?),
            Long("approve") if session => match p.value()?.string()?.as_str() {
                m @ ("ask" | "auto") => a.flags.approve = Some(m == "auto"),
                m => return Err(format!("--approve takes ask or auto, not {m}").into()),
            },
            Long("model") if session => a.flags.model = Some(p.value()?.string()?),
            Long("resume") if session => a.resume = Some(p.optional_value().map(|v| v.string()).transpose()?),
            Long("nested") if session => a.nested = true,
            Value(v) if run && a.task.is_none() => a.task = Some(v.string()?),
            Value(v) if !session && v == "rm" && a.rm.is_none() => a.rm = Some(p.value()?.parse()?),
            arg => return Err(arg.unexpected()),
        }
    }
    if run && a.task.is_none() && a.resume.is_none() { Err("missing TASK".into()) } else { Ok(None) }
}

/// `henki allow [--config FILE] [rm N]`: lists the rules of the allow file in the config dir, or removes one. It loads
/// no config, so it works on a broken config or allow file, and inside henki too.
fn allow(a: Args, out: Out) -> u8 {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = a.flags.config.unwrap_or_else(|| home.join(".config/henki/config.toml"));
    let file = std::env::current_dir().unwrap_or_default().join(config).with_file_name("allow");
    // As for the config: a HOME that is not absolute would name a file below the working directory, and show `~` wrong.
    let home = Some(home).filter(|h| h.is_absolute()).ok_or_else(|| "HOME is not set; fix: set HOME to your home directory".to_string());
    let o = match home.and_then(|h| a.rm.map_or_else(|| Ok(allow::list(&file, &h)), |n| allow::remove(&file, n, &h).map(|l| vec![l]))) {
        Ok(lines) => {
            lines.iter().for_each(|l| out.answer(l));
            Outcome::Ok
        }
        Err(e) => {
            out.note(&e);
            Outcome::error("config", e)
        }
    };
    code(out, &o, None)
}

/// `henki run` (startup, one turn, the `result`, the session end) and the REPL (startup, then `repl::run`).
fn run(a: Args, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, t0: Instant, early: io::Result<()>) -> u8 {
    let (mut deadline, repl) = (None, a.mode == Mode::Repl);
    let (cfg, j, hist, task, head, reader) = match start(a, &tx, &rx, t0, early, &mut deadline) {
        Ok(r) => r,
        Err(o) => {
            refuse(&out, &o, t0);
            return code(out, &o, deadline);
        }
    };
    head.iter().for_each(|r| out.record(r));
    let mut s = Session::new(cfg, j, hist, out, tx, rx, deadline, reader);
    if repl {
        let code = repl::run(&mut s);
        s.into_out().flush(Instant::now() + Duration::from_secs(5));
        return code;
    }
    // A finished turn whose stdout reader went away never gave the reader what it waited for.
    let o = s.turn(task);
    let o = if o.finished() && s.out().closed() { Outcome::OutputClosed } else { o };
    let result = s.result(&o, t0.elapsed().as_millis() as u64);
    if o != Outcome::Ok {
        s.out().line(&outcome_line(&o));
    }
    // A journal failure at the end is the run's end, unless the turn already failed.
    let failed = s.close(Some(result)).err().filter(|_| !matches!(o, Outcome::Error { .. }));
    failed.iter().for_each(|e| s.out().line(&outcome_line(e)));
    code(s.into_out(), &failed.unwrap_or(o), deadline)
}

/// What startup hands to the session: the config, the journal, the history, the task, the records it journaled (a
/// resume's repairs, then `session`), and the REPL's line reader.
type Started = (Config, Journal, Vec<Value>, Option<String>, Vec<Value>, Option<Reader>);

/// Startup up to the `session` record. An error ends the run before a session exists.
fn start(a: Args, tx: &Sender<Ev>, rx: &Receiver<Ev>, t0: Instant, early: io::Result<()>, deadline: &mut Option<Instant>) -> Result<Started, Outcome> {
    let bad = |m: String| Outcome::error("config", m);
    early.map_err(|e| Outcome::error("internal", e.to_string()))?;
    let env = Env::os();
    let depth = env.get("HENKI_DEPTH").and_then(|d| d.to_str()).unwrap_or_default();
    if !a.nested && depth.parse::<u32>().is_ok_and(|d| d >= 1) {
        return Err(Outcome::error("nested", format!("refusing to start inside henki (HENKI_DEPTH={depth}); pass --nested if this is intended")));
    }
    // The REPL reads the terminal it was started on; `tty_save` keeps its modes, which it puts back after every line.
    if a.mode == Mode::Repl && !sys::tty_save() {
        return Err(bad("the REPL needs a terminal; use henki run".into()));
    }
    let no_thread = |e: io::Error| Outcome::error("internal", format!("cannot start a thread: {e}"));
    let reader = (a.mode == Mode::Repl).then(|| Reader::with(Box::new(io::stdin()), tx.clone())).transpose().map_err(no_thread)?;
    let cwd = std::env::current_dir().map_err(|e| bad(format!("cannot read the working directory: {e}")))?;
    let mut cfg = config::load(&a.flags, &env, &cwd).map_err(bad)?;
    // The REPL has no time budget: Ctrl-C ends a turn, and `max_steps` bounds it.
    *deadline = (a.mode != Mode::Repl && cfg.max_seconds > 0).then(|| t0 + Duration::from_secs(cfg.max_seconds));
    journal::state_dir(&cfg.state).map_err(bad)?;
    let task = if a.task.as_deref() == Some("-") { Some(stdin_task(tx, rx, *deadline)?) } else { a.task };
    if task.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(bad("task is empty".into()));
    }
    // A signal or the deadline that came during startup ends it here, before the session file exists.
    if let Ok(Ev::Signal(_)) = rx.try_recv() {
        return Err(Outcome::Interrupted);
    }
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return Err(Outcome::Budget);
    }
    let (mut j, hist, from, mut head) = match &a.resume {
        None => (Journal::create(&cfg.state, &cfg.cwd)?, Vec::new(), None, Vec::new()),
        Some(id) => {
            let r = Journal::resume(&cfg.state, &cfg.cwd, id.as_deref(), &cfg)?;
            cfg.notices.extend(r.notices);
            (r.journal, r.hist, Some(r.from_bytes), r.repairs)
        }
    };
    head.push(j.write(rec::session(&cfg, j.id(), &cfg.cwd, from))?);
    Ok((cfg, j, hist, task, head, reader))
}

/// `run -`: stdin to EOF on its own thread, so that a signal or the deadline still ends the wait.
fn stdin_task(tx: &Sender<Ev>, rx: &Receiver<Ev>, deadline: Option<Instant>) -> Result<String, Outcome> {
    let tx = tx.clone();
    let read = move || {
        let mut b = Vec::new();
        let ok = io::stdin().lock().take(TASK_MAX + 1).read_to_end(&mut b).is_ok() && b.len() as u64 <= TASK_MAX;
        let _ = tx.send(Ev::Input { tag: 0, bytes: ok.then_some(b) });
    };
    std::thread::Builder::new().spawn(read).map_err(|e| Outcome::error("internal", format!("cannot start a thread: {e}")))?;
    loop {
        match deadline.map_or_else(|| rx.recv().ok(), |d| rx.recv_timeout(d.saturating_duration_since(Instant::now())).ok()) {
            Some(Ev::Input { bytes: Some(b), .. }) => return Ok(String::from_utf8_lossy(&b).into_owned()),
            Some(Ev::Input { bytes: None, .. }) => return Err(Outcome::error("config", "the task on stdin is over 1 MiB or unreadable")),
            Some(Ev::Signal(_)) => return Err(Outcome::Interrupted),
            None => return Err(Outcome::Budget),
            Some(_) => {}
        }
    }
}

/// An end before any session exists: the `--json` result with `session: null`, or plain text on stderr.
fn refuse(out: &Out, o: &Outcome, t0: Instant) {
    out.record(&rec::result(o, "", Usage::default(), None, 0, t0.elapsed().as_millis() as u64));
    if let Outcome::Error { message, .. } = o { out.note(message) } else { out.line(&outcome_line(o)) }
}

/// The bounded flush, then the exit code: 141 when a run that finished did not get all its output out, its `result`
/// included, because stdout closed or its reader did not keep up.
fn code(out: Out, o: &Outcome, deadline: Option<Instant>) -> u8 {
    let until = deadline.map_or_else(|| Instant::now() + Duration::from_secs(5), |d| d + Duration::from_millis(950));
    let closed = out.flush(until);
    if closed && o.finished() { 141 } else { o.exit() as u8 }
}
