//! The line REPL: a banner, a prompt, `/` commands, `!cmd`, and one turn per message, on the terminal henki was
//! started from.
use crate::{allow, config, handoff, handoff::Next, http, supervise, sys, term, turn::Session, turn::outcome_line, types::Outcome, types::Stop};
use std::ffi::OsString;

const HELP: &str = "/exit  quit (also Ctrl-D)
/mods [ARGS]  the module manager on this terminal (henki mods ARGS)
/allow [rm N]  list the allow rules, or remove rule N (henki mods allow)
/help  this list
!CMD  run CMD yourself on this terminal; the model is told you ran it
Enter sends the message; a paste goes into it whole (up to 1 MiB, newlines kept) and is never a command.";
/// The answer to every message once a trust file changed: the session goes on only after a restart reads it again.
const TRIPPED: &str = "a trust file changed; restart henki to go on (henki --resume continues this session)";

/// From the banner to the exit code, and the argv of the henki a command handed back (`handoff`); closes the session on
/// every path.
pub fn run(s: &mut Session) -> (u8, Option<handoff::Again>) {
    // What henki inherited is not its own: from here on every process below it counts, a `!cmd &` job included.
    supervise::reap();
    banner(s);
    let (code, next) = lines(s);
    sys::raw(false); // the modes it started with, also after a `!cmd` that changed them was stopped by TERM or HUP
    // However the REPL found stdout gone (in a turn, at the prompt, during a `!cmd`), it says so once, as a turn would.
    (code == 141).then(|| s.out().line(&outcome_line(&Outcome::OutputClosed)));
    let failed = s.close(None).err();
    failed.iter().for_each(|o| s.out().line(&outcome_line(o)));
    // A session that did not close cleanly is the end: no new henki starts on it.
    if failed.is_some() { (if code == 0 { 2 } else { code }, None) } else { (code, next) }
}

