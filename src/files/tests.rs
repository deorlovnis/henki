#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use crate::tools::{ALL, Ctx, target, validate};
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::channel;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Held by the test that measures the peak memory and by every test that holds megabytes, so neither sees the other.
static MEMORY: Mutex<()> = Mutex::new(());

fn memory() -> MutexGuard<'static, ()> {
    MEMORY.lock().unwrap_or_else(|e| e.into_inner())
}

/// A fresh project directory under the temp dir (as its realpath), removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-files-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(fs::canonicalize(p).unwrap())
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn file(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }
    /// A validated call and its target, as the turn prepares them.
    fn prepare(&self, name: &str, args: Value) -> (Call, Target) {
        let mut c = Call { id: "c1".into(), name: name.into(), args, raw: None, error: None };
        validate(&mut c, &ALL);
        assert_eq!(c.error, None);
        let cx = Ctx { cwd: &self.0, root: &self.0, home: &self.0, protected: &[] };
        let t = target(c.args["path"].as_str().unwrap(), name != "read", &cx).unwrap();
        (c, t)
    }
    /// Runs the call with the default cap: (text, failed).
    fn go(&self, name: &str, args: Value) -> (String, bool) {
        self.go_cap(name, args, 16_384)
    }
    fn go_cap(&self, name: &str, args: Value, cap: usize) -> (String, bool) {
        let (c, t) = self.prepare(name, args);
        let out = run(&c, &t, cap);
        assert_eq!(out.bytes, out.text.len() as u64);
        let End::File { failed } = out.end else { panic!("{:?}", out.end) };
        (out.text, failed)
    }
    /// The names in the directory that look like a write's temp file.
    fn temps(&self, dir: &str) -> Vec<String> {
        fs::read_dir(self.join(dir)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).filter(|n| n.contains(".henki-")).collect()
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().mode() & 0o7777
}

/// The peak resident set of this process, in KiB.
#[cfg(target_os = "linux")]
fn hwm_kib() -> u64 {
    let s = fs::read_to_string("/proc/self/status").unwrap();
    s.lines().find_map(|l| l.strip_prefix("VmHWM:")).unwrap().trim().trim_end_matches(" kB").parse().unwrap()
}

