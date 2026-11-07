#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The REPL's line editor on a terminal: a paste of any size is one message, never a command and never an answer; a long
//! line arrives whole and is shown as it is; Backspace, Ctrl-U, keys henki drops, Ctrl-C and Ctrl-D while editing; and
//! the terminal as henki found it on every way out, bracketed paste off, also while a `!cmd` runs; input typed before a
//! question shows never answers it; and terminals henki does not draw on (`dumb`, stderr not a terminal). (Every `Pty`
//! run also checks the modes and bracketed paste at its end.)
use crate::harness::*;
use crate::mock::*;
use crate::pty::{Pty, RESET, paste_off, paste_on_at, same_modes, shown};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::time::Duration;

/// Generous: a 1 MiB paste is echoed through the terminal on a loaded machine.
const W: Duration = Duration::from_secs(60);
const HINT: &str = "(Ctrl-C does not quit; /exit or Ctrl-D quits)\r\n";
const NOT_AN_ANSWER: &str = "henki: a paste is not an answer; taken as no\r\n";

fn asking(m: &Mock) -> Run {
    let mut r = Run::new(&m.url());
    r.set("approve", "\"ask\"");
    r
}

/// The REPL on a 24 x 80 terminal, at its first prompt.
fn start(r: &Run) -> Pty<'_> {
    let p = Pty::spawn(r, &[], 24, 80);
    p.prompt(W);
    p
}

/// The end with exit `code` (`Pty::wait` checks the terminal modes and bracketed paste), and the postconditions.
fn ended(p: Pty<'_>, code: i32) -> Done {
    let d = p.wait(W);
    d.finish();
    assert_eq!(d.code, Some(code), "{}", d.diag());
    d
}

fn exit(p: Pty<'_>) -> Done {
    p.line("/exit");
    ended(p, 0)
}

/// Log lines, `n` bytes of them; every line differs.
fn log(n: usize) -> String {
    let mut s = String::new();
    for i in 0.. {
        if s.len() >= n {
            break;
        }
        s += &format!("2026-11-07T08:{:02}:{:02}Z worker[{i}] ERROR request failed: connection reset by peer\n", i / 60 % 60, i % 60);
    }
    s.truncate(n);
    s
}

/// The last user message of the mock's request `i`.
fn sent(m: &Mock, i: usize) -> String {
    m.log()[i].contents("user").last().unwrap().clone()
}

fn verdicts(d: &Done) -> Vec<Value> {
    d.of("approval").iter().map(|a| json!([a["verdict"], a["rule"]])).collect()
}

/// What henki echoes for `line` typed after `> ` on a terminal 80 wide: rows it wraps itself, the first after the prompt.
fn echoed(line: &str) -> String {
    let mut rows = vec![&line[..78]];
    rows.extend(line.as_bytes()[78..].chunks(80).map(|c| std::str::from_utf8(c).unwrap()));
    format!("> {}\r\n", rows.join("\r\n"))
}

/// A paste of 6 KiB and one of 64 KiB, each after typed words: each is one message, byte for byte, newlines kept.
#[test]
fn pty_paste_one_message() {
    let m = Mock::start(vec![say("read the 6 KiB"), say("read the 64 KiB")]);
    let r = asking(&m);
    let p = start(&r);
    for (i, n) in [6 << 10, 64 << 10].into_iter().enumerate() {
        let text = log(n);
        p.send(b"question big: ");
        p.paste(&text);
        p.send(b"\r");
        p.expect(["read the 6 KiB", "read the 64 KiB"][i], W);
        p.prompt(W);
        let want = format!("question big: {text}");
        let got = sent(&m, i);
        assert!(if i == 0 { got.ends_with(&want) } else { got == want }, "message {i}: {} bytes, not the {} pasted", got.len(), want.len());
    }
    let d = exit(p);
    assert_eq!(m.log().len(), 2, "{}", d.diag());
}

