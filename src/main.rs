//! henki: a small terminal agent. `henki run [--json] TASK` drives a model through a shell and three file tools.
#![allow(dead_code)]

mod allow;
mod anthropic;
mod config;
mod ext;
mod files;
mod http;
mod journal;
mod openai;
mod supervise;
mod sys;
mod term;
mod tools;
mod turn;
mod types;

use crate::{config::Config, config::Env, config::Flags, journal::Journal, journal::rec, term::Out, turn::Session, types::Ev, types::Outcome, types::Usage};
use serde_json::Value;
use std::sync::mpsc::{self, Receiver, Sender};
use std::{io, io::Read, process::ExitCode, time::Duration, time::Instant};

const USAGE: &str = "usage: henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki --version | --help";
/// The most `run -` takes from stdin.
const TASK_MAX: u64 = 1 << 20;

/// The arguments of `run`.
struct Args {
    json: bool,
    flags: Flags,
    resume: Option<Option<String>>,
    nested: bool,
    task: Option<String>,
}

fn main() -> ExitCode {
    let t0 = Instant::now();
    let (tx, rx) = mpsc::channel();
    // First, while this is the only thread: every thread started later inherits the blocked signals.
    let early = sys::signals(tx.clone()).and_then(|()| sys::harden());
    std::panic::set_hook(Box::new(|info| {
        supervise::panic_kill();
        term::panic_line(&info.to_string());
    }));
    let flags = Flags { config: None, approve: None, model: None, max_steps: None, max_seconds: None };
    let mut a = Args { json: false, flags, resume: None, nested: false, task: None };
    let cmd = parse(&mut a);
    let out = Out::start(a.json, tx.clone());
    ExitCode::from(match cmd {
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

/// The command line, parsed before any output: the text of `--version` or `--help`, or None for `run` with its
/// arguments in `a`. On an error, `a.json` says whether `--json` came before the bad argument.
fn parse(a: &mut Args) -> Result<Option<String>, lexopt::Error> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_env();
    let (version, arch, os) = (env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS);
    let text = match p.next()? {
        Some(Long("version")) => format!("henki {version} ({}, {arch}-{os})", option_env!("HENKI_COMMIT").unwrap_or("unknown")),
        Some(Long("help")) => format!("henki {version}: a small terminal agent\n\n{USAGE}"),
        Some(Value(v)) if v == "run" => String::new(),
        Some(arg) => return Err(arg.unexpected()),
        None => return Err("no command given".into()),
    };
    if !text.is_empty() {
        return p.next()?.map_or(Ok(Some(text)), |arg| Err(arg.unexpected()));
    }
    while let Some(arg) = p.next()? {
        match arg {
            Long("json") => a.json = true,
            Long("max-steps") => a.flags.max_steps = Some(p.value()?.parse()?),
            Long("max-seconds") => a.flags.max_seconds = Some(p.value()?.parse()?),
            Long("approve") => match p.value()?.string()?.as_str() {
                m @ ("ask" | "auto") => a.flags.approve = Some(m == "auto"),
                m => return Err(format!("--approve takes ask or auto, not {m}").into()),
            },
            Long("config") => a.flags.config = Some(p.value()?.into()),
            Long("model") => a.flags.model = Some(p.value()?.string()?),
            Long("resume") => a.resume = Some(p.optional_value().map(|v| v.string()).transpose()?),
            Long("nested") => a.nested = true,
            Value(v) if a.task.is_none() => a.task = Some(v.string()?),
            arg => return Err(arg.unexpected()),
        }
    }
    if a.task.is_none() && a.resume.is_none() { Err("missing TASK".into()) } else { Ok(None) }
}

/// `henki run`: startup, one turn, the `result`, the session end.
fn run(a: Args, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, t0: Instant, early: io::Result<()>) -> u8 {
    let mut deadline = None;
    let (cfg, j, hist, task, head) = match start(a, &tx, &rx, t0, early, &mut deadline) {
        Ok(r) => r,
        Err(o) => {
            refuse(&out, &o, t0);
            return code(out, &o, deadline);
        }
    };
    head.iter().for_each(|r| out.record(r));
    let mut s = Session::new(cfg, j, hist, out, tx, rx, deadline);
    // A finished turn whose stdout reader went away never gave the reader what it waited for.
    let o = s.turn(task);
    let o = if matches!(o, Outcome::Ok | Outcome::GaveUp(_) | Outcome::NeedsApproval(_)) && s.out().closed() { Outcome::OutputClosed } else { o };
    let result = s.result(&o, ms(t0));
    if o != Outcome::Ok {
        s.out().line(&outcome_line(&o));
    }
    // A journal failure at the end is the run's end, unless the turn already failed.
    let failed = s.close(Some(result)).err().filter(|_| !matches!(o, Outcome::Error { .. }));
    failed.iter().for_each(|e| s.out().line(&outcome_line(e)));
    code(s.into_out(), &failed.unwrap_or(o), deadline)
}

/// What startup hands to the session: the config, the journal, the history, the task, and the records it journaled (a
/// resume's repairs, then `session`).
type Started = (Config, Journal, Vec<Value>, Option<String>, Vec<Value>);

/// Startup up to the `session` record. An error ends the run before a session exists.
fn start(a: Args, tx: &Sender<Ev>, rx: &Receiver<Ev>, t0: Instant, early: io::Result<()>, deadline: &mut Option<Instant>) -> Result<Started, Outcome> {
    let bad = |m: String| Outcome::error("config", m);
    early.map_err(|e| Outcome::error("internal", e.to_string()))?;
    let env = Env::os();
    let depth = env.get("HENKI_DEPTH").and_then(|d| d.to_str()).unwrap_or_default();
    if !a.nested && depth.parse::<u32>().is_ok_and(|d| d >= 1) {
        return Err(Outcome::error("nested", format!("refusing to start inside henki (HENKI_DEPTH={depth}); pass --nested if this is intended")));
    }
    let cwd = std::env::current_dir().map_err(|e| bad(format!("cannot read the working directory: {e}")))?;
    let mut cfg = config::load(&a.flags, &env, &cwd).map_err(bad)?;
    *deadline = (cfg.max_seconds > 0).then(|| t0 + Duration::from_secs(cfg.max_seconds));
    journal::state_dir(&cfg.state).map_err(bad)?;
    let state = cfg.state.clone();
    cfg.protected.extend([Some(state.clone()), state.canonicalize().ok()].into_iter().flatten());
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
        None => (Journal::create(&state, &cfg.cwd)?, Vec::new(), None, Vec::new()),
        Some(id) => {
            let r = Journal::resume(&state, &cfg.cwd, id.as_deref(), &cfg)?;
            cfg.notices.extend(r.notices);
            (r.journal, r.hist, Some(r.from_bytes), r.repairs)
        }
    };
    head.push(j.write(rec::session(&cfg, j.id(), &cfg.cwd, from))?);
    Ok((cfg, j, hist, task, head))
}

/// `run -`: stdin to EOF on its own thread, so that a signal or the deadline still ends the wait.
fn stdin_task(tx: &Sender<Ev>, rx: &Receiver<Ev>, deadline: Option<Instant>) -> Result<String, Outcome> {
    let tx = tx.clone();
    let read = move || {
        let mut b = Vec::new();
        let ok = io::stdin().lock().take(TASK_MAX + 1).read_to_end(&mut b).is_ok() && b.len() as u64 <= TASK_MAX;
        let _ = tx.send(Ev::Line(ok.then(|| String::from_utf8_lossy(&b).into_owned())));
    };
    std::thread::Builder::new().spawn(read).map_err(|e| Outcome::error("internal", format!("cannot start a thread: {e}")))?;
    loop {
        match deadline.map_or_else(|| rx.recv().ok(), |d| rx.recv_timeout(d.saturating_duration_since(Instant::now())).ok()) {
            Some(Ev::Line(Some(t))) => return Ok(t),
            Some(Ev::Line(None)) => return Err(Outcome::error("config", "the task on stdin is over 1 MiB or unreadable")),
            Some(Ev::Signal(_)) => return Err(Outcome::Interrupted),
            None => return Err(Outcome::Budget),
            Some(_) => {}
        }
    }
}

/// An end before any session exists: the `--json` result with `session: null`, or plain text on stderr.
fn refuse(out: &Out, o: &Outcome, t0: Instant) {
    out.record(&rec::result(o, "", Usage::default(), None, 0, ms(t0)));
    if let Outcome::Error { message, .. } = o { out.note(message) } else { out.line(&outcome_line(o)) }
}

/// The plain-mode line for an end other than `ok`: `(gave up: stuck)`, `(error auth: MSG)`, `(budget)`.
fn outcome_line(o: &Outcome) -> String {
    match o {
        Outcome::GaveUp(r) => format!("(gave up: {r})"),
        Outcome::Error { kind, message } => format!("(error {kind}: {message})"),
        Outcome::NeedsApproval(c) => format!("(needs approval: {})", c.name),
        o => format!("({})", o.status().replace('_', " ")),
    }
}

/// The bounded flush, then the exit code: 141 when a run that finished did not get all its output out, its `result`
/// included, because stdout closed or its reader did not keep up.
fn code(out: Out, o: &Outcome, deadline: Option<Instant>) -> u8 {
    let until = deadline.map_or_else(|| Instant::now() + Duration::from_secs(5), |d| d + Duration::from_millis(950));
    let closed = out.flush(until);
    if closed && matches!(o, Outcome::Ok | Outcome::GaveUp(_) | Outcome::NeedsApproval(_)) { 141 } else { o.exit() as u8 }
}

fn ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}