#[test]
fn read_bounds_limit() {
    let tmp = Tmp::new();
    tmp.file("f.txt", numbered(10));
    let len = numbered(10).len();
    assert_eq!(tmp.go("read", json!({"path": "f.txt", "limit": 3})), (format!("line 1\nline 2\nline 3\n[lines 1-3 shown; file is {len} bytes; continue with offset=4]\n"), false));
    assert_eq!(tmp.go("read", json!({"path": "f.txt", "offset": 9, "limit": 3})), ("line 9\nline 10\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "f.txt"})), (numbered(10), false));
    // The default is 2,000 lines.
    tmp.file("g.txt", "x\n".repeat(2_500));
    let (text, _) = tmp.go("read", json!({"path": "g.txt"}));
    assert!(text.ends_with("x\n[lines 1-2000 shown; file is 5,000 bytes; continue with offset=2001]\n"), "{}", &text[text.len() - 90..]);
}

#[test]
fn read_bounds_bytes() {
    let tmp = Tmp::new();
    let line = "y".repeat(100);
    tmp.file("f.txt", format!("{line}\n").repeat(1000));
    // A cap of 1,000 bytes holds nine 101-byte lines.
    let (text, failed) = tmp.go_cap("read", json!({"path": "f.txt"}), 1_000);
    assert!(!failed);
    assert_eq!(text, format!("{}[lines 1-9 shown; file is 101,000 bytes; continue with offset=10]\n", format!("{line}\n").repeat(9)));
    // A big cap is still held to 32 KiB.
    let (text, _) = tmp.go_cap("read", json!({"path": "f.txt"}), 1 << 20);
    let body = text.split("[lines").next().unwrap();
    assert!(body.len() <= READ_MAX && body.len() > READ_MAX - 101, "{}", body.len());
    assert!(text.ends_with("[lines 1-324 shown; file is 101,000 bytes; continue with offset=325]\n"), "{}", &text[text.len() - 80..]);
}

#[test]
fn read_line_cut_never_empty() {
    let tmp = Tmp::new();
    tmp.file("one.txt", "z".repeat(5_000));
    assert_eq!(tmp.go_cap("read", json!({"path": "one.txt"}), 1_000), (format!("{} [line cut: 5,000 bytes]\n", "z".repeat(1_000)), false));
    // A short line, then a long one: the page stops before it, and the next page cuts it.
    tmp.file("two.txt", format!("short\n{}\nend\n", "z".repeat(5_000)));
    let len = 5_000 + 11;
    assert_eq!(tmp.go_cap("read", json!({"path": "two.txt"}), 1_000), (format!("short\n[lines 1-1 shown; file is {} bytes; continue with offset=2]\n", num(len)), false));
    assert_eq!(
        tmp.go_cap("read", json!({"path": "two.txt", "offset": 2}), 1_000),
        (format!("{} [line cut: 5,000 bytes]\n[lines 2-2 shown; file is {} bytes; continue with offset=3]\n", "z".repeat(1_000), num(len)), false)
    );
    // Cut on a character boundary: no replacement character, no trailer.
    tmp.file("wide.txt", "é".repeat(3_000));
    let (text, _) = tmp.go_cap("read", json!({"path": "wide.txt"}), 1_001);
    assert_eq!(text, format!("{} [line cut: 6,000 bytes]\n", "é".repeat(500)));
}

#[test]
fn read_offset_past_end() {
    let tmp = Tmp::new();
    tmp.file("f.txt", numbered(120));
    assert_eq!(tmp.go("read", json!({"path": "f.txt", "offset": 500})), ("offset 500 is past the end (file has 120 lines)".into(), true));
    assert_eq!(tmp.go("read", json!({"path": "f.txt", "offset": 121})), ("offset 121 is past the end (file has 120 lines)".into(), true));
    assert_eq!(tmp.go("read", json!({"path": "f.txt", "offset": 120})), ("line 120\n".into(), false));
    // A last line without a newline is a line.
    tmp.file("g.txt", "a\nb");
    assert_eq!(tmp.go("read", json!({"path": "g.txt", "offset": 3})), ("offset 3 is past the end (file has 2 lines)".into(), true));
}

#[test]
fn read_more_remains() {
    // A 1 GiB sparse file: text first, then a hole. A page costs what it reads, not the file.
    let tmp = Tmp::new();
    let p = tmp.file("big.txt", "0123456789\n".repeat(1_000));
    fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(1 << 30).unwrap();
    let t0 = Instant::now();
    let (text, failed) = tmp.go("read", json!({"path": "big.txt", "offset": 201, "limit": 200}));
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert!(!failed);
    assert_eq!(text, format!("{}[lines 201-400 shown; file is 1,073,741,824 bytes; continue with offset=401]\n", "0123456789\n".repeat(200)));
    let (text, _) = tmp.go("read", json!({"path": "big.txt", "limit": 5}));
    assert_eq!(text, format!("{}[lines 1-5 shown; file is 1,073,741,824 bytes; continue with offset=6]\n", "0123456789\n".repeat(5)));
}

#[test]
fn read_empty_file() {
    let tmp = Tmp::new();
    tmp.file("e.txt", "");
    assert_eq!(tmp.go("read", json!({"path": "e.txt"})), ("(empty file)".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "e.txt", "offset": 5})), ("(empty file)".into(), false));
    // One empty line is not an empty file.
    tmp.file("n.txt", "\n");
    assert_eq!(tmp.go("read", json!({"path": "n.txt"})), ("\n".into(), false));
}

#[test]
fn read_invalid_utf8_trailer() {
    let tmp = Tmp::new();
    tmp.file("f.txt", b"caf\xe9\nok\n");
    assert_eq!(tmp.go("read", json!({"path": "f.txt"})), ("caf\u{FFFD}\nok\n[invalid UTF-8 replaced with U+FFFD]\n".into(), false));
    tmp.file("g.txt", b"\xff\nb\nc\n");
    assert_eq!(
        tmp.go("read", json!({"path": "g.txt", "limit": 1})),
        ("\u{FFFD}\n[lines 1-1 shown; file is 6 bytes; continue with offset=2]\n[invalid UTF-8 replaced with U+FFFD]\n".into(), false)
    );
}