/// A paste over 1 MiB: one message of its first 1 MiB, with a note; nothing past it reaches the model or the next prompt.
#[test]
fn pty_paste_over_1_mib() {
    let m = Mock::start(vec![say("got the first MiB")]);
    let r = asking(&m);
    let p = start(&r);
    let text = log((1 << 20) + 1000);
    p.paste(&text);
    p.send(b"\r");
    p.expect("henki: the line is over 1,048,576 bytes: the last 1,000 were dropped\r\n", W);
    p.expect("got the first MiB", W);
    p.prompt(W);
    let d = exit(p);
    let got = sent(&m, 0);
    assert!(got.ends_with(&text[..1 << 20]) && got[..got.len() - (1 << 20)].ends_with("\n\n"), "{} bytes sent\n{}", got.len(), d.diag());
    assert_eq!(m.log().len(), 1, "{}", d.diag());
}

/// A pasted `!touch FILE` line, at the start, in the middle and right after the first 4,095 bytes (the canonical line
/// buffer of a Linux terminal), never runs: each paste is one message as it was pasted. Without paste marks, a block
/// sent at once is one message too.
#[test]
fn pty_pasted_bang_never_runs() {
    let m = Mock::start((0..5).map(|i| say(&format!("answer {i}"))).collect());
    let r = asking(&m);
    let f = r.path("PWNED");
    let bang = format!("!touch {}\n", f.display());
    let texts = [format!("{bang}after it\n"), format!("before it\n{bang}after it\n"), format!("{}{bang}after it\n", log(4095))];
    let p = start(&r);
    for (i, t) in texts.iter().enumerate() {
        p.paste(t);
        p.send(b"\r");
        p.expect(&format!("answer {i}"), W);
        p.prompt(W);
        assert!(sent(&m, i).ends_with(t.as_str()), "paste {i}: {:?}", sent(&m, i));
    }
    for (i, t) in texts[..2].iter().enumerate() {
        p.send(t.as_bytes());
        p.expect(&format!("answer {}", i + 3), W);
        p.prompt(W);
        assert!(sent(&m, i + 3).ends_with(t.trim_end_matches('\n')), "block {i}: {:?}", sent(&m, i + 3));
    }
    let d = exit(p);
    assert!(!f.exists(), "a pasted ! line ran\n{}", d.diag());
    assert_eq!(m.log().len(), 5, "{}", d.diag());
}

/// The case a canonical terminal got wrong: a question and 6,286 bytes of log sent at once without paste marks, with a
/// `!touch` line right after the first 4,095 bytes. Nothing runs, and no part of the block becomes a message of its own.
#[test]
fn pty_unmarked_paste_past_line_buffer() {
    let m = Mock::start(vec![say("about the log"), say("never")]);
    let r = asking(&m);
    let f = r.path("PWNED");
    let head = log(4095 - "question big\n".len());
    let tail = log(6286 - 4095 - 20);
    let block = format!("question big\n{head}!touch {}\n{tail}\n", f.display());
    let p = start(&r);
    p.send(block.as_bytes());
    p.expect("about the log", W);
    p.prompt(W);
    let d = exit(p);
    assert!(!f.exists(), "a pasted ! line ran\n{}", d.diag());
    assert_eq!(m.log().len(), 1, "part of the block became a message\n{}", d.diag());
    assert!(sent(&m, 0).contains("\n\nquestion big\n"), "{}", d.diag());
}

