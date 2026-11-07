#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt, fs::symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const NO_TAB: &str = "no TAB between ROOT and COMMAND";
const BAD_ROOT: &str = "ROOT is not * or an absolute path";
const BAD_CMD: &str = "COMMAND is not one printable-ASCII line of 1 to 200 bytes";

/// A fresh directory under the temp dir (its realpath), removed when dropped.
struct T(PathBuf);

impl T {
    fn new() -> T {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-allow-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        T(fs::canonicalize(p).unwrap())
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    /// The allow file with `bytes` and `mode`.
    fn allow(&self, bytes: impl AsRef<[u8]>, mode: u32) -> PathBuf {
        let p = self.p("allow");
        fs::write(&p, bytes).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn r(n: usize, root: &str, cmd: &str) -> Rule {
    Rule { n, root: root.into(), cmd: cmd.into() }
}

fn rules(rules: Vec<Rule>) -> Allow {
    Allow { rules, ..Allow::default() }
}

fn read(p: &Path) -> String {
    String::from_utf8(fs::read(p).unwrap()).unwrap()
}

/// `op` again while it finds the file locked, for at most 10 s. A child that another test thread forks holds a copy of
/// every open fd until it execs, so a lock this test already released can outlive its close for a moment. Only for a
/// lock taken again on a file this test locked before; a lock held on purpose is asserted directly.
fn unlocked<R, E: std::fmt::Display>(mut op: impl FnMut() -> Result<R, E>) -> Result<R, E> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        match op() {
            Err(e) if e.to_string().ends_with("locked by another henki") && Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
            r => return r,
        }
    }
}

#[test]
fn allow_parse_rules() {
    let t = T::new();
    let f = t.allow("# rules\n\n/src/app\tgit status\r\n*\tmake test\n/src/app\tls -la  x", 0o600);
    let a = load(&f, true).unwrap();
    assert_eq!(a.file, f);
    assert_eq!(a.rules, [r(1, "/src/app", "git status"), r(2, "*", "make test"), r(3, "/src/app", "ls -la  x")]);
}

#[test]
fn allow_invalid_lines() {
    let x200 = format!("/a\t{}", "x".repeat(200));
    assert_eq!(rule(7, x200.as_bytes()), Ok(r(7, "/a", &"x".repeat(200))));
    let cases: [(&[u8], &str); 14] = [
        (b"/a git status", NO_TAB),
        (b"", NO_TAB),
        (b"a\tls", BAD_ROOT),
        (b"\tls", BAD_ROOT),
        (b"~/a\tls", BAD_ROOT),
        ("/a\u{200b}b\tls".as_bytes(), BAD_ROOT),
        (b"/a\x1bb\tls", BAD_ROOT),
        (b"/a\xffb\tls", BAD_ROOT),
        (b"/a\t", BAD_CMD),
        (b"/a\t ls", BAD_CMD),
        (b"/a\t#ls", BAD_CMD),
        ("/a\tls \u{e4}".as_bytes(), BAD_CMD),
        (b"/a\tls\tx", BAD_CMD),
        (b"/a\tls\x7f", BAD_CMD),
    ];
    for (line, why) in cases {
        assert_eq!(rule(1, line), Err(why), "{:?}", String::from_utf8_lossy(line));
    }
    let x201 = format!("/a\t{}", "x".repeat(201));
    assert_eq!(rule(1, x201.as_bytes()), Err(BAD_CMD));
}

#[test]
fn allow_numbers_count_invalid() {
    let t = T::new();
    let f = t.allow("# c\n/a\tok1\nbad line\n\n# c2\n/a\tok2\n", 0o600);
    let want = [f.display().to_string(), "1  /a  ok1".into(), format!("2  !bad line  (invalid: {NO_TAB})"), "3  /a  ok2".into()];
    assert_eq!(list(&f, Path::new("/home/u")), want);
    assert_eq!(remove(&f, 3, Path::new("/home/u")).unwrap(), "removed rule 3: /a  ok2");
    assert_eq!(read(&f), "# c\n/a\tok1\nbad line\n\n# c2\n");
}

#[test]
fn allow_exact_only() {
    let a = rules(vec![r(1, "/r", "git status")]);
    let root = Path::new("/r");
    assert_eq!(a.find("git status", root), Some(&r(1, "/r", "git status")));
    for cmd in ["git status; rm -rf ~", "git status && x", "git status ", "git status --short", " git status", "Git status", "git  status", "git"] {
        assert_eq!(a.find(cmd, root), None, "{cmd:?}");
    }
}

#[test]
fn allow_scoped_root() {
    let a = rules(vec![r(1, "/src/app", "make"), r(2, "/src/other", "ls")]);
    assert_eq!(a.find("make", Path::new("/src/app")).map(|r| r.n), Some(1));
    for root in ["/src/app/sub", "/src", "/src/app2", "/", "/src/other"] {
        assert_eq!(a.find("make", Path::new(root)), None, "{root}");
    }
    assert!(applies(&r(1, "/src/app/", "make"), Path::new("/src/app")), "one path, however it is written");
    assert!(!applies(&r(1, "/src/app", "make"), Path::new("/src/app/sub")));
}

#[test]
fn allow_star_root() {
    let a = rules(vec![r(1, "/src/app", "ls"), r(2, "*", "make")]);
    for root in ["/", "/src/app", "/anywhere/else"] {
        assert_eq!(a.find("make", Path::new(root)).map(|r| r.n), Some(2), "{root}");
    }
    assert_eq!(a.find("ls", Path::new("/x")), None);
}

#[test]
fn allow_load_absent() {
    let t = T::new();
    let a = load(&t.p("allow"), true).unwrap();
    assert_eq!((a.file, a.rules, a.hash), (t.p("allow"), vec![], None));
}

#[test]
fn allow_load_not_read_in_auto() {
    let t = T::new();
    let f = t.allow("not a rule\n", 0o666);
    let a = load(&f, false).unwrap();
    assert_eq!((&a.file, a.rules.len(), &a.hash), (&f, 0, &None));
    fs::remove_file(&f).unwrap();
    symlink(t.p("elsewhere"), &f).unwrap();
    assert!(load(&f, false).is_ok());
}

#[test]
fn allow_load_fail_closed() {
    let t = T::new();
    let f = t.p("allow");
    let s = f.display();
    let fix = "fix: keep the rules in a regular file of at most 64 KiB";
    let target = t.p("target");
    fs::write(&target, "/a\tls\n").unwrap();
    let link = format!("{s}: a symlink; fix: put the file itself in its place");
    symlink(&target, &f).unwrap();
    assert_eq!(load(&f, true).unwrap_err(), link);
    fs::remove_file(&f).unwrap();
    symlink(t.p("gone"), &f).unwrap();
    assert_eq!(load(&f, true).unwrap_err(), link, "a dangling symlink is not an absent file");
    fs::remove_file(&f).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(t.p("fifo")).status().unwrap().success());
    symlink(t.p("fifo"), &f).unwrap();
    assert_eq!(load(&f, true).unwrap_err(), link, "refused at the open, before anything is read");
    fs::remove_file(&f).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&f).status().unwrap().success());
    assert_eq!(load(&f, true).unwrap_err(), format!("{s}: not a regular file; {fix}"));
    fs::remove_file(&f).unwrap();
    fs::create_dir(&f).unwrap();
    assert_eq!(load(&f, true).unwrap_err(), format!("{s}: not a regular file; {fix}"));
    fs::remove_dir(&f).unwrap();
    for mode in [0o666, 0o620, 0o602] {
        t.allow("/a\tls\n", mode);
        assert_eq!(load(&f, true).unwrap_err(), format!("{s}: another user can change it; fix: chown $USER {s}; chmod 600 {s}"), "{mode:o}");
    }
    t.allow("#".repeat(65_536), 0o644);
    assert!(load(&f, true).is_ok(), "64 KiB is the limit");
    t.allow("#".repeat(65_537), 0o600);
    assert_eq!(load(&f, true).unwrap_err(), format!("{s}: over 65,536 bytes; {fix}"));
    t.allow("# c\n/a\tok\n\nbad\n/a\tok2\n", 0o600);
    assert_eq!(load(&f, true).unwrap_err(), format!("{s}:4: {NO_TAB}; fix: henki allow rm 2, or edit the file"));
}

