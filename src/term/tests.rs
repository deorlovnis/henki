#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use serde_json::json;
use std::io;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

fn ch(u: u32) -> char {
    char::from_u32(u).unwrap()
}

#[test]
fn term_escape_every_range() {
    for &(a, b) in ESCAPE {
        for u in [a, b] {
            assert!(escaped(ch(u)), "U+{u:04x} in the table");
            let want = if u < 0x100 { format!("\\x{u:02x}") } else { format!("\\u{{{u:04x}}}") };
            assert_eq!(escape(&ch(u).to_string()), want);
        }
    }
    // Neighbours just outside the ranges stay as they are.
    for u in [0x20, 0x7E, 0xA1, 0xAC, 0xAE, 0x34E, 0x350, 0x2010, 0x2030, 0xFFFC, 0xE1000, 0x10FFFF] {
        assert!(!escaped(ch(u)), "U+{u:04x} outside the table");
    }
}

#[test]
fn term_escape_default_ignorable() {
    for u in [0x200B, 0x2066, 0xE0001] {
        assert!(escaped(ch(u)));
    }
    assert_eq!(escape("a\u{200b}b\u{2066}c\u{e0001}"), "a\\u{200b}b\\u{2066}c\\u{e0001}");
}

#[test]
fn term_escape_keeps_tab_newline() {
    let s = "a\tb\nc é 日本";
    assert!(matches!(escape(s), Cow::Borrowed(t) if t == s));
    assert!(!escaped('\t') && !escaped('\n') && !escaped(' '));
}

#[test]
fn term_escape_forms() {
    assert_eq!(escape("\x1b]52;c;ZWNobw==\x07"), "\\x1b]52;c;ZWNobw==\\x07");
    assert_eq!(escape("sh\u{202e}"), "sh\\u{202e}");
    assert_eq!(escape("\r\u{7f}\u{85}\u{a0}"), "\\x0d\\x7f\\x85\\xa0");
}

