//! The line REPL: a banner, a prompt, `/` commands, `!cmd`, and one turn per message, on the terminal henki was
//! started from.
use crate::{allow, http, supervise, sys, term, turn::Session, turn::outcome_line, types::Outcome, types::Stop};

const HELP: &str = "/exit  quit (also Ctrl-D)
/allow  list the allow rules; /allow rm N removes rule N
/help  this list
!CMD  run CMD yourself on this terminal; the model is told you ran it
One line is one message; for longer text, save it to a file and name the file.";
/// The answer to every message once a trust file changed: the session goes on only after a restart reads it again.
const TRIPPED: &str = "a trust file changed; restart henki to go on (henki --resume continues this session)";

/// From the banner to the exit code; closes the session on every path.
pub fn run(s: &mut Session) -> u8 {
    // What henki inherited is not its own: from here on every process below it counts, a `!cmd &` job included.
    supervise::reap();
    banner(s);
    let code = lines(s);
    // However the REPL found stdout gone (in a turn, at the prompt, during a `!cmd`), it says so once, as a turn would.
    (code == 141).then(|| s.out().line(&outcome_line(&Outcome::OutputClosed)));
    let failed = s.close(None).err();
    failed.iter().for_each(|o| s.out().line(&outcome_line(o)));
    if failed.is_some() && code == 0 { 2 } else { code }
}

/// What henki talks to, how it approves, where its config is, the extensions, the notices and this root's rules.
fn banner(s: &Session) {
    let (c, out) = (&s.cfg, s.out());
    let url = format!("http://{}{}", http::host_port(&c.url), c.url.path);
    out.line(&format!("henki {} · {} · {url} ({}) · ctx {}", env!("CARGO_PKG_VERSION"), c.model, c.dialect, term::num(c.window)));
    out.line(&if c.auto { format!("approval: AUTO (from {})", c.approve_from) } else { "approval: ask".into() });
    out.line(&c.file.as_ref().map_or_else(|| "config: none (HENKI_MODEL and HENKI_BASE_URL)".into(), |f| format!("config: {}", allow::shown(f, &c.home))));
    for e in &c.exts {
        let cmds = if e.cmds.is_empty() { "doc only".into() } else { e.cmds.join(", ") };
        let shadows = e.shadows.iter().map(|p| allow::shown(p, &c.home)).collect::<Vec<_>>().join(", ");
        out.line(&format!("ext {}: {cmds}{}{shadows}", e.name, if shadows.is_empty() { "" } else { "; shadows " }));
    }
    if c.window < 16_384 {
        let fix = "fix: start llama-server with -c 32768 (Ollama: OLLAMA_CONTEXT_LENGTH=32768) and set context_window = 32768";
        out.note(&format!("context_window is {} tokens, too small for most tasks; {fix}", term::num(c.window)));
    }
    c.notices.iter().for_each(|n| out.note(n));
    // Every rule that can run a command here without asking, each time henki starts.
    c.allow.rules.iter().filter(|r| allow::applies(r, &c.root)).for_each(|r| out.line(&format!("allow:   {}  {}", r.n, r.cmd)));
}