#[test]
fn read_binary() {
    let tmp = Tmp::new();
    tmp.file("b.bin", b"ab\0cd");
    assert_eq!(tmp.go("read", json!({"path": "b.bin"})), ("binary file (5 bytes); use the shell".into(), true));
    // Only the first 8 KiB decide.
    let mut late = vec![b'a'; 9_000];
    late.push(0);
    tmp.file("late.txt", &late);
    assert!(!tmp.go("read", json!({"path": "late.txt"})).1);
}

#[test]
fn read_refuses_fifo() {
    let tmp = Tmp::new();
    assert!(std::process::Command::new("mkfifo").arg(tmp.join("fifo")).status().unwrap().success());
    let (c, t) = tmp.prepare("read", json!({"path": "fifo"}));
    let (tx, rx) = channel();
    start(c, t, 16_384, 7, &tx);
    // Nobody writes to the fifo: a blocking open would never answer.
    let Ev::Tool { tag, out } = rx.recv_timeout(Duration::from_secs(5)).expect("the read blocked") else { panic!() };
    assert_eq!((tag, out.text.as_str(), out.end), (7, "not a regular file: fifo (fifo)", End::File { failed: true }));
}

#[test]
fn read_dir_lists() {
    let tmp = Tmp::new();
    tmp.file("b.txt", "x");
    tmp.file(".hidden", "x");
    tmp.file("a/inner.txt", "x");
    symlink(tmp.join("a"), tmp.join("link")).unwrap();
    assert_eq!(tmp.go("read", json!({"path": "."})), (".hidden\na/\nb.txt\nlink\n[directory, 4 entries]\n".into(), false));
    // Sorted bytewise on the names: the directory `a` before `a.txt`, uppercase first.
    tmp.file("a.txt", "x");
    tmp.file("Z", "x");
    assert_eq!(tmp.go("read", json!({"path": "."})).0, ".hidden\nZ\na/\na.txt\nb.txt\nlink\n[directory, 6 entries]\n");
    fs::create_dir(tmp.join("empty")).unwrap();
    assert_eq!(tmp.go("read", json!({"path": "empty"})), ("[directory, 0 entries]\n".into(), false));
}

#[test]
fn read_dir_newline_name() {
    let tmp = Tmp::new();
    tmp.file("d/a\nb", "x");
    tmp.file("d/c", "x");
    // One entry, one line: the newline is shown as `\n`, and the count and the paging agree.
    assert_eq!(tmp.go("read", json!({"path": "d"})), ("a\\nb\nc\n[directory, 2 entries]\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "d", "limit": 1})), ("a\\nb\n[entries 1-1 shown of 2; continue with offset=2]\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "d", "offset": 2})), ("c\n[directory, 2 entries]\n".into(), false));
}

#[test]
fn read_dir_pages() {
    let tmp = Tmp::new();
    for i in 1..=5 {
        tmp.file(&format!("d/e{i}"), "x");
    }
    assert_eq!(tmp.go("read", json!({"path": "d", "limit": 2})), ("e1\ne2\n[entries 1-2 shown of 5; continue with offset=3]\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "d", "offset": 3, "limit": 2})), ("e3\ne4\n[entries 3-4 shown of 5; continue with offset=5]\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "d", "offset": 5})), ("e5\n[directory, 5 entries]\n".into(), false));
    assert_eq!(tmp.go("read", json!({"path": "d", "offset": 9})), ("offset 9 is past the end (directory has 5 entries)".into(), true));
    // The byte cap holds for listings too.
    assert_eq!(tmp.go_cap("read", json!({"path": "d"}), 7).0, "e1\ne2\n[entries 1-2 shown of 5; continue with offset=3]\n");
}

#[test]
fn read_long_line_bounded() {
    let _alone = memory();
    let tmp = Tmp::new();
    // Written in chunks, so making the file does not raise the peak the check below measures.
    let mut f = fs::File::create(tmp.join("long.txt")).unwrap();
    for _ in 0..512 {
        f.write_all(&[b'q'; 64 << 10]).unwrap();
    }
    f.write_all(b"\nnext\n").unwrap();
    drop(f);
    #[cfg(target_os = "linux")]
    let before = hwm_kib();
    let (text, failed) = tmp.go("read", json!({"path": "long.txt"}));
    #[cfg(target_os = "linux")]
    assert!(hwm_kib() < before + 8 * 1024, "peak grew by {} KiB", hwm_kib() - before);
    assert!(!failed);
    assert_eq!(text, format!("{} [line cut: 33,554,432 bytes]\n[lines 1-1 shown; file is 33,554,438 bytes; continue with offset=2]\n", "q".repeat(16_384)));
}

