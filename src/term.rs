//! The only writer to stdout and stderr: one escape table for every path, a JSON line writer, and a writer thread so a
//! reader that stops reading never blocks the main thread.
use crate::types::Ev;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};

/// Code points escaped on every output path (C0 except tab and newline, DEL, C1, Cf, Zl/Zp, Zs except space, default ignorables).
#[rustfmt::skip]
pub const ESCAPE: &[(u32, u32)] = &[
    (0x00, 0x08), (0x0B, 0x1F), (0x7F, 0x9F),
    (0xA0, 0xA0), (0xAD, 0xAD), (0x34F, 0x34F), (0x600, 0x605), (0x61C, 0x61C), (0x6DD, 0x6DD), (0x70F, 0x70F), (0x890, 0x891), (0x8E2, 0x8E2),
    (0x115F, 0x1160), (0x1680, 0x1680), (0x17B4, 0x17B5), (0x180B, 0x180F), (0x2000, 0x200F), (0x2028, 0x202F), (0x205F, 0x206F), (0x3000, 0x3000), (0x3164, 0x3164),
    (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFFB), (0x110BD, 0x110BD), (0x110CD, 0x110CD),
    (0x13430, 0x1343F), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF),
];

pub fn escaped(c: char) -> bool {
    todo!()
}

/// `\t` and `\n` stay; an escaped code point below 0x100 becomes `\xNN`, any other `\u{XXXX}` (lowercase hex).
pub fn escape(s: &str) -> std::borrow::Cow<'_, str> {
    todo!()
}

/// Compact JSON, keys sorted, with every escaped code point inside strings written as `\uXXXX` (surrogate pairs above U+FFFF).
pub fn json_line(v: &Value) -> String {
    todo!()
}

/// `2,941,512`.
pub fn num(n: u64) -> String {
    todo!()
}

/// Milliseconds as seconds with one decimal: `2.4`.
pub fn secs(ms: u64) -> String {
    todo!()
}

pub struct Out {
    json: bool,
    queue: Option<Sender<(u8, Vec<u8>)>>,
    closed: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl Out {
    /// The writer thread over fd 1 and fd 2. `main` calls it after argument parsing, when `--json` is known.
    pub fn start(json: bool, tx: Sender<Ev>) -> Out {
        todo!()
    }

    /// The same over any two writers (tests).
    pub fn with(json: bool, tx: Sender<Ev>, out: Box<dyn std::io::Write + Send>, err: Box<dyn std::io::Write + Send>) -> Out {
        todo!()
    }

    pub fn json(&self) -> bool {
        todo!()
    }

    /// `--json` only: one `json_line` + "\n" to stdout. Plain mode: nothing.
    pub fn record(&self, rec: &Value) {
        todo!()
    }

    /// Plain mode: the final answer, escaped, + "\n" to stdout.
    pub fn answer(&self, text: &str) {
        todo!()
    }

    /// stderr: TEXT, escaped, + "\n", no prefix: the plain call lines (`$ ls -> exit 0, 0.3 s`) and the outcome line.
    pub fn line(&self, text: &str) {
        todo!()
    }

    /// stderr: `henki: TEXT`, escaped, + "\n" (notices and errors); `line` with the prefix.
    pub fn note(&self, text: &str) {
        todo!()
    }

    /// stdout gave EPIPE (or any write error) so far.
    pub fn closed(&self) -> bool {
        todo!()
    }

    /// Close the queue and wait for the writer until `until`, then return regardless. Returns whether stdout was closed
    /// at any point, including on lines written during this flush (the `result` line itself).
    pub fn flush(self, until: std::time::Instant) -> bool {
        todo!()
    }
}

/// The panic hook's line, written synchronously to fd 2: `henki: MSG`, escaped.
pub fn panic_line(msg: &str) {
    todo!()
}
