#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! End-to-end tests: the henki binary against the in-repo mock model, one process per run (the REPL under a
//! pseudo-terminal). Every test ends with `Done::finish`, the postconditions every run must meet.
mod mock;

#[path = "e2e/allow.rs"]
mod allow;
#[path = "e2e/anthropic.rs"]
mod anthropic;
#[path = "e2e/ext.rs"]
mod ext;
#[path = "e2e/handoff.rs"]
mod handoff;
#[path = "e2e/harness.rs"]
mod harness;
#[path = "e2e/inert.rs"]
mod inert;
#[path = "e2e/input.rs"]
mod input;
#[path = "e2e/journal.rs"]
mod journal;
#[path = "e2e/observe.rs"]
mod observe;
#[path = "e2e/outcomes.rs"]
mod outcomes;
#[path = "e2e/policy.rs"]
mod policy;
#[path = "e2e/ports.rs"]
mod ports;
#[path = "e2e/processes.rs"]
mod processes;
#[path = "e2e/prompt.rs"]
mod prompt;
#[path = "e2e/protocol.rs"]
mod protocol;
#[path = "e2e/pty.rs"]
mod pty;
#[path = "e2e/repl.rs"]
mod repl;
#[path = "e2e/request.rs"]
mod request;
#[path = "e2e/signals.rs"]
mod signals;
#[path = "e2e/stops.rs"]
mod stops;
#[path = "e2e/trust.rs"]
mod trust;
