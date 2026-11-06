#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;

fn every() -> [Outcome; 7] {
    [
        Outcome::Ok,
        Outcome::GaveUp("stuck"),
        Outcome::error("provider", "HTTP 500"),
        Outcome::NeedsApproval(Call { id: "h1".into(), name: "shell".into(), ..Call::default() }),
        Outcome::Budget,
        Outcome::Interrupted,
        Outcome::OutputClosed,
    ]
}

#[test]
fn exit_table() {
    assert_eq!(every().map(|o| o.exit()), [0, 1, 2, 3, 124, 130, 141]);
}

#[test]
fn status_words() {
    assert_eq!(every().map(|o| o.status()), ["ok", "gave_up", "error", "needs_approval", "budget", "interrupted", "output_closed"]);
}

#[test]
fn reason_only_gave_up() {
    assert_eq!(every().map(|o| o.reason()), [None, Some("stuck"), None, None, None, None, None]);
    for r in ["max_steps", "stuck", "empty_answer"] {
        assert_eq!(Outcome::GaveUp(r).reason(), Some(r));
    }
}

#[test]
fn not_run_reasons() {
    assert_eq!(every().map(|o| o.not_run()), ["interrupted", "interrupted", "interrupted", "needs approval", "budget", "interrupted", "output closed"]);
}

#[test]
fn stops_set() {
    assert_eq!(every().map(|o| o.stops()), [false, false, false, false, true, true, true]);
}

#[test]
fn from_stop() {
    assert_eq!(Outcome::from(Stop::Signal(2)), Outcome::Interrupted);
    assert_eq!(Outcome::from(Stop::Signal(15)), Outcome::Interrupted);
    assert_eq!(Outcome::from(Stop::Budget), Outcome::Budget);
    assert_eq!(Outcome::from(Stop::Closed), Outcome::OutputClosed);
}

#[test]
fn usage_add() {
    let mut u = Usage { input: 1, output: 2, cached: 3 };
    u.add(Usage { input: 10, output: 20, cached: 30 });
    u.add(Usage::default());
    assert_eq!(u, Usage { input: 11, output: 22, cached: 33 });
}

#[test]
fn error_builds_the_error_outcome() {
    assert_eq!(Outcome::error("config", String::from("task is empty")), Outcome::Error { kind: "config", message: "task is empty".into() });
}
