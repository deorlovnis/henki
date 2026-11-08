//! henki: a small terminal agent. `henki` is a line REPL on the terminal it was started from; `henki run [--json] TASK`
//! does one task headless; `henki serve` is the REPL's session for a frontend program. All drive a model through a
//! shell and three file tools.

mod allow;
mod anthropic;
mod config;
mod ext;
mod files;
mod handoff;
mod http;
mod journal;
mod observe;
mod openai;
mod port;
mod remote;
mod repl;
mod serve;
mod supervise;
mod sys;
mod term;
mod tools;
mod turn;
mod types;

use crate::types::{Ev, Outcome, Usage};
use crate::{config::Config, config::Env, config::Flags, journal::rec, port::Ports, term::Mode, term::Out, term::Reader, turn::Session, turn::outcome_line};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver, Sender};
use std::{ffi::OsString, io, io::Read, path::PathBuf, process::ExitCode, time::Duration, time::Instant};

const USAGE: &str = "usage: henki [--config FILE] [--approve ask|auto] [--model NAME] [--resume[=ID[@N]]] [--port PORT=std] [--nested]
       henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID[@N]]] [--port PORT=std] [--nested] [TASK | -]
       henki serve [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID[@N]]] [--port PORT=std] [--nested]
       henki mods [ARGS] | henki allow [rm N]
       henki --version | --help";

