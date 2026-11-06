//! The only writer to stdout and stderr: one escape table for every path, a JSON line writer, and a writer thread so a
//! reader that stops reading never blocks the main thread.
use crate::types::Ev;
use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering::SeqCst, mpsc, mpsc::Receiver, mpsc::Sender};
use std::{borrow::Cow, fmt::Write as _, io::Write, time::Instant};

/// Code points escaped on every output path (C0 except tab and newline, DEL, C1, Cf, Zl/Zp, Zs except space, default ignorables).
#[rustfmt::skip]
pub const ESCAPE: &[(u32, u32)] = &[
    (0x00, 0x08), (0x0B, 0x1F), (0x7F, 0x9F),
    (0xA0, 0xA0), (0xAD, 0xAD), (0x34F, 0x34F), (0x600, 0x605), (0x61C, 0x61C), (0x6DD, 0x6DD), (0x70F, 0x70F), (0x890, 0x891), (0x8E2, 0x8E2),
    (0x115F, 0x1160), (0x1680, 0x1680), (0x17B4, 0x17B5), (0x180B, 0x180F), (0x2000, 0x200F), (0x2028, 0x202F), (0x205F, 0x206F), (0x3000, 0x3000), (0x3164, 0x3164),
    (0xFE00, 0xFE0F), (0xFEFF, 0xFEFF), (0xFFA0, 0xFFA0), (0xFFF0, 0xFFFB), (0x110BD, 0x110BD), (0x110CD, 0x110CD),
    (0x13430, 0x1343F), (0x1BCA0, 0x1BCA3), (0x1D173, 0x1D17A), (0xE0000, 0xE0FFF),
];

/// The table is sorted, so the scan stops at the first range that starts above `c`.
pub fn escaped(c: char) -> bool {
    let u = c as u32;
    ESCAPE.iter().take_while(|r| r.0 <= u).any(|r| u <= r.1)
}

/// `\t` and `\n` stay; an escaped code point below 0x100 becomes `\xNN`, any other `\u{XXXX}` (lowercase hex).
pub fn escape(s: &str) -> Cow<'_, str> {
    let form = |o: &mut String, c: char| if (c as u32) < 0x100 { write!(o, "\\x{:02x}", c as u32) } else { write!(o, "\\u{{{:04x}}}", c as u32) };
    subst(s, form).map_or(Cow::Borrowed(s), Cow::Owned)
}

/// Compact JSON, keys sorted, with every escaped code point inside strings written as `\uXXXX` (surrogate pairs above U+FFFF).
/// Outside strings compact JSON is ASCII without controls, so one pass over the whole text only touches string contents.
pub fn json_line(v: &Value) -> String {
    let s = v.to_string();
    let form = |o: &mut String, c: char| c.encode_utf16(&mut [0; 2]).iter().try_for_each(|w| write!(o, "\\u{w:04x}"));
    subst(&s, form).unwrap_or(s)
}

/// `s` with each escaped code point written by `form`; None when there is none.
fn subst(s: &str, form: impl Fn(&mut String, char) -> std::fmt::Result) -> Option<String> {
    let (head, tail) = s.split_at(s.find(escaped)?);
    let mut o = String::with_capacity(s.len() + 16);
    o.push_str(head);
    for c in tail.chars() {
        let _ = if escaped(c) { form(&mut o, c) } else { o.write_char(c) };
    }
    Some(o)
}

/// `2,941,512`.
pub fn num(n: u64) -> String {
    let d = n.to_string();
    d.chars().enumerate().flat_map(|(i, c)| [(i > 0 && (d.len() - i).is_multiple_of(3)).then_some(','), Some(c)]).flatten().collect()
}

/// Milliseconds as seconds with one decimal, rounded half up: `2.4`.
pub fn secs(ms: u64) -> String {
    let t = ms / 100 + u64::from(ms % 100 >= 50);
    format!("{}.{}", t / 10, t % 10)
}

