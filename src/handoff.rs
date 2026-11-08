//! `handoff/1`: a typed command, `/NAME REST` (a module's `cmd/NAME`) or `/mods ARGS` (the shipped manager), run on the
//! terminal, and what it hands back in the file `HENKI_NEXT` names: nothing, `send` + TEXT, `resume ID`, `new` or
//! `restart`. No text is sent once a trust file changed; a new henki (new, resume, restart) on trust files the command
//! changed is asked for, naming them; after a change from before the command there is no one-key restart. A `restart`
//! after a change to the history link starts a new session: the old one stays in the store it was kept in.
use crate::{allow, port, sys, term, turn::Session, types::Stop};
use std::{ffi::OsString, fs, io::Read, os::unix::fs::MetadataExt, path::Path, path::PathBuf};

/// What the REPL does after a command.
pub enum Next {
    Stay,
    Tripped,
    /// Shown whole, labelled `[from /NAME]`; Enter sends it, n drops it.
    Send(String),
    /// Close the session as /exit does, then exec this argv.
    Exec(Vec<OsString>),
}

const NEXT_MAX: u64 = 64 << 10;

/// `prog ARGS` on the terminal as `/NAME`: `module` is the module dir (None for the manager). `tripped`: the REPL
/// refuses messages already. Err: TERM, HUP or a closed stdout ended it.
pub fn run(s: &mut Session, name: &str, prog: &Path, module: Option<&Path>, args: &[OsString], tripped: bool) -> Result<Next, Stop> {
    // In the spill dir, so the 24 h sweep takes one a crash left; a dot name never matches a session's spill file.
    let next = s.cfg.state.join("spill").join(format!(".next-{}-{}", std::process::id(), sys::random4().unwrap_or_default()));
    let made = sys::create_excl(&next, 0o600, false).and_then(|f| f.metadata());
    let Ok(made) = made.map_err(|e| s.out().note(&format!("/{name}: cannot make {}: {e}", next.display()))) else { return Ok(Next::Stay) };
    let before = s.changed();
    // A trust file that differed before the command: named once, and never blamed on it.
    before.iter().filter(|_| !tripped).for_each(|p| s.out().note(&format!("trust file changed: {}", p.display())));
    let (pre, env) = (tripped || !before.is_empty(), s.command_env(module, &next));
    let code = s.bang(prog, args, &env).inspect_err(|_| _ = fs::remove_file(&next))?;
    let changed: Vec<PathBuf> = s.changed().into_iter().filter(|p| !before.contains(p)).collect();
    // Only the file henki made, still a regular file, at most 64 KiB of UTF-8; a FIFO, a link or a replaced file is not.
    let mut b = Vec::new();
    let opened = sys::open_read(&next).ok().filter(|(_, m)| m.is_file() && (m.dev(), m.ino()) == (made.dev(), made.ino()));
    let gone = fs::symlink_metadata(&next).is_err(); // removed by the command: nothing handed back
    let ok = gone || opened.is_some_and(|(f, _)| f.take(NEXT_MAX + 1).read_to_end(&mut b).is_ok() && b.len() as u64 <= NEXT_MAX);
    let _ = fs::remove_file(&next);
    changed.iter().for_each(|p| s.out().note(&format!("changed while /{name} ran: {}", allow::shown(p, &s.cfg.home))));
    code.filter(|&c| c != 0).iter().for_each(|c| s.out().note(&format!("/{name} exited {c}")));
    let text = String::from_utf8(b).ok().filter(|_| ok);
    text.is_none().then(|| s.out().note(&format!("/{name}: HENKI_NEXT is not henki's file, over 64 KiB or not UTF-8; ignored")));
    let text = text.unwrap_or_default();
    let (line, body) = text.split_once('\n').unwrap_or((&text, ""));
    let (verb, arg) = line.trim().split_once(' ').map_or((line.trim(), ""), |(v, a)| (v, a.trim()));
    let (stale, to) = (pre || !changed.is_empty(), if changed.is_empty() { "" } else { " on these changes" });
    let stay = if stale { Next::Tripped } else { Next::Stay };
    // A history binding keeps its own sessions: after a change to the history link `restart` starts a new session (a
    // resume there is refused) and says where this one stays.
    let moved = verb == "restart" && changed.contains(&s.cfg.dir.join("port").join("history"));
    let id = if verb == "resume" { Some(arg.to_string()) } else { s.resumable().filter(|_| verb == "restart" && !moved) };
    let old = s.cfg.links.iter().find(|l| l.0 == "history").map_or_else(|| "henki's own history".into(), |l| format!("history binding {}", l.2));
    let kept = |i: &String| format!("session {i} stays in {old}; resume it with that bound (std: henki --port history=std --resume={i})");
    Ok(match verb {
        "" => stay,
        "send" if stale => (s.out().note(&format!("/{name}: a trust file changed; nothing is sent")), Next::Tripped).1,
        "send" if body.contains(term::escaped) => (s.out().note(&format!("/{name} handed back invisible characters; nothing is sent")), stay).1,
        "send" => Next::Send(body.to_string()),
        "new" | "resume" | "restart" if pre => (s.out().note("a trust file changed before; restart henki yourself (henki --resume continues this session)"), Next::Tripped).1,
        // `ID@N` (a fork) waits for a history binding that can fork.
        "resume" if !port::session_id(arg) => (s.out().note(&format!("/{name}: cannot resume {:?}", term::escape(arg))), stay).1,
        "resume" if !s.yes(&format!("switch to session {arg}{to}? [y/N] "))? => stay,
        "new" | "restart" if !changed.is_empty() && !s.yes("restart on these changes? [y/N] ")? => stay,
        "restart" if moved => (s.resumable().iter().for_each(|i| s.out().note(&kept(i))), Next::Exec(again(None))).1,
        "new" | "resume" | "restart" => Next::Exec(again(id)),
        v => (s.out().note(&format!("/{name} handed back {:?}, which henki does not know", term::escape(&v.chars().take(40).collect::<String>()))), stay).1,
    })
}

/// `current_exe()`, then this henki's arguments without `--resume[=ID]` (so `--config`, `--approve`, `--model`,
/// `--nested` and every `--port` stay), then `--resume=ID` when given.
fn again(id: Option<String>) -> Vec<OsString> {
    let exe = std::env::current_exe().map_or_else(|_| OsString::from("henki"), PathBuf::into_os_string);
    let kept = std::env::args_os().skip(1).filter(|a| a.to_str().is_none_or(|a| a != "--resume" && !a.starts_with("--resume=")));
    std::iter::once(exe).chain(kept).chain(id.map(|i| format!("--resume={i}").into())).collect()
}
