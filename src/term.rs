//! The only writer to stdout and stderr: one escape table for every path, a JSON line writer, and a writer thread so a
//! reader that stops reading never blocks the main thread. Also the REPL's input: the terminal reader and the line editor.
use crate::{sys, types::Ev, types::lock};
use serde_json::Value;
use std::sync::{Arc, Condvar, Mutex, PoisonError, atomic::AtomicBool, atomic::Ordering::SeqCst, mpsc, mpsc::Receiver, mpsc::Sender};
use std::{borrow::Cow, fmt::Write as _, io, io::Read, io::Write, time::Duration, time::Instant};

/// Plain `run`, `run --json`, or the REPL.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Plain,
    Json,
    Repl,
}

/// Written before every approval display: soft reset, attributes off, no scroll region, main screen, line wrap on, then
/// the cursor to the start of the bottom row and one row scrolled in, bracketed paste off. Removing the scroll region also
/// moves the cursor to the top-left corner, where the display would be drawn over old text; from a fresh bottom row every
/// display line lands on a blank row, below everything already on the screen.
pub const RESET: &str = "\x1b[!p\x1b[0m\x1b[r\x1b[?1049l\x1b[?7h\x1b[999;1H\x1bD\x1b[?2004l";
/// The start and end of a paste, as a terminal with bracketed paste on (`paste`) sends it.
const PASTE: [&[u8]; 2] = [b"\x1b[200~", b"\x1b[201~"];
/// The most a line holds; more is dropped.
pub const LINE_MAX: usize = 1 << 20;

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

/// Where a queued piece goes: a stdout line, stderr, or a streamed piece of the REPL's answer (stdout, no newline).
const STDOUT: u8 = 1;
const STDERR: u8 = 2;
const DELTA: u8 = 3;

/// The queue to the writer thread; pieces leave in the order they were queued, across both streams.
pub struct Out {
    mode: Mode,
    tty: bool,
    queue: Sender<(u8, Vec<u8>)>,
    closed: Arc<AtomicBool>,
    done: Receiver<()>,
}

impl Out {
    /// The writer thread over fd 1 and fd 2. `main` calls it after argument parsing, when the mode is known.
    #[allow(clippy::disallowed_methods)] // the one place that takes fd 1 and fd 2
    pub fn start(mode: Mode, tx: Sender<Ev>) -> Out {
        Out::with(mode, tx, Box::new(std::io::stdout()), Box::new(std::io::stderr()), crate::sys::isatty(2))
    }

