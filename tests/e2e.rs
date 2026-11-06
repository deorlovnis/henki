#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! End-to-end tests: the henki binary against the in-repo mock model, one process per run. Every test ends with
//! `Done::finish`, the postconditions every run must meet.
mod mock;

#[path = "e2e/harness.rs"]
mod harness;
#[path = "e2e/inert.rs"]
mod inert;
#[path = "e2e/journal.rs"]
mod journal;
#[path = "e2e/outcomes.rs"]
mod outcomes;
#[path = "e2e/processes.rs"]
mod processes;
#[path = "e2e/protocol.rs"]
mod protocol;
#[path = "e2e/request.rs"]
mod request;
#[path = "e2e/signals.rs"]
mod signals;
#[path = "e2e/stops.rs"]
mod stops;