#[test]
fn allow_load_foreign_owner() {
    assert!(!others(0o600, 1000, 1000));
    assert!(!others(0o644, 0, 1000), "root may own it");
    assert!(others(0o600, 1001, 1000), "another user owns it");
    assert!(others(0o620, 1000, 1000));
    assert!(others(0o602, 1000, 1000));
}

#[test]
fn allow_append_creates_with_header() {
    let t = T::new();
    let f = t.p("allow");
    let mut a = load(&f, true).unwrap();
    assert_eq!(a.append(&t.0, "git status").unwrap(), 1);
    let want = format!(
        "# henki allow rules: ROOT<TAB>COMMAND, one per line; COMMAND then runs without asking in ROOT.\n\
         # List and remove them with /allow in henki or henki allow [rm N]; or edit this file.\n{}\tgit status\n",
        t.0.display()
    );
    assert_eq!(read(&f), want);
    assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o600);
    let root = t.0.to_str().unwrap();
    assert_eq!(a.rules, [r(1, root, "git status")]);
    assert_eq!(unlocked(|| a.append(&t.0, "make")).unwrap(), 2, "the second one appends");
    assert_eq!(load(&f, true).unwrap().rules, [r(1, root, "git status"), r(2, root, "make")]);
}

#[test]
fn allow_append_appends() {
    let t = T::new();
    let f = t.allow("# mine\n/x\tls\nbad", 0o600);
    let mut a = Allow { file: f.clone(), hash: Some(sys::fnv(b"# mine\n/x\tls\nbad")), ..Allow::default() };
    assert_eq!(a.append(Path::new("/src/app"), "make test").unwrap(), 3, "a last line without a newline stays its own");
    assert_eq!(read(&f), "# mine\n/x\tls\nbad\n/src/app\tmake test\n");
    assert_eq!(a.rules, [r(3, "/src/app", "make test")]);
    assert_eq!(a.find("make test", Path::new("/src/app")).map(|r| r.n), Some(3));
    fs::write(&f, format!("{}*\tpwd\n", read(&f))).unwrap();
    assert_eq!(unlocked(|| a.append(Path::new("/src/app"), "ls")).unwrap(), 5, "numbered from the file as it is");
    let e = a.append(Path::new("/src/app"), "a\nb").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "never a line that is not a rule");
}