    /// The same over any two writers (tests); `tty`: stderr is a terminal (the reset is drawn). The first stdout error
    /// marks stdout closed, sends `Ev::OutputClosed` once and drops every later stdout piece; stderr errors are ignored.
    /// The writer keeps one fact, `open`: the last stdout piece was a delta not ending in "\n". Anything but a delta first
    /// ends that line, so a cut answer keeps its text and what follows starts on its own row.
    pub fn with(mode: Mode, tx: Sender<Ev>, mut out: Box<dyn Write + Send>, mut err: Box<dyn Write + Send>, tty: bool) -> Out {
        let (queue, pieces) = mpsc::channel::<(u8, Vec<u8>)>();
        let (end, done) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&closed);
        let writer = move || {
            let mut open = false;
            let mut put = |b: &[u8]| {
                if !c.load(SeqCst) && out.write_all(b).and_then(|()| out.flush()).is_err() {
                    c.store(true, SeqCst);
                    let _ = tx.send(Ev::OutputClosed);
                }
            };
            for (to, b) in pieces {
                if to != DELTA && std::mem::take(&mut open) {
                    put(b"\n");
                }
                match to {
                    STDERR => drop(err.write_all(&b).and_then(|()| err.flush())),
                    _ => put(&b),
                }
                if to == DELTA && !b.is_empty() {
                    open = !b.ends_with(b"\n");
                }
            }
            let _ = end.send(());
        };
        if std::thread::Builder::new().name("output".into()).spawn(writer).is_err() {
            closed.store(true, SeqCst); // nothing can be written: report stdout closed rather than lose lines silently
        }
        Out { mode, tty, queue, closed, done }
    }

    /// Queues `piece()` for `to` when `when` holds; nothing is built otherwise.
    fn send(&self, to: u8, when: bool, piece: impl FnOnce() -> String) {
        if when {
            let _ = self.queue.send((to, piece().into_bytes()));
        }
    }

    pub fn json(&self) -> bool {
        self.mode == Mode::Json
    }

    pub fn repl(&self) -> bool {
        self.mode == Mode::Repl
    }

    /// `--json` only: one `json_line` + "\n" to stdout. Otherwise nothing.
    pub fn record(&self, rec: &Value) {
        self.send(STDOUT, self.json(), || json_line(rec) + "\n");
    }

    /// Plain mode only: the final answer, escaped, + "\n" to stdout (`--json` stdout holds records only; the REPL
    /// streams its answer with `delta`).
    pub fn answer(&self, text: &str) {
        self.send(STDOUT, self.mode == Mode::Plain, || format!("{}\n", escape(text)));
    }

    /// Plain mode and the REPL: TEXT, escaped, + "\n" to stderr, no prefix: the call lines (`$ ls -> exit 0, 0.3 s`) and
    /// the outcome line. `--json`: nothing (stderr is kept for a crash message).
    pub fn line(&self, text: &str) {
        self.send(STDERR, !self.json(), || format!("{}\n", escape(text)));
    }

    /// Plain mode and the REPL: `henki: TEXT`, escaped, + "\n" to stderr (notices and errors); `line` with the prefix.
    pub fn note(&self, text: &str) {
        self.line(&format!("henki: {text}"));
    }

    /// The REPL: a piece of the streamed answer, escaped, to stdout with no newline.
    pub fn delta(&self, text: &str) {
        self.send(DELTA, self.repl(), || escape(text).into_owned());
    }

    /// The REPL with stderr a terminal: `RESET` to stderr, unescaped.
    pub fn reset(&self) {
        self.send(STDERR, self.repl() && self.tty, || RESET.into());
    }

    /// The REPL: the line editor's echo, already escaped, to stderr as it is.
    pub fn echo(&self, text: String) {
        self.send(STDERR, self.repl() && !text.is_empty(), || text);
    }

    /// stdout gave EPIPE (or any write error) so far.
    pub fn closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    /// Close the queue and wait for the writer until `until`, then return regardless. Returns whether some line did not
    /// get out: stdout was closed at any point (on the `result` line itself, too), or the writer was still writing at
    /// `until` because the reader is too slow.
    pub fn flush(self, until: Instant) -> bool {
        let Out { queue, closed, done, .. } = self;
        drop(queue);
        done.recv_timeout(until.saturating_duration_since(Instant::now())).is_err() || closed.load(SeqCst)
    }
}

/// The rows `lines` take on a terminal `cols` wide after escaping (an ASCII character is one column, a tab 8, the most it
/// can move, any other character two); at least one per line. A character that does not fit in what is left of a row
/// starts the next one, as a terminal never splits a wide character.
pub fn rows(lines: &[String], cols: usize) -> usize {
    let put = |(rows, col): (usize, usize), c: char| {
        let w = width(c.encode_utf8(&mut [0; 4]));
        if col > 0 && col + w > cols { (rows + 1, w) } else { (rows, col + w) }
    };
    lines.iter().map(|l| escape(l).chars().fold((1, 0), put).0).sum()
}

/// The columns `s` takes at most: an ASCII character one, a tab eight, any other character two.
fn width(s: &str) -> usize {
    s.chars().map(|c| if c == '\t' { 8 } else { 2 - usize::from(c.is_ascii()) }).sum()
}

/// The REPL's terminal reader. It reads only while MAIN waits for input, so what the user types into a `!cmd` reaches
/// the command. Each read goes out as `Ev::Input { tag, bytes }` (None: EOF or an error) with the tag MAIN asked with
/// when it returned: 0 when MAIN no longer waits, and MAIN drops it as stale.
pub struct Reader(Arc<(Mutex<u32>, Condvar)>);

