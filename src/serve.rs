//! `henki serve` (front/1): the REPL's session for a frontend program, which talks to the person. henki reads commands
//! on stdin, one JSON object per line (`send`, `answer`, `command`, `cancel`), and writes the json/1 records on stdout as
//! `run --json` does, plus `reject` for a line it does not take, `command_output` and `next`. A question is the `ask`
//! record; `answer` names its call. The REPL's loop runs it (repl::run): a `send` is a message that is never a command,
//! as a paste is, and a `command` is the line `/NAME ARGS` typed at the prompt; the banner and the terminal lines write
//! nothing in json mode. stdin's EOF ends the session once the lines before it are taken: idle, or once a command
//! ends, it closes (exit 0); in a turn it is a hangup (the turn is interrupted, exit 130). `henki serve --resume`
//! continues the session.
use crate::{sys, term, term::Reader, turn::Session, types::Ev};
use serde_json::Value;
use std::{io::BufRead, io::BufReader, io::Read, sync::atomic::Ordering::SeqCst, sync::mpsc::Sender};

/// The tag of every command line on MAIN's channel, above any tag a session takes: MAIN takes a command by waiting on
/// it, idle or at a question. A line that comes while a turn runs is stale and dropped.
pub const FRONT: u32 = u32::MAX;

/// The command reader: stdin line by line, each at most term::LINE_MAX. `cancel` is a Ctrl-C at once (counted in
/// `sys::SEEN`, so a binding's wait ends too); any other line goes to MAIN as `Ev::Input { tag: FRONT }`; EOF is an
/// `Input` without bytes, in order after them. A line over 1 MiB is read to its end and is one reject. The Reader it returns
/// never reads: it marks the session as one with a person behind it, so questions are asked. None when the thread cannot
/// start: the session then ends at once. Once MAIN is gone nothing more is sent (the process is ending).
pub fn reader(tx: Sender<Ev>) -> Option<Reader> {
    let read = move || {
        let (mut r, mut line) = (BufReader::new(std::io::stdin().lock()), Vec::new());
        while (&mut r).take(term::LINE_MAX as u64 + 1).read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            (line.len() > term::LINE_MAX && line.last() != Some(&b'\n') && r.skip_until(b'\n').is_ok()).then(|| line = br#"{"c":"a line over 1 MiB"}"#.to_vec());
            let l = std::mem::take(&mut line);
            let cancel = serde_json::from_slice::<Value>(&l).is_ok_and(|v| v["c"] == "cancel");
            _ = if cancel { (sys::SEEN.fetch_add(1, SeqCst), tx.send(Ev::Signal(libc::SIGINT)).is_ok()).1 } else { tx.send(Ev::Input { tag: FRONT, bytes: Some(l) }).is_ok() };
        }
        // After every line before it: idle (also once a command ends) the session ends (exit 0); in a turn MAIN takes it
        // as a hangup (exit 130).
        let _ = tx.send(Ev::Input { tag: FRONT, bytes: None });
    };
    std::thread::Builder::new().name("front".into()).spawn(read).ok().map(|_| Reader(Default::default(), -1))
}

/// A command line as the REPL's line: idle, a `send`'s text, which is not typed (never a command), or a `command` as
/// the typed line `/NAME ARGS`, NAME a command's name (`[A-Za-z0-9._-]+`, not `exit` or `help`: EOF ends the session);
/// at the question about call `asked`, the verdict of an `answer` for it (yes, no or always). Anything else is a
/// `reject` and an empty typed line: nothing when idle, a no at a question.
pub fn take(s: &Session, line: &[u8], asked: Option<&str>) -> (String, bool) {
    let v: Value = serde_json::from_slice(line).unwrap_or_default();
    let verdict = v["verdict"].as_str().filter(|a| v["c"] == "answer" && asked.is_some_and(|id| v["id"] == id) && ["yes", "no", "always"].contains(a));
    let named = |n: &&str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) && !["exit", "help"].contains(n);
    let cmd = v["name"].as_str().filter(|n| v["c"] == "command" && named(n)).map(|n| format!("/{n} {}", v["args"].as_str().unwrap_or_default()));
    match (asked, v["text"].as_str().filter(|t| v["c"] == "send" && !t.trim().is_empty()), cmd, verdict) {
        (None, Some(t), ..) => (t.to_string(), false),
        (None, _, Some(c), _) => (c, true),
        (Some(_), .., Some(a)) => (a.to_string(), true),
        _ => (s.reject(&format!("not taken now: {}", v["c"].as_str().unwrap_or("not a command").chars().take(40).collect::<String>())), (String::new(), true)).1,
    }
}