/// At an approval question a paste is never an answer: a pasted `y`, and a block sent at once without marks, are a no,
/// with a note, and so is a paste at the rule's confirmation (which asks `run?` again). A pasted `!` line never runs.
/// Bracketed paste is on while each question waits, also after the reset drawn before it, so a terminal marks a paste
/// there as it does at the prompt.
#[test]
fn pty_paste_at_question_not_an_answer() {
    let m = Mock::start(vec![shell("echo first"), say("after the first"), shell("echo second"), say("after the second")]);
    let r = asking(&m);
    let f = r.path("PASTED");
    let p = start(&r);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.paste("y");
    p.send(b"\r");
    p.expect(NOT_AN_ANSWER, W);
    p.expect("after the first", W);
    p.prompt(W);
    p.line("again");
    p.expect("run? [y/N/always] ", W);
    p.line("always");
    p.expect(" ? [y/N] ", W);
    p.paste("y\n");
    p.send(b"\r");
    p.expect(NOT_AN_ANSWER, W);
    p.expect("run? [y/N/always] ", W);
    p.send(format!("y\n!touch {}\npasted-tail-line\n", f.display()).as_bytes());
    p.expect(NOT_AN_ANSWER, W);
    p.expect("after the second", W);
    p.prompt(W);
    let screen = p.raw_screen();
    let d = exit(p);
    assert_eq!(verdicts(&d), [json!(["no", null]), json!(["no", null])], "{}", d.diag());
    assert!(!f.exists() && !r.allow().exists(), "{}", d.diag());
    assert!(m.log().len() == 4 && m.log().iter().all(|l| !l.text().contains("pasted-tail-line")), "{}", d.diag());
    for question in ["run? [y/N/always] ", " ? [y/N] "] {
        assert!(paste_on_at(&screen, question), "bracketed paste is off while {question:?} waits:\n{}", shown(&screen));
    }
}

/// What is typed before the call's display reaches the terminal never answers its question. henki's stdout is a pipe
/// nobody reads yet, so the long answer before the call holds the display and the question back while `y` and Enter are
/// typed. henki drops them, with a note, once everything before the question is written, and only then asks.
#[test]
fn pty_typeahead_before_question_shown() {
    let mut r = Run::new("");
    let f = r.path("CALLED");
    let reply = Step::Stream { parts: vec![say(&"x".repeat(300_000)), shell(&format!("touch {}", f.display()))], finish: Some("tool_calls".into()), done: true, hold_ms: 0 };
    let m = Mock::start(vec![reply, say("after the no")]);
    r.at(&m.url()).set("approve", "\"ask\"");
    let (mut rd, wr) = std::io::pipe().unwrap();
    let p = Pty::spawn_to(&r, &[], 24, 80, Some(fs::File::from(std::os::fd::OwnedFd::from(wr))));
    p.prompt(W);
    p.line("go");
    assert!(poll(W, || m.log().len() == 1), "no request");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!String::from_utf8_lossy(&p.screen()).contains("run? "), "the question is on the screen: stdout did not hold it back");
    p.send(b"y\r");
    p.expect("y\r\n", W); // the terminal's own echo: henki is not reading
    std::thread::sleep(Duration::from_millis(300));
    let drain = std::thread::spawn(move || {
        let mut b = Vec::new();
        rd.read_to_end(&mut b).unwrap();
        String::from_utf8(b).unwrap()
    });
    p.expect("henki: dropped 2 bytes typed before this prompt\r\nrun? [y/N/always] ", W);
    p.line("n");
    p.prompt(W);
    let d = exit(p);
    assert!(drain.join().unwrap().ends_with("after the no\n"), "the answers on stdout");
    assert!(!f.exists(), "the call ran on what was typed before its question showed\n{}", d.diag());
    assert_eq!(verdicts(&d), [json!(["no", null])], "{}", d.diag());
}

/// Backspace after a pasted log draws one row, not the whole line again: three of them write a few dozen bytes however
/// long the paste before them.
#[test]
fn pty_backspace_after_paste_one_row() {
    let m = Mock::start(vec![say("explained")]);
    let r = asking(&m);
    let p = start(&r);
    let log: String = (0..250).map(|i| format!("UNIQUE-{i:03} {}\n", "x".repeat(60))).collect();
    p.paste(&log);
    p.send(b" please explain");
    p.expect(" please explain", W);
    let before = p.raw_screen().len();
    p.send(b"\x7f\x7f\x7f");
    p.expect("\r\x1b[J please expl", W);
    let wrote = p.raw_screen().len() - before;
    assert!(wrote < 100, "{wrote} bytes written for 3 Backspaces");
    p.send(b"\r");
    p.expect("explained", W);
    p.prompt(W);
    let d = exit(p);
    assert!(sent(&m, 0).ends_with(&format!("\n\n{log} please expl")), "{}", d.diag());
}