#[test]
fn allow_append_locked() {
    let t = T::new();
    let f = t.allow("/x\tls\n", 0o600);
    let mut a = load(&f, true).unwrap();
    let hold = File::open(&f).unwrap();
    hold.lock().unwrap();
    assert_eq!(a.append(Path::new("/x"), "make").unwrap_err().to_string(), "locked by another henki");
    assert_eq!(read(&f), "/x\tls\n");
    assert_eq!(remove(&f, 1, &t.0).unwrap_err(), format!("{}: locked by another henki", f.display()));
    drop(hold);
    assert_eq!(unlocked(|| a.append(Path::new("/x"), "make")).unwrap(), 2);
}

#[test]
fn allow_append_size_check() {
    let t = T::new();
    let f = t.allow("0123456789", 0o600);
    let m = fs::metadata(&f).unwrap();
    assert!(intact(4, 6, &m, &m));
    assert!(!intact(4, 5, &m, &m), "someone else wrote too");
    assert!(!intact(0, 6, &m, &m));
}

#[test]
fn allow_append_inode_check() {
    let t = T::new();
    let f = t.allow("0123456789", 0o600);
    let fd = File::open(&f).unwrap();
    let other = t.p("other");
    fs::write(&other, "0123456789").unwrap();
    fs::rename(&other, &f).unwrap();
    let (m, p) = (fd.metadata().unwrap(), fs::symlink_metadata(&f).unwrap());
    assert_eq!(m.len(), p.len());
    assert!(!intact(0, 10, &m, &p), "the path names another file now");
    assert!(intact(0, 10, &p, &p));
}

#[test]
fn allow_append_over_limit() {
    let t = T::new();
    // 65,524 bytes, and the rule line `/src/app<TAB>ls\n` is 12: exactly the limit.
    let f = t.allow(format!("#{}\n", "x".repeat(65_522)), 0o600);
    let mut a = load(&f, true).unwrap();
    assert_eq!(a.append(Path::new("/src/app"), "ls").unwrap(), 1, "up to the limit is fine");
    assert_eq!(fs::metadata(&f).unwrap().len(), 65_536);
    let before = fs::read(&f).unwrap();
    assert_eq!(unlocked(|| a.append(Path::new("/src/app"), "x")).unwrap_err().to_string(), "would be over 65,536 bytes");
    assert_eq!(fs::read(&f).unwrap(), before, "nothing written");
    assert_eq!(a.rules, [r(1, "/src/app", "ls")]);
    assert_eq!(load(&f, true).unwrap().rules, [r(1, "/src/app", "ls")], "the file still loads");

    let f = t.allow(format!("#{}\n", "x".repeat(65_498)), 0o600);
    let mut a = load(&f, true).unwrap();
    assert_eq!(unlocked(|| a.append(Path::new("/src/app"), &"x".repeat(100))).unwrap_err().to_string(), "would be over 65,536 bytes");
    assert_eq!(fs::metadata(&f).unwrap().len(), 65_500);
    assert_eq!(list(&f, &t.0), [f.display().to_string()], "list and remove still read it");
}