#[test]
fn term_json_escape_surrogate() {
    assert_eq!(json_line(&json!("\u{e0001}")), r#""\udb40\udc01""#);
    assert_eq!(json_line(&json!({"t": "a\u{202e}b"})), r#"{"t":"a\u202eb"}"#);
}

#[test]
fn term_json_escape_c1_del() {
    assert_eq!(json_line(&json!("\u{7f}\u{85}\u{9f}")), r#""\u007f\u0085\u009f""#);
    // serde_json's own escapes for C0 are kept; nothing raw below U+0020 survives.
    assert_eq!(json_line(&json!("\x1b[0m\t\n")), r#""\u001b[0m\t\n""#);
}

#[test]
fn json_line_roundtrip() {
    let v = json!({"a": "\u{e0001}x\u{200b}\u{7f}\u{1b}\"\\", "b": [1, -2, 3.5, null, true], "c": {"z": "日本", "y": ""}});
    let line = json_line(&v);
    assert!(!line.chars().any(escaped));
    assert!(!line.contains('\n'));
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), v);
}

#[test]
fn json_bytes_sorted() {
    let v = json!({"b": 1, "a": {"d": 2, "c": [3, {"f": 4, "e": 5}]}});
    assert_eq!(json_line(&v), r#"{"a":{"c":[3,{"e":5,"f":4}],"d":2},"b":1}"#);
}

#[test]
fn fmt_numbers() {
    let cases = [(0, "0"), (7, "7"), (999, "999"), (1_000, "1,000"), (32_768, "32,768"), (2_941_512, "2,941,512"), (100_000, "100,000")];
    for (n, s) in cases {
        assert_eq!(num(n), s);
    }
    assert_eq!(num(u64::MAX), "18,446,744,073,709,551,615");
}

#[test]
fn fmt_secs() {
    let cases = [(0, "0.0"), (49, "0.0"), (50, "0.1"), (300, "0.3"), (2_400, "2.4"), (2_449, "2.4"), (2_450, "2.5"), (120_000, "120.0")];
    for (ms, s) in cases {
        assert_eq!(secs(ms), s);
    }
    assert_eq!(secs(u64::MAX), "18446744073709551.6");
}

/// A writer into a shared log of (stream, bytes); `fail` makes every write an error of that kind.
#[derive(Clone)]
struct Log {
    lines: Arc<Mutex<Vec<(u8, String)>>>,
    fd: u8,
    fail: Option<io::ErrorKind>,
    writes: Arc<AtomicUsize>,
}

impl Log {
    fn new() -> Log {
        Log { lines: Arc::default(), fd: 1, fail: None, writes: Arc::default() }
    }
    fn on(&self, fd: u8, fail: Option<io::ErrorKind>) -> Box<dyn Write + Send> {
        Box::new(Log { fd, fail, ..self.clone() })
    }
    fn get(&self) -> Vec<(u8, String)> {
        self.lines.lock().unwrap().clone()
    }
}

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.writes.fetch_add(1, SeqCst);
        if let Some(k) = self.fail {
            return Err(k.into());
        }
        self.lines.lock().unwrap().push((self.fd, String::from_utf8(b.to_vec()).unwrap()));
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn soon() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

/// The events the writer sent, once the queue is flushed.
fn events(rx: &Receiver<Ev>) -> Vec<Ev> {
    rx.try_iter().collect()
}

#[test]
fn out_json_record_line() {
    let (tx, rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(true, tx, log.on(1, None), log.on(2, None));
    assert!(out.json);
    out.record(&json!({"t": "text", "text": "a\u{202e}"}));
    out.answer("never in --json");
    out.line("never in --json");
    out.note("never in --json");
    assert!(!out.flush(soon()));
    assert_eq!(log.get(), [(1, "{\"t\":\"text\",\"text\":\"a\\u202e\"}\n".to_string())]);
    assert!(events(&rx).is_empty());
}

#[test]
fn out_plain_answer_escaped() {
    let (tx, _rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(false, tx, log.on(1, None), log.on(2, None));
    assert!(!out.json);
    out.record(&json!({"t": "result"}));
    out.answer("done \x1b[2J\tok");
    out.flush(soon());
    assert_eq!(log.get(), [(1, "done \\x1b[2J\tok\n".to_string())]);
}

#[test]
fn out_note_prefix() {
    let (tx, _rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(false, tx, log.on(1, None), log.on(2, None));
    out.note("HTTP 429; retry 1/4 in 1.0 s \u{7}");
    out.flush(soon());
    assert_eq!(log.get(), [(2, "henki: HTTP 429; retry 1/4 in 1.0 s \\x07\n".to_string())]);
}

#[test]
fn out_line_unprefixed() {
    let (tx, _rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(false, tx, log.on(1, None), log.on(2, None));
    out.line("$ ls -> exit 0, 0.3 s");
    out.line("(gave up: stuck)");
    out.flush(soon());
    assert_eq!(log.get(), [(2, "$ ls -> exit 0, 0.3 s\n".to_string()), (2, "(gave up: stuck)\n".to_string())]);
}

#[test]
fn out_epipe_closed_once() {
    let (tx, rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(true, tx, log.on(1, Some(io::ErrorKind::BrokenPipe)), log.on(2, None));
    for i in 0..3 {
        out.record(&json!({"i": i}));
    }
    // The writer may not have run yet; wait for its event.
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Ev::OutputClosed)));
    assert!(out.closed());
    assert!(out.flush(soon()));
    assert!(events(&rx).is_empty(), "OutputClosed is sent once");
    assert_eq!(log.writes.load(SeqCst), 1, "later stdout lines are dropped, not written");
}

#[test]
fn out_stderr_error_ignored() {
    let (tx, rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(false, tx, log.on(1, None), log.on(2, Some(io::ErrorKind::BrokenPipe)));
    out.note("lost");
    out.answer("kept");
    assert!(!out.flush(soon()));
    assert_eq!(log.get(), [(1, "kept\n".to_string())]);
    assert!(events(&rx).is_empty());
}

/// A stdout that blocks until the test lets it go, like a pipe nobody reads.
struct Stuck(Receiver<()>);

impl Write for Stuck {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        let _ = self.0.recv();
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn out_flush_bounded() {
    let (tx, _rx) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let out = Out::with(true, tx, Box::new(Stuck(hold)), Box::new(io::sink()));
    out.record(&json!({"t": "result"}));
    let t0 = Instant::now();
    assert!(out.flush(t0 + Duration::from_millis(200)), "a line still queued at the bound did not get out");
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(200) && took < Duration::from_secs(2), "{took:?}");
    drop(release);
}

#[test]
fn out_flush_reports_closed() {
    // stdout takes the first line, then breaks on the last queued one (the `result` line): flush still says closed.
    struct Once(usize);
    impl Write for Once {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0 += 1;
            if self.0 > 1 { Err(io::ErrorKind::BrokenPipe.into()) } else { Ok(b.len()) }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (tx, rx) = mpsc::channel();
    let out = Out::with(true, tx, Box::new(Once(0)), Box::new(io::sink()));
    out.record(&json!({"t": "session"}));
    out.record(&json!({"t": "result"}));
    assert!(out.flush(soon()));
    assert!(matches!(events(&rx)[..], [Ev::OutputClosed]));
}

#[test]
fn out_order_across_streams() {
    let (tx, _rx) = mpsc::channel();
    let log = Log::new();
    let out = Out::with(false, tx, log.on(1, None), log.on(2, None));
    out.note("one");
    out.line("$ two -> exit 0, 0.1 s");
    out.answer("three");
    out.note("four");
    out.answer("five");
    out.flush(soon());
    let got: Vec<(u8, String)> = log.get();
    let want = [(2, "henki: one\n"), (2, "$ two -> exit 0, 0.1 s\n"), (1, "three\n"), (2, "henki: four\n"), (1, "five\n")];
    assert_eq!(got, want.map(|(fd, s)| (fd, s.to_string())));
}

#[test]
fn out_flush_past_deadline() {
    let (tx, _rx) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let until = Instant::now();
    let out = Out::with(true, tx, Box::new(Stuck(hold)), Box::new(io::sink()));
    out.record(&json!({"t": "result"}));
    assert!(out.flush(until), "the result was still queued");
    assert!(until.elapsed() < Duration::from_millis(500), "{:?}", until.elapsed());
    drop(release);
}

#[test]
fn panic_line_bounded() {
    // Another thread holds std's stderr lock, as the writer thread does while it is blocked on a full stderr pipe.
    let (locked, is_locked) = mpsc::channel();
    let (release, hold) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _guard = std::io::stderr().lock();
        locked.send(()).unwrap();
        let _ = hold.recv();
    });
    is_locked.recv().unwrap();
    let t0 = Instant::now();
    panic_line("a test of the panic line, not a failure");
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    drop(release);
    holder.join().unwrap();
}

/// `n` strings of up to 11 random code points: ASCII, the escaped ranges, and any plane.
fn random_strings(n: usize) -> Vec<String> {
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut point = move || {
        let r = next();
        let (a, b) = ESCAPE[(r >> 8) as usize % ESCAPE.len()];
        char::from_u32(match r % 4 {
            0 => (r >> 16) as u32 % 0x80,
            1 => a + (r >> 32) as u32 % (b - a + 1),
            _ => (r >> 16) as u32 % 0x11_0000,
        })
    };
    (0..n).map(|i| (0..i % 12).filter_map(|_| point()).collect()).collect()
}

#[test]
fn term_escape_never_panics() {
    for s in random_strings(50_000) {
        let want: String = s
            .chars()
            .map(|c| match c as u32 {
                _ if !escaped(c) => c.to_string(),
                u if u < 0x100 => format!("\\x{u:02x}"),
                u => format!("\\u{{{u:04x}}}"),
            })
            .collect();
        let got = escape(&s);
        assert!(!got.chars().any(escaped), "{s:?}");
        assert_eq!(got, want);
    }
}

#[test]
fn json_line_roundtrip_fuzz() {
    for s in random_strings(50_000) {
        let mut m = serde_json::Map::new();
        m.insert(s.clone(), json!([s, 1]));
        let v = Value::Object(m);
        let line = json_line(&v);
        assert!(!line.chars().any(escaped) && !line.contains('\n'), "{line:?}");
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), v);
    }
}