impl Reader {
    /// The thread over `input`: stdin, or any input in tests. Reads of 64 KiB go past std's buffer for stdin, which stays
    /// empty.
    pub fn with(mut input: Box<dyn Read + Send>, tx: Sender<Ev>) -> io::Result<Reader> {
        let wanted = Arc::new((Mutex::new(0), Condvar::new()));
        let w = Arc::clone(&wanted);
        let read = move || {
            let mut b = vec![0; 64 << 10];
            loop {
                drop(w.1.wait_while(lock(&w.0), |t| *t == 0).unwrap_or_else(PoisonError::into_inner));
                let bytes = match input.read(&mut b) {
                    // A terminal left non-blocking by a command that shares it has nothing yet: that is not EOF. Read
                    // again after 50 ms, or at once when MAIN asks again.
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        drop(w.1.wait_timeout(lock(&w.0), Duration::from_millis(50)));
                        continue;
                    }
                    Ok(n @ 1..) => b.get(..n).map(<[u8]>::to_vec),
                    _ => None,
                };
                if tx.send(Ev::Input { tag: std::mem::take(&mut *lock(&w.0)), bytes }).is_err() {
                    return;
                }
            }
        };
        std::thread::Builder::new().name("input".into()).spawn(read)?;
        Ok(Reader(wanted))
    }

    /// Read once for `tag`; `ask(0)` stops waiting: what a read under way returns later is stale.
    pub fn ask(&self, tag: u32) {
        *lock(&self.0.0) = tag;
        self.0.1.notify_one();
    }
}

/// Bracketed paste on or off, written to stderr right away when it is a terminal, so it holds before a `!cmd` starts.
#[allow(clippy::disallowed_methods)] // ordered before a command's own output, which the writer thread cannot do
pub fn paste(on: bool) {
    let _ = sys::isatty(2).then(|| std::io::stderr().write_all(if on { b"\x1b[?2004h" } else { b"\x1b[?2004l" }));
}

/// The REPL's line editor over what the terminal sends in the editor's modes: printable text, Backspace and DEL (one
/// whole character), Ctrl-U (the whole line), Enter, Ctrl-D on an empty line (EOF), and a bracketed paste kept whole,
/// newlines included. Every other control byte and escape sequence is dropped. It echoes each character escaped (a
/// newline as "\n", which the terminal's output processing makes CR LF, as for all henki writes) and wraps rows itself
/// before the last column, counting the most a character can take (`width`), so the terminal never wraps a
/// row and henki knows how many the line takes: Backspace and Ctrl-U go up to the first and draw the line again.
#[derive(Default)]
pub struct Edit {
    pub text: String,
    /// The line holds a paste or a newline (one that came with more input): it is never a command or an answer.
    pub pasted: bool,
    /// The bytes dropped past `LINE_MAX`.
    pub over: usize,
    /// What to write to the terminal next.
    pub echo: String,
    /// The prompt, escaped, as the first row starts with it ("" when the line starts on the row after it).
    prompt: String,
    cols: usize,
    /// The rows below the first that the line takes, and the column after its last character.
    rows: usize,
    col: usize,
    utf8: Vec<u8>,
    esc: Vec<u8>,
    paste: bool,
}

impl Edit {
    /// A line after `prompt` (`> `, `run? [y/N] `), which it writes first, from column 0 of a terminal `cols` wide. Past a
    /// prompt that fills its row, the line starts on the next one.
    pub fn new(prompt: &str, cols: usize) -> Edit {
        let (shown, mut e) = (escape(prompt).into_owned(), Edit { cols, ..Edit::default() });
        (e.prompt, e.echo) = if width(&shown) < cols { (shown.clone(), shown) } else { (String::new(), shown + "\n") };
        e.col = width(&e.prompt);
        e
    }

    /// Takes `b`, read from the terminal; `more`: input waits beyond `b`. Some(true): Enter sent the line; Some(false): EOF.
    pub fn feed(&mut self, b: &[u8], more: impl Fn() -> bool) -> Option<bool> {
        let done = b.iter().enumerate().find_map(|(i, &c)| self.byte(c, &|| i + 1 < b.len() || more()));
        self.decode(done.is_some());
        done
    }