#[test]
fn allow_append_symlink_refused() {
    let t = T::new();
    let (f, target) = (t.p("allow"), t.p("target"));
    fs::write(&target, "/x\tls\n").unwrap();
    let mut a = load(&f, true).unwrap();
    symlink(&target, &f).unwrap();
    assert_eq!(a.append(Path::new("/x"), "make").unwrap_err().raw_os_error(), Some(libc::ELOOP), "linked after load");
    assert_eq!(read(&target), "/x\tls\n");
    fs::remove_file(&f).unwrap();
    symlink(t.p("gone"), &f).unwrap();
    assert_eq!(a.append(Path::new("/x"), "make").unwrap_err().raw_os_error(), Some(libc::ELOOP), "dangling");
    assert!(!t.p("gone").exists() && a.rules.is_empty());
}

#[test]
fn allow_append_missing_config_dir() {
    let t = T::new();
    let mut a = load(&t.p("missing/allow"), true).unwrap();
    assert_eq!(a.append(Path::new("/x"), "make").unwrap_err().kind(), io::ErrorKind::NotFound, "always needs the config directory");
    assert!(!t.p("missing").exists() && a.rules.is_empty());
}

#[test]
fn allow_find_first_match() {
    let t = T::new();
    let f = t.allow("/src/app\tmake\n*\tmake\n*\tls\n/src/app\tls\n", 0o600);
    let a = load(&f, true).unwrap();
    assert_eq!(a.find("make", Path::new("/src/app")).map(|r| r.n), Some(1));
    assert_eq!(a.find("make", Path::new("/elsewhere")).map(|r| r.n), Some(2));
    assert_eq!(a.find("ls", Path::new("/src/app")).map(|r| r.n), Some(3), "the first in file order, a * rule too");
}

#[test]
fn allow_load_non_utf8_comment() {
    let t = T::new();
    let f = t.allow(b"# caf\xe9 \x1b[31m\n/a\tls\n", 0o600);
    assert_eq!(load(&f, true).unwrap().rules, [r(1, "/a", "ls")], "a comment is never parsed or shown");
    assert_eq!(list(&f, &t.0), [f.display().to_string(), "1  /a  ls".into()]);
}

#[test]
fn allow_load_whitespace_only_line() {
    let t = T::new();
    let f = t.allow("/a\tls\n\r\n   \n", 0o600);
    assert_eq!(load(&f, true).unwrap_err(), format!("{}:3: {NO_TAB}; fix: henki allow rm 2, or edit the file", f.display()));
    t.allow("/a\tls\n\t\n", 0o600);
    assert_eq!(load(&f, true).unwrap_err(), format!("{}:2: {BAD_ROOT}; fix: henki allow rm 2, or edit the file", f.display()));
}

#[test]
fn allow_fits_shapes() {
    let root = Path::new("/src/app");
    assert!(fits("git status", root));
    assert!(fits(&"x".repeat(200), root));
    assert!(fits(&format!("a{}b", " ".repeat(32)), root));
    for cmd in ["a\nb", &"x".repeat(201), &format!("a{}b", " ".repeat(33)), "ls \u{e4}", "#x", " ls", "", "ls\tx", "ls\x1b"] {
        assert!(!fits(cmd, root), "{cmd:?}");
    }
    for root in ["rel", "*", "/a\nb", "/a\tb", "/a\u{200b}", "/a\x1b"] {
        assert!(!fits("ls", Path::new(root)), "{root:?}");
    }
    assert!(!fits("ls", Path::new(std::ffi::OsStr::from_bytes(b"/a\xff"))));
}

#[test]
fn allow_list_format() {
    let t = T::new();
    let home = t.p("h");
    let f = t.allow(format!("# c\n{0}/src/app\tgit status\n*\tmake\n/opt/x\tls\n{0}\tpwd\n", home.display()), 0o600);
    let want = [f.display().to_string(), "1  ~/src/app  git status".into(), "2  *  make".into(), "3  /opt/x  ls".into(), "4  ~  pwd".into()];
    assert_eq!(list(&f, &home), want);
    assert_eq!(list(&t.p("none"), &home), [t.p("none").display().to_string()]);
    assert_eq!(shown(Path::new("/elsewhere/h"), &home), "/elsewhere/h");
}