/// With TERM=dumb (an editor's shell buffer) henki writes no bracketed paste switch and no reset, at the prompt or at a
/// question.
#[test]
fn pty_dumb_terminal_no_switches() {
    let m = Mock::start(vec![shell("echo asked"), say("after the no")]);
    let mut r = asking(&m);
    r.env("TERM", "dumb");
    let p = start(&r);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.line("n");
    p.expect("after the no", W);
    p.prompt(W);
    let screen = String::from_utf8_lossy(&p.raw_screen()).into_owned();
    let d = exit(p);
    assert!(!screen.contains("\x1b[?2004") && !screen.contains(RESET), "{}", shown(screen.as_bytes()));
    assert_eq!(verdicts(&d), [json!(["no", null])], "{}", d.diag());
}

/// With stderr not a terminal (`henki 2>log`) nothing typed could be shown, nor a question: the REPL does not start, and
/// says why on stderr.
#[test]
fn pty_stderr_not_a_terminal() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let log = r.dir.join("stderr.log");
    let p = Pty::spawn_io(&r, &[], 24, 80, None, Some(fs::File::create(&log).unwrap()));
    let d = p.wait(W);
    assert_eq!(d.code, Some(2), "{}", d.diag());
    assert_eq!(fs::read_to_string(&log).unwrap(), "henki: the REPL needs a terminal; use henki run\n");
    assert!(m.log().is_empty() && d.journal.is_none());
}

/// A line of 3,000 bytes, typed in one go and then pasted: each arrives whole, and the screen shows it as it is, in rows
/// henki wraps before the last column.
#[test]
fn pty_long_line_intact() {
    let m = Mock::start(vec![say("typed one"), say("pasted one")]);
    let r = asking(&m);
    let line: String = (0..3000).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    let p = start(&r);
    p.send(line.as_bytes());
    p.send(b"\r");
    p.expect("typed one", W);
    p.prompt(W);
    p.paste(&line);
    p.send(b"\r");
    p.expect("pasted one", W);
    p.prompt(W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert_eq!(screen.matches(&echoed(&line)).count(), 2, "the line not shown as it is:\n{screen:?}");
    let d = exit(p);
    assert!(sent(&m, 0).ends_with(&format!("\n\n{line}")) && sent(&m, 1) == line, "{}", d.diag());
}

/// Backspace takes one whole character, multi-byte ones too, and draws the line again; Ctrl-U empties it.
#[test]
fn pty_backspace_and_ctrl_u() {
    let m = Mock::start(vec![say("first done"), say("second done")]);
    let r = asking(&m);
    let p = start(&r);
    p.send("aä€😀".as_bytes());
    p.expect("aä€😀", W);
    p.send(b"\x7f\x7f\x7f");
    p.send(b"b\r");
    p.expect("first done", W);
    p.prompt(W);
    p.send(b"garbage that goes");
    p.send(b"\x15real\r");
    p.expect("second done", W);
    p.prompt(W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains("\r\x1b[J> ab\r\n") && screen.contains("\r\x1b[J> real\r\n"), "{screen:?}");
    let d = exit(p);
    assert!(sent(&m, 0).ends_with("\n\nab") && sent(&m, 1) == "real", "{}", d.diag());
}

/// Arrow keys and other sequences are dropped: nothing of them is echoed or sent.
#[test]
fn pty_keys_dropped() {
    let m = Mock::start(vec![say("keys done")]);
    let r = asking(&m);
    let p = start(&r);
    for k in [&b"ab"[..], b"\x1b[D", b"\x1b[A", b"\x1bOB", b"\x1b[3~", b"\x1b[1;5C", b"\x01\x07", b"c\r"] {
        p.send(k);
    }
    p.expect("keys done", W);
    p.prompt(W);
    let screen = String::from_utf8_lossy(&p.screen()).into_owned();
    assert!(screen.contains("\n> abc\r\n"), "{screen:?}");
    let d = exit(p);
    assert!(sent(&m, 0).ends_with("\n\nabc"), "{}", d.diag());
}

/// Ctrl-C while a line is being typed: the hint, a new prompt, and the line is not sent. Ctrl-D on a line with text does
/// nothing; on an empty one it quits.
#[test]
fn pty_ctrl_c_and_ctrl_d_while_editing() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r);
    p.send(b"partial");
    p.expect("partial", W);
    p.ctrl_c();
    p.expect(&format!("\r\n{HINT}> "), W);
    p.send(b"x");
    p.ctrl_d();
    p.send(b"\x7f");
    p.ctrl_d();
    let d = ended(p, 0);
    assert!(m.log().is_empty() && d.journal.is_none(), "{}", d.diag());
}

