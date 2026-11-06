#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! Signals, the deadline and a closed stdout: each ends the run with its own code, within its bound, and kills what
//! the run started.
use crate::harness::*;
use crate::mock::*;
use std::time::{Duration, Instant};

/// A shell call that runs until killed; the signal arrives while it runs.
fn signal_during_call(sig: i32) {
    let mut r = Run::new("");
    let m = Mock::start(vec![shell(&r.sleep()), say("never")]);
    r.at(&m.url());
    let mut l = r.cmd(&["--json", "wait"]).spawn();
    l.ready();
    l.wait_marked(5 * SEC);
    let t = Instant::now();
    l.signal(sig);
    let d = l.wait();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(130), "interrupted"), "{}", d.diag());
    assert!(d.exited - t < Duration::from_millis(1000), "exit {:?} after the signal", d.exited - t);
    let c = d.results()[0]["content"].as_str().unwrap().to_string();
    assert!(c.ends_with("[interrupted; process group killed]") || c == "interrupted; the command may have partly run", "{c}");
    assert_eq!(m.log().len(), 1);
}

#[test]
fn e2e_run_sigint_130() {
    signal_during_call(libc::SIGINT);
}

#[test]
fn e2e_run_sigterm_130() {
    signal_during_call(libc::SIGTERM);
}

#[test]
fn e2e_run_sighup_130() {
    signal_during_call(libc::SIGHUP);
}

/// Exit 124 within the budget plus one second.
fn budget_exit(d: &Done, budget_s: u64) {
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(124), "budget"), "{}", d.diag());
    assert!(d.elapsed <= Duration::from_secs(budget_s + 1), "exit after {:?}", d.elapsed);
    assert!(d.elapsed >= Duration::from_millis(budget_s * 1000 - 100), "exit after {:?}", d.elapsed);
}

#[test]
fn e2e_budget_fifo() {
    let mut r = Run::new("");
    assert!(std::process::Command::new("mkfifo").arg(r.path("pipe")).status().unwrap().success());
    // The shell's fork blocks opening the fifo; it carries the marker and, being a fork, no parent-death signal.
    let m = Mock::start(vec![call("read", serde_json::json!({"path": "pipe"})), shell(&format!("sleep {} < pipe; :", r.marker)), say("never")]);
    r.at(&m.url());
    let d = r.cmd(&["--json", "--max-seconds", "2", "read the pipe"]).go();
    budget_exit(&d, 2);
    let first = d.results()[0]["content"].as_str().unwrap().to_string();
    assert!(first.starts_with("not a regular file: ") && first.ends_with(" (fifo)"), "{first}");
}

#[test]
fn e2e_budget_stalled_server() {
    let mut r = Run::new("");
    let m = Mock::start(vec![term_proof(&r), Step::Stall { before_first_byte: true, after_words: 0, for_ms: 30_000 }]);
    r.at(&m.url());
    let d = r.cmd(&["--json", "--max-seconds", "2", "wait"]).go();
    budget_exit(&d, 2);
    closed_before_exit(&m, 1, &d);
}

#[test]
fn e2e_budget_stalled_stdout() {
    let long: String = (0..4000).map(|i| format!("word{i} ")).collect();
    let m = Mock::start(vec![Step::Stream {
        parts: vec![Step::Say(long), Step::Stall { before_first_byte: false, after_words: 0, for_ms: 30_000 }],
        finish: Some("stop".into()),
        done: true,
        hold_ms: 0,
    }]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["--json", "--max-seconds", "2", "talk"]).unread_stdout().timeout(Duration::from_secs(10)).go();
    budget_exit(&d, 2);
    assert!(d.stdout.len() >= 60_000, "stdout was not blocked: only {} bytes", d.stdout.len());
    closed_before_exit(&m, 0, &d);
}

#[test]
fn e2e_budget_stdin_open() {
    let r = Run::new("http://127.0.0.1:9/v1");
    let d = r.cmd(&["--json", "--max-seconds", "2", "-"]).stdin(In::Open).go();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(124), "budget"), "{}", d.diag());
    assert!(d.elapsed <= Duration::from_secs(3), "exit after {:?}", d.elapsed);
    assert!(d.result()["session"].is_null() && d.journal.is_none(), "{}", d.diag());
}

#[test]
fn e2e_signal_before_session_null() {
    let r = Run::new("http://127.0.0.1:9/v1");
    let mut l = r.cmd(&["--json", "-"]).stdin(In::Open).spawn();
    l.ready();
    l.signal(libc::SIGTERM);
    let d = l.wait();
    d.finish();
    assert_eq!((d.code, d.status().as_str()), (Some(130), "interrupted"), "{}", d.diag());
    assert!(d.result()["session"].is_null() && d.journal.is_none(), "{}", d.diag());
    assert_eq!(d.records().len(), 1);
}