/// The arguments of the REPL (`mode` Repl), `run` (Plain or Json), `serve` (Plain, its output Json) and `mods`.
struct Args {
    mode: Mode,
    serve: bool,
    flags: Flags,
    resume: Option<Option<String>>,
    nested: bool,
    task: Option<String>,
    /// `henki mods ARGS`, and `henki allow ARGS` as `henki mods allow ARGS`: the manager's arguments, untouched.
    mods: Option<Vec<OsString>>,
    /// `henki port PORT [--offer V]... -- PROGRAM`: the port, the versions offered, the program.
    port: Option<(&'static str, Vec<u64>, PathBuf)>,
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
    let mut a = Args { mode: Mode::Plain, serve: false, flags: Flags::default(), resume: None, nested: false, task: None, mods: None, port: None };
    let cmd = parse(&mut a);
    let out = Out::start(if a.serve { Mode::Json } else { a.mode }, tx.clone());
    ExitCode::from(match cmd {
        Ok(None) => match (a.mods.take(), a.port.take()) {
            (Some(args), _) => mods(args, out),
            (_, Some(p)) => pipe(p, out, (tx, rx)),
            _ => run(a, out, tx, rx, t0, early),
        },
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
        Some(Value(v)) if v == "mods" || v == "allow" => {
            a.mods = Some((v == "allow").then_some(v).into_iter().chain(p.raw_args()?).collect());
            return Ok(None);
        }
        Some(Value(v)) if v == "port" => {
            let name = p.value()?.string()?;
            let port = port::LINKED[..3].iter().find(|k| **k == name).copied().ok_or(format!("henki port takes {}, not {name}", port::LINKED[..3].join(" or ")))?;
            let (mut offer, mut exe) = (Vec::new(), None);
            while let Some(arg) = p.next()? {
                match arg {
                    Long("offer") => offer.push(p.value()?.parse()?),
                    Value(e) if exe.is_none() => exe = Some(PathBuf::from(e)),
                    arg => return Err(arg.unexpected()),
                }
            }
            a.port = Some((port, if offer.is_empty() { vec![1] } else { offer }, exe.ok_or("henki port needs -- PROGRAM")?));
            return Ok(None);
        }
        Some(Value(v)) if v == "run" || v == "serve" => (String::new(), a.serve = v == "serve").0,
        // The REPL, whose flags start at the first argument.
        _ => {
            (p, a.mode) = (lexopt::Parser::from_env(), Mode::Repl);
            String::new()
        }
    };
    if !text.is_empty() {
        return p.next()?.map_or(Ok(Some(text)), |arg| Err(arg.unexpected()));
    }
    let run = a.mode != Mode::Repl && !a.serve;
    while let Some(arg) = p.next()? {
        match arg {
            Long("config") => a.flags.config = Some(p.value()?.into()),
            Long("json") if run => a.mode = Mode::Json,
            Long("max-steps") if run => a.flags.max_steps = Some(p.value()?.parse()?),
            Long("max-seconds") if run => a.flags.max_seconds = Some(p.value()?.parse()?),
            Long("approve") => match p.value()?.string()?.as_str() {
                m @ ("ask" | "auto") => a.flags.approve = Some(m == "auto"),
                m => return Err(format!("--approve takes ask or auto, not {m}").into()),
            },
            Long("model") => a.flags.model = Some(p.value()?.string()?),
            Long("resume") => a.resume = Some(p.optional_value().map(|v| v.string()).transpose()?),
            Long("nested") => a.nested = true,
            Long("port") => match p.value()?.string()?.split_once('=') {
                Some((n, "std")) if port::PORTS.contains(&n) => a.flags.ports.push(n.into()),
                _ => return Err(format!("--port takes PORT=std, PORT one of {}", port::PORTS.join(", ")).into()),
            },
            Value(v) if run && a.task.is_none() => a.task = Some(v.string()?),
            arg => return Err(arg.unexpected()),
        }
    }
    if run && a.task.is_none() && a.resume.is_none() { Err("missing TASK".into()) } else { Ok(None) }
}

/// `henki mods ARGS`: henki becomes the shipped manager, with its environment plus HENKI_BIN, and the signals it blocks
/// and ignores back to their defaults. No config is read and no nested check runs, so it repairs a broken setup, from
/// inside henki too. Returns only on failure.
fn mods(args: Vec<OsString>, out: Out) -> u8 {
    let mut env = Env::os();
    let found = config::which("henki-mods", env.get("PATH"), true);
    env.0.push(("HENKI_BIN".into(), std::env::current_exe().unwrap_or_default().into()));
    let e = found.map_or_else(|| "not found".into(), |m| supervise::exec(&m, &args, &env.0).to_string());
    let e = format!("henki mods needs henki-mods next to henki or on PATH: {e}; fix: reinstall henki");
    out.note(&e);
    code(out, &Outcome::error("config", e), None)
}

/// `henki port` (`remote::pipe`): no config is read and no nested check runs, because it is not a session. The
/// binding gets the tool environment without extensions and the state dir `STATE/port/PORT`.
fn pipe((port, offer, exe): (&'static str, Vec<u64>, PathBuf), out: Out, ch: (Sender<Ev>, Receiver<Ev>)) -> u8 {
    let (env, home) = (Env::os(), std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default());
    let (path, state) = (env.get("PATH").unwrap_or_default().to_string_lossy(), config::xdg(&env, &home, "XDG_STATE_HOME", ".local/state").join("port").join(port));
    let o = remote::pipe(port, &exe, &state, &supervise::child_env(&env, &[], &path, &path, ""), &offer, &out, ch);
    _ = (0..5).all(|_| supervise::sweep_pass() > 0); // what the ended binding left (Linux: its orphans are henki's children)
    code(out, &o, None)
}

/// `henki run` (startup, one turn, the `result`, the session end) and the REPL and `serve` (startup, then `repl::run`).
fn run(a: Args, out: Out, tx: Sender<Ev>, rx: Receiver<Ev>, t0: Instant, early: io::Result<()>) -> u8 {
    let (mut deadline, repl, serve) = (None, a.mode == Mode::Repl || a.serve, a.serve);
    let (cfg, ports, sid, hist, task, mut head, reader) = match start(a, &tx, &rx, t0, early, &mut deadline) {
        Ok(r) => r,
        Err(o) => {
            // The bindings started before the refusal are ended (bye, kill); then what they left, as at a session end.
            _ = (0..5).all(|_| supervise::sweep_pass() > 0);
            refuse(&out, &o, t0);
            return code(out, &o, deadline);
        }
    };
    head.iter().for_each(|r| out.record(r));
    // serve's command reader starts once the `session` record is out, so a line sent early is read after it.
    let reader = if serve { serve::reader(tx.clone()) } else { reader };
    let mut s = Session::new(cfg, ports, sid, hist, out, (tx, rx), deadline, reader);
    s.opened = head.pop(); // the `session` record, for `session_start`
    if repl {
        let (code, next) = repl::run(&mut s);
        let out = s.into_out();
        // serve hands a new henki's argv to its frontend, which starts it once this one exits.
        next.iter().filter(|_| serve).for_each(|n| out.record(&json!({"t": "next", "argv": n.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>()})));
        out.flush(Instant::now() + Duration::from_secs(5));
        // A command handed back a new henki (`handoff`): this one is closed, its bindings ended, so it becomes that one.
        let Some((exe, args)) = next.as_deref().filter(|_| !serve).and_then(<[OsString]>::split_first) else { return code };
        let e = supervise::exec(exe.as_ref(), args, &Env::os().0);
        term::panic_line(&format!("cannot start {}: {e}", exe.to_string_lossy()));
        return 2;
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

/// What startup hands to the session: the config, the bindings, the session id, the history, the task, the records it
/// journaled (a resume's repairs, then `session`), and the REPL's terminal reader.
type Started = (Config, Ports, String, Vec<Value>, Option<String>, Vec<Value>, Option<Reader>);

/// Startup up to the `session` record. An error ends the run before a session exists.
fn start(a: Args, tx: &Sender<Ev>, rx: &Receiver<Ev>, t0: Instant, early: io::Result<()>, deadline: &mut Option<Instant>) -> Result<Started, Outcome> {
    let bad = |m: String| Outcome::error("config", m);
    early.map_err(|e| Outcome::error("internal", e.to_string()))?;
    let env = Env::os();
    let depth = env.get("HENKI_DEPTH").and_then(|d| d.to_str()).unwrap_or_default();
    if !a.nested && depth.parse::<u32>().is_ok_and(|d| d >= 1) {
        return Err(Outcome::error("nested", format!("refusing to start inside henki (HENKI_DEPTH={depth}); pass --nested if this is intended")));
    }
    // The REPL reads the terminal it was started on, and shows what is typed on it through stderr; `tty_save` keeps its
    // modes, which it puts back after every line.
    if a.mode == Mode::Repl && !(sys::isatty(2) && sys::tty_save()) {
        return Err(bad("the REPL needs a terminal; use henki run".into()));
    }
    let no_thread = |e: io::Error| Outcome::error("internal", format!("cannot start a thread: {e}"));
    let reader = (a.mode == Mode::Repl).then(|| Reader::with(io::stdin(), tx.clone())).transpose().map_err(no_thread)?;
    let cwd = std::env::current_dir().map_err(|e| bad(format!("cannot read the working directory: {e}")))?;
    let mut cfg = config::load(&a.flags, &env, &cwd).map_err(bad)?;
    // The REPL and serve have no time budget: Ctrl-C ends a turn, and `max_steps` bounds it.
    *deadline = (a.mode != Mode::Repl && !a.serve && cfg.max_seconds > 0).then(|| t0 + Duration::from_secs(cfg.max_seconds));
    journal::state_dir(&cfg.state).map_err(bad)?;
    let task = if a.task.as_deref() == Some("-") { Some(stdin_task(tx, rx, *deadline)?) } else { a.task };
    if task.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err(bad("task is empty".into()));
    }
    cfg.context = ext::sources(&mut cfg, tx, rx)?;
    // A signal or the deadline that came during startup ends it here, before the session file exists.
    if let Ok(Ev::Signal(_)) = rx.try_recv() {
        return Err(Outcome::Interrupted);
    }
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return Err(Outcome::Budget);
    }
    let mut ports = port::bind(&mut cfg, *deadline)?;
    let (sid, hist, mut head, notices) = journal::start(ports.history.as_mut(), &a.resume, &mut cfg)?;
    cfg.notices.extend(notices);
    head.push(journal::put(ports.history.as_mut(), &sid, rec::session(&cfg, &sid, &cfg.cwd, a.resume.is_some(), &ports.bound))?);
    Ok((cfg, ports, sid, hist, task, head, reader))
}

/// `run -`: stdin to EOF on its own thread, so that a signal or the deadline still ends the wait.
fn stdin_task(tx: &Sender<Ev>, rx: &Receiver<Ev>, deadline: Option<Instant>) -> Result<String, Outcome> {
    let tx = tx.clone();
    let read = move || {
        let mut b = Vec::new();
        let ok = io::stdin().lock().take(term::LINE_MAX as u64 + 1).read_to_end(&mut b).is_ok() && b.len() <= term::LINE_MAX;
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