/// The terminal modes henki found, and bracketed paste off, on every way out (`Pty::wait` checks both): `/exit`, Ctrl-D,
/// Ctrl-C then `/exit`, TERM and HUP at the prompt, TERM during a `!cmd` that made the terminal raw, Ctrl-D at a
/// question, and a closed stdout.
#[test]
fn pty_modes_restored_every_exit() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r);
    exit(p);
    let p = start(&r);
    p.ctrl_d();
    ended(p, 0);
    let p = start(&r);
    p.ctrl_c();
    p.expect(HINT, W);
    exit(p);
    for sig in [libc::SIGTERM, libc::SIGHUP] {
        let p = start(&r);
        p.signal(sig);
        ended(p, 130);
    }
    let p = start(&r);
    p.line(&format!("!stty raw -echo; sleep {}", r.marker));
    assert!(poll(W, || !marked(&r.marker).is_empty()), "the command did not start");
    assert!(!same_modes(&p.modes(), &p.initial), "the command left the terminal as it was");
    p.signal(libc::SIGTERM);
    ended(p, 130);
    let m = Mock::start(vec![shell("echo asked"), say("never")]);
    let r = asking(&m);
    let p = start(&r);
    p.line("go");
    p.expect("run? [y/N/always] ", W);
    p.ctrl_d(); // the turn ends, and the REPL with it
    p.expect("(interrupted)\r\n", W);
    ended(p, 0);
    let m = Mock::start(vec![say("nobody reads this")]);
    let r = asking(&m);
    let (rd, wr) = std::io::pipe().unwrap();
    drop(rd);
    let p = Pty::spawn_to(&r, &[], 24, 80, Some(fs::File::from(std::os::fd::OwnedFd::from(wr))));
    p.prompt(W);
    p.line("hello");
    p.expect("(output closed)\r\n", W);
    ended(p, 141);
}

/// While a `!cmd` runs the terminal has the modes henki found and bracketed paste is off; at the next prompt it is on.
#[test]
fn pty_bang_gets_a_plain_terminal() {
    let m = Mock::start(vec![]);
    let r = asking(&m);
    let p = start(&r);
    assert!(!paste_off(&p.raw_screen()), "bracketed paste is off at the prompt");
    assert!(!same_modes(&p.modes(), &p.initial), "the prompt reads with the terminal's own modes");
    p.line("!cat");
    assert!(poll(W, || same_modes(&p.modes(), &p.initial)), "the modes are not back for the command");
    p.line("into cat");
    p.expect("into cat\r\ninto cat\r\n", W);
    assert!(paste_off(&p.raw_screen()), "bracketed paste is on while the command runs");
    p.ctrl_d();
    p.prompt(W);
    assert!(!paste_off(&p.raw_screen()), "bracketed paste is off at the next prompt");
    let d = exit(p);
    assert!(m.log().is_empty(), "{}", d.diag());
}