/// The queue to the writer thread; lines leave in the order they were queued, across both streams.
pub struct Out {
    json: bool,
    queue: Sender<(u8, Vec<u8>)>,
    closed: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl Out {
    /// The writer thread over fd 1 and fd 2. `main` calls it after argument parsing, when `--json` is known.
    #[allow(clippy::disallowed_methods)] // the one place that takes fd 1 and fd 2
    pub fn start(json: bool, tx: Sender<Ev>) -> Out {
        Out::with(json, tx, Box::new(std::io::stdout()), Box::new(std::io::stderr()))
    }

    /// The same over any two writers (tests). The first stdout error marks stdout closed, sends `Ev::OutputClosed` once
    /// and drops every later stdout line; stderr errors are ignored.
    pub fn with(json: bool, tx: Sender<Ev>, mut out: Box<dyn Write + Send>, mut err: Box<dyn Write + Send>) -> Out {
        let (queue, lines) = mpsc::channel::<(u8, Vec<u8>)>();
        let (end, done) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&closed);
        let writer = move || {
            for (fd, b) in lines {
                if fd == 2 {
                    let _ = err.write_all(&b).and_then(|()| err.flush());
                } else if !c.load(SeqCst) && out.write_all(&b).and_then(|()| out.flush()).is_err() {
                    c.store(true, SeqCst);
                    let _ = tx.send(Ev::OutputClosed);
                }
            }
            let _ = end.send(());
        };
        if std::thread::Builder::new().name("output".into()).spawn(writer).is_err() {
            closed.store(true, SeqCst); // nothing can be written: report stdout closed rather than lose lines silently
        }
        Out { json, queue, closed, done }
    }

    /// Queues `line()` for fd 1 or 2 when `when` holds; nothing is built otherwise.
    fn send(&self, fd: u8, when: bool, line: impl FnOnce() -> String) {
        if when {
            let _ = self.queue.send((fd, line().into_bytes()));
        }
    }

    pub fn json(&self) -> bool {
        self.json
    }

    /// `--json` only: one `json_line` + "\n" to stdout. Plain mode: nothing.
    pub fn record(&self, rec: &Value) {
        self.send(1, self.json, || json_line(rec) + "\n");
    }

    /// Plain mode: the final answer, escaped, + "\n" to stdout. `--json`: nothing (stdout holds records only).
    pub fn answer(&self, text: &str) {
        self.send(1, !self.json, || format!("{}\n", escape(text)));
    }

    /// Plain mode: TEXT, escaped, + "\n" to stderr, no prefix: the call lines (`$ ls -> exit 0, 0.3 s`) and the outcome
    /// line. `--json`: nothing (stderr is kept for a crash message).
    pub fn line(&self, text: &str) {
        self.send(2, !self.json, || format!("{}\n", escape(text)));
    }

    /// Plain mode: `henki: TEXT`, escaped, + "\n" to stderr (notices and errors); `line` with the prefix.
    pub fn note(&self, text: &str) {
        self.line(&format!("henki: {text}"));
    }

    /// stdout gave EPIPE (or any write error) so far.
    pub fn closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    /// Close the queue and wait for the writer until `until`, then return regardless. Returns whether stdout was closed
    /// at any point, including on lines written during this flush (the `result` line itself).
    pub fn flush(self, until: Instant) -> bool {
        let Out { queue, closed, done, .. } = self;
        drop(queue);
        let _ = done.recv_timeout(until.saturating_duration_since(Instant::now()));
        closed.load(SeqCst)
    }
}

/// The panic hook's line to fd 2: `henki: MSG`, escaped. A helper thread writes it and the hook waits at most 100 ms, so
/// a stderr nobody reads (or the writer thread blocked on one, holding std's stderr lock) never holds up the abort.
#[allow(clippy::disallowed_methods)] // the hook cannot rely on the writer thread, which may be the one that panicked
pub fn panic_line(msg: &str) {
    let (line, (done, wait)) = (format!("henki: {}\n", escape(msg)), mpsc::channel::<()>());
    let write = move || drop((std::io::stderr().write_all(line.as_bytes()), done)); // `wait` ends when `done` drops
    let _ = std::thread::Builder::new().spawn(write).map(|_| wait.recv_timeout(std::time::Duration::from_millis(100)));
}

#[cfg(test)]
mod tests;