#[test]
fn write_atomic_created() {
    let tmp = Tmp::new();
    assert_eq!(tmp.go("write", json!({"path": "new.txt", "content": "hello\n"})), ("wrote 6 bytes to new.txt (created)".into(), false));
    assert_eq!(fs::read(tmp.join("new.txt")).unwrap(), b"hello\n");
    // Created 0644 and the umask applies: never wider than 0644, never executable.
    assert_eq!(mode(&tmp.join("new.txt")) & !0o644, 0);
    assert!(tmp.temps(".").is_empty());
}

#[test]
fn write_new_mode_umask() {
    let tmp = Tmp::new();
    // References made the usual way in the same process: a file opened with mode 0644, a directory with 0777.
    fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(tmp.join("ref.txt")).unwrap();
    fs::create_dir(tmp.join("ref.d")).unwrap();
    tmp.go("write", json!({"path": "new/new.txt", "content": "x"}));
    assert_eq!(mode(&tmp.join("new/new.txt")), mode(&tmp.join("ref.txt")));
    assert_eq!(mode(&tmp.join("new")), mode(&tmp.join("ref.d")));
}

#[test]
fn write_temp_private_when_replacing() {
    // While it replaces a file the temp holds the new bytes before the old mode is applied: only the owner may read it.
    assert_eq!(temp_mode(true), 0o600);
    // A new file is created as it will stay, 0644 less the umask.
    assert_eq!(temp_mode(false), 0o644);
}

#[test]
fn write_atomic_replaced() {
    let tmp = Tmp::new();
    let p = tmp.file("f.txt", "old content\n");
    let ino = fs::metadata(&p).unwrap().ino();
    assert_eq!(tmp.go("write", json!({"path": "f.txt", "content": "new"})), ("wrote 3 bytes to f.txt (replaced)".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"new");
    // A new inode renamed over the name: a reader never sees a half-written file.
    assert_ne!(fs::metadata(&p).unwrap().ino(), ino);
    assert!(tmp.temps(".").is_empty());
}

#[test]
fn write_mode_kept() {
    let tmp = Tmp::new();
    let p = tmp.file("run.sh", "#!/bin/sh\n");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o750)).unwrap();
    tmp.go("write", json!({"path": "run.sh", "content": "#!/bin/sh\necho hi\n"}));
    assert_eq!(mode(&p), 0o750);
    fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    tmp.go("edit", json!({"path": "run.sh", "old_text": "hi", "new_text": "ho"}));
    assert_eq!(mode(&p), 0o640);
}

#[test]
fn write_setuid_dropped() {
    let tmp = Tmp::new();
    let p = tmp.file("tool", "x");
    // The sticky bit goes too: it means nothing on a file, and macOS refuses to set it on one.
    for m in [0o4755, 0o2755, 0o6755, 0o1755] {
        if fs::set_permissions(&p, fs::Permissions::from_mode(m)).is_err() || mode(&p) != m {
            continue; // the system, the file system or the group does not keep this bit
        }
        tmp.go("write", json!({"path": "tool", "content": "y"}));
        assert_eq!(mode(&p), 0o755, "from {m:o}");
    }
}

