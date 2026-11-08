#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The contracts of CONTRACTS.md, pinned: the field tables there against the records henki writes, golden journals
//! replayed, the `--json` stream of scripted runs, the request henki sends and the responses it accepts (against the
//! in-repo mock and fixed SSE fixtures), and the extension layouts henki loads or refuses. The index, the field
//! tables and the exit table are read from the document, so a change there or in the code fails a test; the prose
//! rules are pinned by tests written from them, which fail when the code changes, not when only the text does.

// The e2e harness and mock, shared; this crate uses part of them.
#[allow(dead_code)]
#[path = "e2e/harness.rs"]
mod harness;
#[allow(dead_code)]
mod mock;

#[path = "contracts/doc.rs"]
mod doc;
#[path = "contracts/ext.rs"]
mod ext;
#[path = "contracts/journal.rs"]
mod journal;
#[path = "contracts/json.rs"]
mod json;
#[path = "contracts/ports.rs"]
mod ports;
#[path = "contracts/wire.rs"]
mod wire;