#[test]
fn e2e_json_head_141() {
    let m = Mock::start(vec![Step::Stall { before_first_byte: false, after_words: 1, for_ms: 1500 }]);
    let r = Run::new(&m.url());
    let d = r.cmd(&["--json", "talk slowly"]).close_after_first_line().go();
    d.finish();
    assert_eq!(d.code, Some(141), "{}", d.diag());
    assert_eq!(d.result()["status"], "output_closed", "the journal says why");
    assert_eq!(d.records()[0]["t"], "session");
    assert!(d.records().iter().all(|x| x["t"] != "result"), "no stdout result");
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Startup,
    Requesting,
    Streaming,
    Backoff,
    Running,
    Background,
    Escalating,
    Stopping,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cause {
    Signal(i32),
    Deadline,
    Epipe,
}

/// One case: reach `state`, apply `cause`, check the code. With `Epipe` every state resolves itself after about a
/// second into a record write, which meets the closed stdout.
#[cfg(target_os = "linux")]
fn case(state: State, cause: Cause) {
    let quick = cause == Cause::Epipe;
    let hold = if quick { 1000 } else { 30_000 };
    let mut r = Run::new("");
    let stall = |before: bool, words: usize| Step::Stall { before_first_byte: before, after_words: words, for_ms: hold };
    let steps = match state {
        State::Startup => vec![say("started")],
        State::Requesting => vec![stall(true, 0)],
        State::Streaming => vec![stall(false, 3)],
        State::Backoff => vec![Step::Error { status: 429, retry_after: Some(if quick { "1" } else { "30" }.into()), body: "{}".into(), times: 3 }, say("late")],
        State::Running => vec![shell(&if quick { "sleep 1".into() } else { r.sleep() }), say("done")],
        State::Background => vec![shell(&format!("sleep {} & echo started", r.marker)), stall(true, 0), say("done")],
        State::Escalating => {
            let cmd = format!("trap 'echo term > esc.flag' TERM; while :; do sleep 0.05; done # {}", r.marker);
            let delay = Step::Stall { before_first_byte: true, after_words: 0, for_ms: if cause == Cause::Deadline { 650 } else { 0 } };
            vec![
                Step::Stream { parts: vec![delay, call("shell", serde_json::json!({"command": cmd, "timeout": 1}))], finish: Some("tool_calls".into()), done: true, hold_ms: 0 },
                stall(true, 0),
            ]
        }
        State::Stopping => vec![shell(&format!("trap '' TERM; {}", r.sleep())), say("never")],
    };
    let m = Mock::start(steps);
    r.at(&m.url());
    let mut args = vec!["--json"];
    if cause == Cause::Deadline {
        args.extend(["--max-seconds", "2"]);
    }
    args.push(if state == State::Startup { "-" } else { "go" });
    let mut l = r.cmd(&args).stdin(if state == State::Startup { In::Open } else { In::Null }).spawn();
    l.ready();
    let w = 5 * SEC;
    match state {
        State::Startup => {}
        State::Requesting => drop(m.wait_requests(1, w)),
        State::Streaming => drop(l.wait_record(w, |x| x["t"] == "text")),
        State::Backoff => drop(l.wait_record(w, |x| x["t"] == "notice" && x["text"].as_str().is_some_and(|t| t.contains("retry 1/4")))),
        State::Running => drop(l.wait_record(w, |x| x["t"] == "approval")),
        State::Background => drop(m.wait_requests(2, w)),
        // The deadline is timed to land in the grace after the call's SIGTERM; it ends the run wherever it lands.
        State::Escalating if cause == Cause::Deadline => drop(poll(w, || r.path("esc.flag").exists() || !l.running())),
        State::Escalating => l.until("the call's SIGTERM", w, |_| r.path("esc.flag").exists()),
        State::Stopping => {
            l.wait_marked(w);
            if cause == Cause::Deadline {
                l.until("the deadline's eve", w, |l| l.started.elapsed() >= Duration::from_millis(1700));
            }
            l.signal(libc::SIGINT);
        }
    }
    let t = Instant::now();
    match cause {
        Cause::Signal(s) => l.signal(s),
        Cause::Deadline => {}
        Cause::Epipe => {
            l.close_stdout();
            if state == State::Startup {
                l.write_stdin(b"the task");
                l.close_stdin();
            }
        }
    }
    let d = l.wait();
    let want = match (state, cause) {
        (State::Stopping, _) | (_, Cause::Signal(_)) => 130,
        (_, Cause::Deadline) => 124,
        (_, Cause::Epipe) => 141,
    };
    d.check(d.code == Some(want), &format!("{state:?} x {cause:?}: exit {:?}, not {want}", d.code));
    d.finish();
    match cause {
        Cause::Deadline => d.check(d.elapsed <= Duration::from_secs(3), &format!("{state:?} x deadline: exit after {:?}", d.elapsed)),
        _ => d.check(d.exited - t <= Duration::from_millis(2500), &format!("{state:?} x {cause:?}: exit {:?} after the cause", d.exited - t)),
    }
    if state == State::Startup && cause != Cause::Epipe {
        d.check(d.journal.is_none() && d.result()["session"].is_null(), "a stop before the session has session null");
    }
}

/// Linux only: the states and their timing are measured there.
#[test]
#[cfg(target_os = "linux")]
fn e2e_interrupt_matrix() {
    let states = [State::Startup, State::Requesting, State::Streaming, State::Backoff, State::Running, State::Background, State::Escalating, State::Stopping];
    let causes = [Cause::Signal(libc::SIGINT), Cause::Signal(libc::SIGQUIT), Cause::Signal(libc::SIGTERM), Cause::Signal(libc::SIGHUP), Cause::Deadline, Cause::Epipe];
    let threads: Vec<_> = states.into_iter().map(|s| std::thread::spawn(move || causes.into_iter().for_each(|c| case(s, c)))).collect();
    let failed: Vec<String> = threads.into_iter().zip(states).filter_map(|(t, s)| t.join().err().map(|_| format!("{s:?}"))).collect();
    assert!(failed.is_empty(), "failed states: {failed:?}");
}