#[test]
fn write_keeps_symlink() {
    let tmp = Tmp::new();
    let real = tmp.file("dotfiles/real.conf", "original\n");
    symlink(&real, tmp.join("link.conf")).unwrap();
    assert_eq!(tmp.go("write", json!({"path": "link.conf", "content": "edited\n"})), ("wrote 7 bytes to link.conf (replaced)".into(), false));
    assert!(fs::symlink_metadata(tmp.join("link.conf")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(tmp.join("link.conf")).unwrap(), real);
    assert_eq!(fs::read(&real).unwrap(), b"edited\n");
    tmp.go("edit", json!({"path": "link.conf", "old_text": "edited", "new_text": "again"}));
    assert!(fs::symlink_metadata(tmp.join("link.conf")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&real).unwrap(), b"again\n");
}

#[test]
fn write_parent_swap() {
    let tmp = Tmp::new();
    tmp.file("d/f.txt", "old");
    let (c, t) = tmp.prepare("write", json!({"path": "d/f.txt", "content": "new"}));
    // After the target was shown, the directory is swapped for another one.
    fs::rename(tmp.join("d"), tmp.join("d.old")).unwrap();
    fs::create_dir(tmp.join("d")).unwrap();
    let out = run(&c, &t, 16_384);
    assert_eq!((out.text.as_str(), out.end), ("target changed since approval; not run", End::File { failed: true }));
    assert_eq!(fs::read(tmp.join("d.old/f.txt")).unwrap(), b"old");
    assert_eq!(fs::read_dir(tmp.join("d")).unwrap().count(), 0);
    // Swapped for a symlink to elsewhere: the no-follow open refuses it.
    let (c, t) = tmp.prepare("write", json!({"path": "d.old/f.txt", "content": "new"}));
    fs::rename(tmp.join("d.old"), tmp.join("d.moved")).unwrap();
    symlink(tmp.join("d.moved"), tmp.join("d.old")).unwrap();
    assert!(run(&c, &t, 16_384).end == End::File { failed: true });
    assert_eq!(fs::read(tmp.join("d.moved/f.txt")).unwrap(), b"old");
}

#[test]
fn write_long_name() {
    let tmp = Tmp::new();
    // The longest name a file system takes; the temp beside it must still fit.
    let name = "n".repeat(255);
    assert_eq!(tmp.go("write", json!({"path": name, "content": "x"})), (format!("wrote 1 bytes to {name} (created)"), false));
    assert!(!tmp.go("write", json!({"path": name, "content": "y"})).1);
    assert_eq!(fs::read(tmp.join(&name)).unwrap(), b"y");
    // Cut on a character boundary (a cut through `é` is no valid name on macOS).
    let wide = format!("a{}", "é".repeat(127));
    assert!(!tmp.go("write", json!({"path": wide, "content": "z"})).1);
    assert!(tmp.temps(".").is_empty(), "{:?}", tmp.temps("."));
}

#[test]
fn write_creates_parents() {
    let tmp = Tmp::new();
    assert_eq!(tmp.go("write", json!({"path": "a/b/c.txt", "content": "deep"})), ("wrote 4 bytes to a/b/c.txt (created)".into(), false));
    assert_eq!(fs::read(tmp.join("a/b/c.txt")).unwrap(), b"deep");
    assert!(fs::metadata(tmp.join("a/b")).unwrap().is_dir());
    // A parent created meanwhile is fine.
    let (c, t) = tmp.prepare("write", json!({"path": "x/y/z.txt", "content": "z"}));
    fs::create_dir(tmp.join("x")).unwrap();
    assert_eq!(run(&c, &t, 16_384).text, "wrote 1 bytes to x/y/z.txt (created)");
    // Below a regular file nothing is created.
    tmp.file("plain", "x");
    let cx = Ctx { cwd: &tmp.0, root: &tmp.0, home: &tmp.0, protected: &[] };
    assert_eq!(target("plain/new.txt", true, &cx).err().unwrap(), "plain/new.txt: Not a directory");
}

#[test]
fn write_temp_removed_on_error() {
    let tmp = Tmp::new();
    let (c, t) = tmp.prepare("write", json!({"path": "x", "content": "data"}));
    // The name became a non-empty directory: the rename fails after the temp was written.
    tmp.file("x/keep", "k");
    let out = run(&c, &t, 16_384);
    assert_eq!(out.end, End::File { failed: true });
    assert!(out.text.starts_with("x: "), "{}", out.text);
    assert!(tmp.temps(".").is_empty(), "{:?}", tmp.temps("."));
    assert_eq!(fs::read(tmp.join("x/keep")).unwrap(), b"k");
}

#[test]
fn edit_exactly_once() {
    let tmp = Tmp::new();
    let p = tmp.file("f.toml", "a = 1\nb = 2\nc = 3\n");
    assert_eq!(tmp.go("edit", json!({"path": "f.toml", "old_text": "b = 2", "new_text": "b = 20"})), ("edited f.toml: lines 2-2 (1) -> 1 lines".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"a = 1\nb = 20\nc = 3\n");
    assert!(tmp.temps(".").is_empty());
}

#[test]
fn edit_not_found() {
    let tmp = Tmp::new();
    let p = tmp.file("f.txt", "alpha\n");
    assert_eq!(tmp.go("edit", json!({"path": "f.txt", "old_text": "beta", "new_text": "x"})), ("old_text not found in f.txt".into(), true));
    assert_eq!(fs::read(&p).unwrap(), b"alpha\n");
}

#[test]
fn edit_occurs_n() {
    let tmp = Tmp::new();
    tmp.file("f.txt", "x = 1\nx = 1\nx = 1\n");
    assert_eq!(tmp.go("edit", json!({"path": "f.txt", "old_text": "x = 1", "new_text": "y"})), ("old_text occurs 3 times in f.txt; include more context".into(), true));
}

#[test]
fn edit_empty_old() {
    let tmp = Tmp::new();
    tmp.file("f.txt", "x");
    assert_eq!(tmp.go("edit", json!({"path": "f.txt", "old_text": "", "new_text": "y"})), ("old_text is empty".into(), true));
}

#[test]
fn edit_overlap_counts() {
    let tmp = Tmp::new();
    let p = tmp.file("f.txt", "aaa");
    assert_eq!(tmp.go("edit", json!({"path": "f.txt", "old_text": "aa", "new_text": "b"})), ("old_text occurs 2 times in f.txt; include more context".into(), true));
    assert_eq!(fs::read(&p).unwrap(), b"aaa");
}

#[test]
fn edit_crlf_both() {
    let tmp = Tmp::new();
    let p = tmp.file("win.txt", "one\r\ntwo\r\nthree\r\n");
    assert_eq!(tmp.go("edit", json!({"path": "win.txt", "old_text": "one\ntwo", "new_text": "1\n2\n2b"})), ("edited win.txt: lines 1-2 (2) -> 3 lines".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"1\r\n2\r\n2b\r\nthree\r\n");
    // No retry without a newline in old_text.
    assert!(tmp.go("edit", json!({"path": "win.txt", "old_text": "three\r", "new_text": "3"})).0.starts_with("edited"));
    assert!(tmp.go("edit", json!({"path": "win.txt", "old_text": "nothing", "new_text": "3"})).1);
}

#[test]
fn edit_crlf_keeps_crlf_new_text() {
    let tmp = Tmp::new();
    // new_text already in CRLF: no `\r\r\n`.
    let p = tmp.file("w.txt", "one\r\ntwo\r\n");
    assert_eq!(tmp.go("edit", json!({"path": "w.txt", "old_text": "one\ntwo", "new_text": "1\r\n2"})), ("edited w.txt: lines 1-2 (2) -> 2 lines".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"1\r\n2\r\n");
    // old_text with mixed endings matches a CRLF file.
    let p = tmp.file("m.txt", "a\r\nb\r\nc\r\n");
    assert_eq!(tmp.go("edit", json!({"path": "m.txt", "old_text": "a\r\nb\nc", "new_text": "x\ny"})), ("edited m.txt: lines 1-3 (3) -> 2 lines".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"x\r\ny\r\n");
    // An LF file is never converted.
    let p = tmp.file("u.txt", "a\nb\n");
    assert_eq!(tmp.go("edit", json!({"path": "u.txt", "old_text": "a\r\nb", "new_text": "c"})).0, "old_text not found in u.txt");
    assert_eq!(fs::read(&p).unwrap(), b"a\nb\n");
}

#[test]
fn edit_large_file_bounded() {
    let _alone = memory();
    let tmp = Tmp::new();
    // One byte over the limit: refused before a byte is read, however big the file.
    let p = tmp.file("big.log", "needle\n");
    for len in [EDIT_MAX + 1, 1 << 30] {
        fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(len).unwrap();
        let t0 = Instant::now();
        let (text, failed) = tmp.go("edit", json!({"path": "big.log", "old_text": "needle", "new_text": "pin"}));
        assert_eq!((text, failed), (format!("big.log is {} bytes; edit takes files up to 8 MiB; use the shell", num(len)), true));
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    }
    // At the limit it works, and bytes outside the span stay.
    fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(EDIT_MAX).unwrap();
    assert_eq!(tmp.go("edit", json!({"path": "big.log", "old_text": "needle", "new_text": "pin"})), ("edited big.log: lines 1-1 (1) -> 1 lines".into(), false));
    let f = fs::File::open(&p).unwrap();
    assert_eq!(f.metadata().unwrap().len(), EDIT_MAX - 3);
    let mut head = [0; 8];
    f.take(8).read_exact(&mut head).unwrap();
    assert_eq!(&head, b"pin\n\0\0\0\0");
}

#[test]
fn edit_swapped_symlink_refused() {
    let tmp = Tmp::new();
    tmp.file("f.txt", "public = 1\n");
    let secret = tmp.file("secret", "token = abc\npublic = 1\n");
    let (c, t) = tmp.prepare("edit", json!({"path": "f.txt", "old_text": "public = 1", "new_text": "public = 2"}));
    // After the target was shown, the file is swapped for a symlink to another file.
    fs::remove_file(tmp.join("f.txt")).unwrap();
    symlink(&secret, tmp.join("f.txt")).unwrap();
    let out = run(&c, &t, 16_384);
    assert_eq!(out.end, End::File { failed: true });
    assert!(out.text.starts_with("f.txt: "), "{}", out.text);
    assert!(fs::symlink_metadata(tmp.join("f.txt")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&secret).unwrap(), b"token = abc\npublic = 1\n");
    assert!(tmp.temps(".").is_empty());
}

#[test]
fn edit_keeps_invalid_utf8() {
    let tmp = Tmp::new();
    let p = tmp.file("latin1.ini", b"caf\xe9 = 1\nother = 2");
    assert_eq!(tmp.go("edit", json!({"path": "latin1.ini", "old_text": "other = 2", "new_text": "other = 3"})), ("edited latin1.ini: lines 2-2 (1) -> 1 lines".into(), false));
    assert_eq!(fs::read(&p).unwrap(), b"caf\xe9 = 1\nother = 3");
}

#[test]
fn edit_result_lines() {
    let tmp = Tmp::new();
    tmp.file("f.txt", numbered(50));
    let r = tmp.go("edit", json!({"path": "f.txt", "old_text": "line 42\nline 43\nline 44", "new_text": "a\nb\nc\nd\ne"}));
    assert_eq!(r, ("edited f.txt: lines 42-44 (3) -> 5 lines".into(), false));
    // A deleted span: no lines of new text.
    assert_eq!(tmp.go("edit", json!({"path": "f.txt", "old_text": "line 1\n", "new_text": ""})).0, "edited f.txt: lines 1-1 (1) -> 0 lines");
}

#[test]
fn read_number_forms() {
    // Counts and sizes have thousands separators; line numbers and offsets are positions the model sends back as
    // `offset`, so they stay plain digits.
    let tmp = Tmp::new();
    tmp.file("n.txt", numbered(3_000));
    let (text, _) = tmp.go("read", json!({"path": "n.txt", "offset": 1001, "limit": 1500}));
    assert!(text.ends_with("line 2500\n[lines 1001-2500 shown; file is 28,893 bytes; continue with offset=2501]\n"), "{}", &text[text.len() - 100..]);
    assert_eq!(tmp.go("read", json!({"path": "n.txt", "offset": 5000})), ("offset 5000 is past the end (file has 3,000 lines)".into(), true));
    let old: String = (1234..=2533).map(|i| format!("line {i}\n")).collect();
    let new: String = (1..=2000).map(|i| format!("new {i}\n")).collect();
    assert_eq!(tmp.go("edit", json!({"path": "n.txt", "old_text": old, "new_text": new})), ("edited n.txt: lines 1234-2533 (1,300) -> 2,000 lines".into(), false));
    for i in 1..=1001 {
        tmp.file(&format!("d/{i:04}"), "");
    }
    assert!(tmp.go("read", json!({"path": "d", "limit": 1000})).0.ends_with("1000\n[entries 1-1000 shown of 1,001; continue with offset=1001]\n"));
    assert!(tmp.go("read", json!({"path": "d", "offset": 1001})).0.ends_with("1001\n[directory, 1,001 entries]\n"));
    assert_eq!(tmp.go("read", json!({"path": "d", "offset": 1002})).0, "offset 1002 is past the end (directory has 1,001 entries)");
}

#[test]
fn string_write() {
    let tmp = Tmp::new();
    let content = "n".repeat(1_204);
    assert_eq!(tmp.go("write", json!({"path": "notes.md", "content": content})).0, "wrote 1,204 bytes to notes.md (created)");
    assert_eq!(tmp.go("write", json!({"path": "notes.md", "content": content})).0, "wrote 1,204 bytes to notes.md (replaced)");
}

#[test]
fn string_edit() {
    let tmp = Tmp::new();
    tmp.file("src/lib.rs", "fn a() {}\nfn b() {}\n");
    assert_eq!(tmp.go("edit", json!({"path": "src/lib.rs", "old_text": "fn b() {}\n", "new_text": "fn b() {\n    a();\n}\n"})).0, "edited src/lib.rs: lines 2-2 (1) -> 3 lines");
    assert_eq!(tmp.go("edit", json!({"path": "src/missing.rs", "old_text": "x", "new_text": "y"})), ("src/missing.rs: No such file or directory".into(), true));
}

#[test]
fn start_sends_one_event() {
    let tmp = Tmp::new();
    tmp.file("f.txt", "hi\n");
    let (c, t) = tmp.prepare("read", json!({"path": "f.txt"}));
    let (tx, rx) = channel();
    start(c, t, 16_384, 42, &tx);
    let Ev::Tool { tag, out } = rx.recv_timeout(Duration::from_secs(5)).unwrap() else { panic!() };
    assert_eq!((tag, out.text.as_str(), out.end), (42, "hi\n", End::File { failed: false }));
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

/// xorshift64: the same inputs on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Reads `name` page by page, following each paging hint, and checks every page: not empty, within its bounds, and
/// moving on. Returns the pages' text without the hints and trailers, or the single message a read gave.
fn follow(tmp: &Tmp, name: &str, limit: u64, cap: usize) -> String {
    let (mut all, mut offset) = (String::new(), 1);
    loop {
        let (text, failed) = tmp.go_cap("read", json!({"path": name, "offset": offset, "limit": limit}), cap);
        if failed || text == "(empty file)" {
            assert_eq!(offset, 1, "{text}");
            return text;
        }
        // U+FFFD takes three bytes for one; the cut marker, the hint and the trailer are short.
        assert!(text.len() <= 3 * READ_MAX.min(cap) + 200, "{} bytes from cap {cap}", text.len());
        let body: String = text.split_inclusive('\n').take_while(|l| !l.starts_with("[lines ") && !l.starts_with("[invalid UTF-8")).collect();
        assert!(!body.is_empty(), "an empty page at offset {offset}");
        all.push_str(&body);
        let Some(hint) = text.split("; continue with offset=").nth(1) else { return all };
        let next: u64 = hint.split(']').next().unwrap().parse().unwrap();
        assert!(next > offset && next <= offset + limit, "offset {offset} -> {next}");
        offset = next;
    }
}

#[test]
fn read_pages_roundtrip() {
    let tmp = Tmp::new();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for i in 0..300 {
        // Lines shorter than the page, so nothing is cut and the pages give the file back.
        let lines: Vec<String> = (0..rng.below(400)).map(|_| (0..rng.below(300)).map(|_| char::from(b'a' + rng.below(26) as u8)).collect()).collect();
        let mut text = lines.join("\n");
        if !lines.is_empty() && rng.below(2) == 0 {
            text.push('\n');
        }
        let name = format!("f{i}.txt");
        tmp.file(&name, &text);
        let (limit, cap) = (1 + rng.below(60), 302 + rng.below(3_000) as usize);
        let got = follow(&tmp, &name, limit, cap);
        // Every line comes back with its newline, the last one too.
        let want = if text.is_empty() {
            "(empty file)".into()
        } else if text.ends_with('\n') {
            text
        } else {
            text + "\n"
        };
        assert_eq!(got, want, "limit {limit} cap {cap}");
    }
}

#[test]
fn read_pager_never_panics() {
    let tmp = Tmp::new();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let bytes = [b'a', b'b', b' ', b'\n', b'\n', b'\r', 0xC3, 0xA9, 0xFF, 0xE2, 0x82, 0xAC, 0xF0];
    for i in 0..300 {
        let mut data: Vec<u8> = (0..rng.below(6_000)).map(|_| bytes[rng.below(bytes.len() as u64) as usize]).collect();
        if rng.below(10) == 0 {
            data.push(0);
        }
        let name = format!("r{i}.bin");
        tmp.file(&name, &data);
        let (limit, cap) = (1 + rng.below(40), rng.below(2_000) as usize);
        follow(&tmp, &name, limit, cap);
        // Any offset gives a page, the past-the-end message or the binary refusal; never a panic.
        let offset = 1 + rng.below(200);
        let (text, _) = tmp.go_cap("read", json!({"path": name, "offset": offset, "limit": limit}), cap);
        assert!(!text.is_empty());
    }
}