    fn byte(&mut self, c: u8, more: &dyn Fn() -> bool) -> Option<bool> {
        // Text: printable bytes, and in a paste any byte up to its end.
        if self.paste || self.esc.is_empty() && c >= 0x20 && c != 0x7f {
            self.utf8.push(c);
            if self.paste && self.utf8.ends_with(PASTE[1]) {
                self.utf8.truncate(self.utf8.len() - PASTE[1].len());
                self.decode(true);
                (self.paste, self.pasted) = (false, true);
            }
            return None;
        }
        if c == 0x1b || !self.esc.is_empty() {
            self.esc.push(c);
            let e = &self.esc[..];
            // CSI (ESC [ ... final byte), SS3 (ESC O x) or ESC x, dropped whole; a second ESC starts over.
            if e == b"\x1b\x1b" {
                self.esc.pop();
            } else if !(matches!(e, [_] | [_, b'[' | b'O']) || matches!(e, [_, b'[', .., f] if !(0x40..=0x7e).contains(f) && e.len() < 32)) {
                self.paste = std::mem::take(&mut self.esc) == PASTE[0];
            }
            return None;
        }
        self.decode(true);
        match c {
            b'\r' | b'\n' if more() => self.put('\n'),
            b'\r' | b'\n' => return Some(true).inspect(|_| self.echo.push('\n')),
            0x7f | 0x08 => self.redo(self.text.chars().count().saturating_sub(1)),
            0x15 => self.redo(0),
            0x04 if self.text.is_empty() => return Some(false),
            _ => {}
        }
        None
    }

    /// The UTF-8 bytes so far as characters; a code point cut at the end of a read waits for the next one, unless `all`,
    /// and in a paste so do the last 5 bytes, which may be the start of its end.
    fn decode(&mut self, all: bool) {
        let b = std::mem::take(&mut self.utf8);
        let n = if all { b.len() } else { head_end(&b[..b.len().saturating_sub(5 * usize::from(self.paste))]) };
        String::from_utf8_lossy(&b[..n]).chars().for_each(|c| self.put(c));
        self.utf8 = b[n..].to_vec();
    }

    /// One character at the end, echoed; a pasted CR is a newline. A line with a newline is never a command or an answer.
    fn put(&mut self, c: char) {
        let c = if c == '\r' { '\n' } else { c };
        if self.text.len() + c.len_utf8() > LINE_MAX {
            self.over += c.len_utf8();
            return;
        }
        let shown = escape(c.encode_utf8(&mut [0; 4])).into_owned();
        let w = width(&shown);
        // A newline ends the row; a character that does not fit in what is left of it starts the next one.
        if c == '\n' || self.col > 0 && self.col + w > self.cols {
            self.echo.push('\n');
            (self.rows, self.col, self.pasted) = (self.rows + 1, 0, self.pasted || c == '\n');
        }
        self.text.push(c);
        if c != '\n' {
            self.echo.push_str(&shown);
            self.col += w;
        }
    }

    /// Backspace (`keep` all but the last character) and Ctrl-U (`keep` 0): up to the first row, everything from there
    /// cleared, the prompt again and the line's first `keep` characters.
    fn redo(&mut self, keep: usize) {
        let text = std::mem::take(&mut self.text);
        let _ = write!(self.echo, "{}\r\x1b[J{}", "\x1b[A".repeat(self.rows), self.prompt);
        (self.rows, self.col) = (0, width(&self.prompt));
        text.chars().take(keep).for_each(|c| self.put(c));
    }
}

/// The length of `b` without a code point cut short at its end.
pub fn head_end(b: &[u8]) -> usize {
    let n = b.len();
    match (n.saturating_sub(3)..n).rev().find(|&i| b[i] & 0xC0 != 0x80) {
        Some(i) if n - i < b[i].leading_ones().max(1) as usize => i, // a lead byte's leading ones are its width
        _ => n,
    }
}

/// The panic hook's line to fd 2: `henki: MSG`, escaped. A helper thread writes it and the hook waits at most 100 ms, so
/// a stderr nobody reads (or the writer thread blocked on one, holding std's stderr lock) never holds up the abort.
#[allow(clippy::disallowed_methods)] // the hook cannot rely on the writer thread, which may be the one that panicked
pub fn panic_line(msg: &str) {
    // A panic at the prompt leaves no bracketed paste on (the hook has put the terminal modes back).
    let off = if sys::isatty(2) { "\x1b[?2004l" } else { "" };
    let (line, (done, wait)) = (format!("{off}henki: {}\n", escape(msg)), mpsc::channel::<()>());
    let write = move || drop((std::io::stderr().write_all(line.as_bytes()), done)); // `wait` ends when `done` drops
    let _ = std::thread::Builder::new().spawn(write).map(|_| wait.recv_timeout(std::time::Duration::from_millis(100)));
}

#[cfg(test)]
mod tests;