#[test]
fn allow_list_invalid_escaped() {
    let t = T::new();
    let f = t.allow(b"/a\x1b[31m\tls\n/b\xff\tls\n", 0o600);
    let want = [f.display().to_string(), format!("1  !/a\\x1b[31m\tls  (invalid: {BAD_ROOT})"), format!("2  !/b\u{fffd}\tls  (invalid: {BAD_ROOT})")];
    assert_eq!(list(&f, &t.0), want);
    fs::remove_file(&f).unwrap();
    fs::create_dir(&f).unwrap();
    assert_eq!(list(&f, &t.0), [f.display().to_string(), "cannot read it: not a regular file".into()]);
}

#[test]
fn allow_list_any_mode() {
    let t = T::new();
    let f = t.allow("/a\tls\n", 0o666);
    assert!(load(&f, true).is_err());
    assert_eq!(list(&f, &t.0), [f.display().to_string(), "1  /a  ls".into()]);
    assert_eq!(remove(&f, 1, &t.0).unwrap(), "removed rule 1: /a  ls");
}

#[test]
fn allow_remove_keeps_rest() {
    let t = T::new();
    let f = t.allow("# head\r\n/a\tone\r\n\nbad line\n*\ttwo\n# tail", 0o600);
    assert_eq!(remove(&f, 2, &t.0).unwrap(), format!("removed rule 2: !bad line  (invalid: {NO_TAB})"));
    assert_eq!(read(&f), "# head\r\n/a\tone\r\n\n*\ttwo\n# tail");
    assert_eq!(remove(&f, 1, &t.0).unwrap(), "removed rule 1: /a  one");
    assert_eq!(read(&f), "# head\r\n\n*\ttwo\n# tail");
    assert_eq!(fs::read_dir(&t.0).unwrap().count(), 1, "no temp file left");
}

#[test]
fn allow_remove_symlinked_config_dir() {
    let t = T::new();
    fs::create_dir(t.p("real")).unwrap();
    symlink(t.p("real"), t.p("henki")).unwrap();
    let f = t.p("henki/allow");
    let mut a = load(&f, true).unwrap();
    assert_eq!(a.append(Path::new("/src/app"), "ls").unwrap(), 1);
    assert_eq!(load(&f, true).unwrap().rules, [r(1, "/src/app", "ls")]);
    assert_eq!(list(&f, &t.0), [f.display().to_string(), "1  /src/app  ls".into()]);
    assert_eq!(unlocked(|| remove(&f, 1, &t.0)).unwrap(), "removed rule 1: /src/app  ls");
    assert!(load(&f, true).unwrap().rules.is_empty());
    assert!(fs::symlink_metadata(t.p("henki")).unwrap().file_type().is_symlink(), "the link stays");
    assert!(fs::symlink_metadata(t.p("real/allow")).unwrap().is_file());
    assert_eq!(fs::read_dir(t.p("real")).unwrap().count(), 1, "no temp file left");
}

#[test]
fn allow_remove_no_such_rule() {
    let t = T::new();
    let f = t.allow("# c\n/a\tone\n*\ttwo\n", 0o600);
    for n in [0, 3] {
        assert_eq!(unlocked(|| remove(&f, n, &t.0)).unwrap_err(), format!("no rule {n} in {}", f.display()));
    }
    assert_eq!(read(&f), "# c\n/a\tone\n*\ttwo\n");
    assert_eq!(remove(&t.p("none"), 1, &t.0).unwrap_err(), format!("no rule 1 in {}", t.p("none").display()));
}

#[test]
fn allow_remove_mode_0600() {
    let t = T::new();
    let f = t.allow("/a\tone\n/a\ttwo\n", 0o644);
    remove(&f, 1, &t.0).unwrap();
    assert_eq!(fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o600);
    assert_eq!(load(&f, true).unwrap().rules, [r(1, "/a", "two")]);
}

#[test]
fn allow_hash() {
    let t = T::new();
    let f = t.allow("# c\n/a\tls\n", 0o600);
    assert_eq!(load(&f, true).unwrap().hash, Some(sys::fnv(b"# c\n/a\tls\n")));
    assert_eq!(load(&f, false).unwrap().hash, None);
    assert_eq!(load(&t.p("none"), true).unwrap().hash, None);
}