/// What henki talks to, how it approves, where its config is, the extensions, the notices and this root's rules.
fn banner(s: &Session) {
    let (c, out) = (&s.cfg, s.out());
    let url = format!("http://{}{}", http::host_port(&c.url), c.url.path);
    out.line(&format!("henki {} · {} · {url} ({}) · ctx {}", env!("CARGO_PKG_VERSION"), c.model, c.dialect, term::num(c.window)));
    out.line(&if c.auto { format!("approval: AUTO (from {})", c.approve_from) } else { "approval: ask".into() });
    out.line(&c.file.as_ref().map_or_else(|| "config: none (HENKI_MODEL and HENKI_BASE_URL)".into(), |f| format!("config: {}", allow::shown(f, &c.home))));
    (c.exts.is_empty()).then(|| out.line("no modules on; /mods (or henki mods) lists the official mods"));
    for e in &c.exts {
        let none = if e.prompt { "doc only" } else { "no tools" };
        let cmds = if e.cmds.is_empty() { none.into() } else { e.cmds.join(", ") };
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
fn lines(s: &mut Session) -> (u8, Option<handoff::Again>) {
    let (mut notes, mut tripped) = (String::new(), false);
    loop {
        let (line, typed) = match s.line("> ") {
            Ok(Some(l)) => l,
            // A terminal that went away ends the input too, maybe before its SIGHUP arrives: a hangup all the same.
            // `henki serve`'s EOF while idle is the end of the session.
            Ok(None) => return (if sys::isatty(0) || s.out().json() { 0 } else { 130 }, None),
            Err(Stop::Signal(libc::SIGINT | libc::SIGQUIT)) => {
                s.out().line("\n(Ctrl-C does not quit; /exit or Ctrl-D quits)");
                continue;
            }
            Err(e) => return (Outcome::from(e).exit() as u8, None), // a closed stdout, or TERM or HUP
        };
        // Only a line typed alone is a command; a paste, or a line that came with more input pending, is a message.
        let t = line.trim();
        let cmd = if typed { t } else { "" };
        let (verb, rest) = cmd.split_once(' ').unwrap_or((cmd, ""));
        let name = verb.strip_prefix('/').unwrap_or_default();
        // `/mods ARGS` and `/allow ARGS` (`/mods allow ARGS`): the shipped manager, never a module's, its words split at spaces.
        let mods =
            matches!(name, "mods" | "allow").then(|| (name == "allow").then_some("allow").into_iter().chain(rest.split_whitespace()).map(OsString::from).collect::<Vec<_>>());
        // `/NAME REST`: a module's cmd/NAME, REST as one argument.
        let module = s.cfg.exts.iter().find(|x| x.user.iter().any(|u| u == name)).map(|x| x.dir.clone());
        let one: Vec<OsString> = Some(rest.trim()).filter(|r| !r.is_empty()).map(OsString::from).into_iter().collect();
        let note = |s: &Session, m: &str| Ok((s.reject(m), Next::Stay).1);
        let next = match (cmd, mods, module, cmd.strip_prefix('!')) {
            _ if t.is_empty() || cmd == "!" => continue,
            ("/exit", ..) => return (0, None),
            ("/help", ..) => {
                Ok((s.out().line(&(HELP.to_string() + &s.cfg.exts.iter().flat_map(|x| x.user.iter().map(|u| format!("\n/{u}  ({})", x.name))).collect::<String>())), Next::Stay).1)
            }
            (_, Some(args), ..) => match config::which("henki-mods", Some(s.cfg.user_path.as_ref()), true) {
                Some(m) => handoff::run(s, name, &m, None, &args, tripped),
                None => note(s, "/mods needs henki-mods next to henki or on PATH; fix: reinstall henki"),
            },
            (_, _, Some(d), _) => handoff::run(s, name, &d.join("cmd").join(name), Some(&d), &one, tripped),
            _ if cmd.starts_with('/') => note(s, &format!("unknown command {t}; /help lists the commands")),
            (.., Some(cmd)) => terminal(s, cmd, &mut tripped, &mut notes),
            _ if tripped => note(s, TRIPPED),
            _ => Ok(Next::Send(line.clone())),
        };
        let text = match after(s, name, next, &mut tripped) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(end) => return end,
        };
        let o = s.turn(Some(std::mem::take(&mut notes) + &text));
        if !matches!(o, Outcome::Ok | Outcome::OutputClosed) {
            s.out().line(&outcome_line(&o));
        }
        match o {
            Outcome::Interrupted if matches!(s.sig, Some(libc::SIGTERM | libc::SIGHUP)) => return (130, None),
            // stdout went away (only a turn writes to it), also when the turn did not end on it: a finishing turn drops
            // that event.
            _ if s.out().closed() => return (141, None),
            Outcome::Error { kind: "journal" | "tampered", .. } => return (2, None),
            Outcome::Error { kind: "changed", .. } => {
                s.reject(TRIPPED);
                tripped = true;
            }
            _ => {}
        }
    }
}

/// What a command handed back, as the REPL's next step: Ok(Some(TEXT)) a message to send, Ok(None) the next prompt,
/// Err the REPL's end (a new henki's argv for Exec). A typed message is a `Send` too, sent without a question. In
/// `henki serve` a command's text is a `next` record instead, which its frontend sends or not.
fn after(s: &mut Session, name: &str, next: Result<Next, Stop>, tripped: &mut bool) -> Result<Option<String>, (u8, Option<handoff::Again>)> {
    let end = |e: Stop| (Outcome::from(e).exit() as u8, None);
    match next.map_err(end)? {
        Next::Stay => Ok(None),
        Next::Tripped => Ok(None).inspect(|_| *tripped = true),
        Next::Exec(again) => Err((0, Some(again))),
        Next::Send(text) if name.is_empty() => Ok(Some(text)),
        Next::Send(text) if s.out().json() => Ok(None).inspect(|_| s.out().record(&serde_json::json!({"t": "next", "send": text}))),
        Next::Send(text) => {
            s.out().line(&format!("[from /{name}]\n{text}"));
            Ok(s.reply("send? [Y/n] ").map_err(end)?.filter(|a| matches!(a.as_str(), "" | "y" | "yes")).map(|_| text))
        }
    }
}

/// `!CMD` on the terminal, and the note the model gets with the next message; Err when the REPL ends there. Nothing is
/// read again: what it changed applies only after a restart. A trust file that differed before it changed outside it:
/// named once, and never blamed on it.
fn terminal(s: &mut Session, cmd: &str, tripped: &mut bool, notes: &mut String) -> Result<Next, Stop> {
    let (bash, env, args) = (s.cfg.bash.clone(), supervise::bang_env(&s.cfg.env), ["--noprofile", "--norc", "-c", cmd].map(OsString::from));
    let before = s.changed();
    let code = s.bang(&bash, &args, &env)?; // a closed stdout, or TERM or HUP
    let (old, new): (Vec<_>, Vec<_>) = s.changed().into_iter().partition(|p| before.contains(p));
    old.iter().filter(|_| !*tripped).for_each(|p| s.out().note(&format!("trust file changed: {}", p.display())));
    new.iter().for_each(|p| s.out().note(&format!("{} changed by !cmd; restart henki to apply it (henki --resume continues this session)", p.display())));
    *tripped |= !old.is_empty() || !new.is_empty();
    code.iter().for_each(|n| *notes += &format!("[the user ran in their terminal: {cmd} (exit {n})]\n"));
    Ok(Next::Stay)
}