/// IDLE and TRIPPED: one line at a time, until the exit code. TRIPPED (a trust file changed) refuses messages.
fn lines(s: &mut Session) -> u8 {
    let (mut notes, mut tripped) = (String::new(), false);
    loop {
        s.out().prompt("> ");
        let line = match s.line() {
            Ok(Some(line)) => line,
            Ok(None) => {
                s.out().line(""); // Ctrl-D leaves the cursor after the prompt
                // A terminal that went away ends the input too, maybe before its SIGHUP arrives: a hangup all the same.
                return if sys::isatty(0) { 0 } else { 130 };
            }
            Err(Stop::Signal(libc::SIGINT | libc::SIGQUIT)) => {
                s.out().line("\n(Ctrl-C does not quit; /exit or Ctrl-D quits)");
                continue;
            }
            Err(e) => return Outcome::from(e).exit() as u8, // a closed stdout, or TERM or HUP
        };
        let t = line.trim();
        // One line per prompt: the rest of a pasted block, or of what was typed ahead, is dropped with a note, so that no
        // pasted line runs as a `!cmd`. What follows a `!cmd` line is the command's to read.
        let dropped = if t.starts_with('!') { 0 } else { sys::flush_input() };
        (dropped > 0).then(|| s.out().note(&format!("dropped {} bytes after that line: one line is one message; save longer text to a file", term::num(dropped as u64))));
        let rm = t.strip_prefix("/allow rm ").and_then(|n| n.trim().parse().ok());
        match (t, rm, t.strip_prefix('!')) {
            ("" | "!", ..) => {}
            ("/exit", ..) => return 0,
            ("/help", ..) => s.out().line(HELP),
            ("/allow", ..) => allow::list(&s.cfg.allow.file, &s.cfg.home).iter().for_each(|l| s.out().line(l)),
            (_, Some(n), _) => tripped |= remove(s, n),
            _ if t.starts_with('/') => s.out().note(&format!("unknown command {t}; /help lists the commands")),
            (_, _, Some(cmd)) => {
                let before = s.changed();
                let done = s.bang(cmd);
                sys::flush_input(); // a command that left the terminal raw or without echo does not leave it so
                match done {
                    Ok(code) => code.iter().for_each(|n| notes += &format!("[the user ran in their terminal: {cmd} (exit {n})]\n")),
                    Err(e) => return Outcome::from(e).exit() as u8, // a closed stdout, or TERM or HUP
                }
                // Nothing is read again: what the command changed applies only after a restart. A file that differed
                // before it changed outside it: named once, as `/allow rm` names it, and never blamed on the command.
                let (old, new): (Vec<_>, Vec<_>) = s.changed().into_iter().partition(|p| before.contains(p));
                old.iter().filter(|_| !tripped).for_each(|p| s.out().note(&format!("trust file changed: {}", p.display())));
                new.iter().for_each(|p| s.out().note(&format!("{} changed by !cmd; restart henki to apply it (henki --resume continues this session)", p.display())));
                tripped |= !old.is_empty() || !new.is_empty();
            }
            _ if tripped => s.out().note(TRIPPED),
            _ => {
                let o = s.turn(Some(std::mem::take(&mut notes) + &line));
                if !matches!(o, Outcome::Ok | Outcome::OutputClosed) {
                    s.out().line(&outcome_line(&o));
                }
                match o {
                    Outcome::Interrupted if matches!(s.sig, Some(libc::SIGTERM | libc::SIGHUP)) => return 130,
                    // stdout went away (only a turn writes to it), also when the turn did not end on it: a finishing turn
                    // drops that event.
                    _ if s.out().closed() => return 141,
                    Outcome::Interrupted => _ = sys::flush_input(), // what was typed during the turn is no answer
                    Outcome::Error { kind: "journal" | "tampered", .. } => return 2,
                    Outcome::Error { kind: "changed", .. } => {
                        s.out().note(TRIPPED);
                        tripped = true;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// `/allow rm N`; true when it leaves the REPL TRIPPED. henki's own writes re-arm the tripwire, so a change it finds is
/// an outside one: nothing is removed, and a restart shows the rules as they are.
fn remove(s: &mut Session, n: usize) -> bool {
    let changed = s.changed();
    changed.iter().for_each(|p| s.out().note(&format!("trust file changed: {}", p.display())));
    if !changed.is_empty() {
        return true;
    }
    match allow::remove(&s.cfg.allow.file, n, &s.cfg.home) {
        Ok(line) => s.out().line(&line),
        Err(e) => s.out().note(&e),
    }
    let loaded = allow::load(&s.cfg.allow.file, !s.cfg.auto);
    s.rearm();
    loaded.map(|a| s.cfg.allow = a).map_err(|e| s.out().note(&e)).is_err()
}
