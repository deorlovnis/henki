#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{sync::mpsc, time::Duration};

const DOC_FIX: &str = "fix: write doc.md with a summary of the extension on line 1";
const LINE_RULE: &str = "line 1 must be a summary of 1 to 160 bytes with no invisible characters";

/// A fresh directory under the temp dir (its realpath), removed when dropped.
struct T(PathBuf);

impl T {
    fn new() -> T {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-ext-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        T(fs::canonicalize(p).unwrap())
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    /// `rel` with `bytes`, parents made.
    fn file(&self, rel: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    /// An executable script at `rel`.
    fn exe(&self, rel: &str) -> PathBuf {
        let p = self.file(rel, "#!/bin/sh\n");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// An extension directory `rel` whose doc.md holds `doc`.
    fn ext(&self, rel: &str, doc: &str) -> PathBuf {
        self.file(&format!("{rel}/doc.md"), doc);
        self.p(rel)
    }

    fn dir(&self, rel: &str) -> PathBuf {
        fs::create_dir_all(self.p(rel)).unwrap();
        self.p(rel)
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn find(paths: &[PathBuf], shadow: &[&str], path: &str) -> Result<Found, String> {
    discover(paths, false, &shadow.iter().map(|s| s.to_string()).collect::<Vec<_>>(), OsStr::new(path))
}

fn ok(paths: &[PathBuf]) -> Found {
    find(paths, &[], "").map_err(|e| panic!("{e}")).ok().unwrap()
}

fn err(paths: &[PathBuf]) -> String {
    find(paths, &[], "").err().expect("discovery should fail")
}

fn names(f: &Found) -> Vec<&str> {
    f.exts.iter().map(|x| x.name.as_str()).collect()
}

#[test]
fn ext_discover_sorted() {
    let t = T::new();
    for n in ["b", "a", "c1", "a0"] {
        t.ext(&format!("one/{n}"), "# x");
    }
    t.ext("two/0", "# y");
    let f = ok(&[t.p("one"), t.p("two")]);
    assert_eq!(names(&f), ["a", "a0", "b", "c1", "0"]);
    let a = &f.exts[0];
    assert_eq!((a.line.as_str(), &a.doc), ("x", &t.p("one/a/doc.md")));
    assert_eq!(a.paths, [t.p("one/a"), t.p("one/a/doc.md")]);
    assert!(a.bin.is_none() && a.cmds.is_empty() && a.shadows.is_empty() && a.targets.is_empty());
}

#[test]
fn ext_dot_entries_skipped() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.file("e/.git/config", "x");
    t.file("e/.DS_Store", "x");
    t.file("e/a/.hidden", "x");
    t.dir("e/a/.git");
    assert_eq!(names(&ok(&[t.p("e")])), ["a"]);
}

#[test]
fn ext_name_rules() {
    let t = T::new();
    let long = "a".repeat(32);
    for n in ["a", "0x", "a_b-c", "9", &long] {
        t.ext(&format!("good/{n}"), "# x");
    }
    assert_eq!(names(&ok(&[t.p("good")])), ["0x", "9", "a", "a_b-c", &long]);
    for n in ["A", "-a", "_a", "a.b", "a b", "\u{e4}", &"a".repeat(33)] {
        let d = t.ext(&format!("bad/{n}"), "# x");
        let want = format!("ext {}: a name is 1 to 32 of a-z, 0-9, _ and -, starting with a-z or 0-9; fix: rename it", d.display());
        assert_eq!(err(&[t.p("bad")]), want);
        fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn ext_entry_not_dir_fails() {
    let t = T::new();
    let f = t.file("e/notes", "x");
    let want = format!("ext {}: Not a directory (os error 20); fix: make it an extension directory, or move it out of the ext path", f.display());
    assert_eq!(err(&[t.p("e")]), want);
    fs::remove_file(&f).unwrap();
    symlink(t.file("elsewhere", "x"), &f).unwrap();
    assert_eq!(err(&[t.p("e")]), want);
}

#[test]
fn ext_doc_line_cleaned() {
    let t = T::new();
    t.ext("e/a", "## #  Title: does x  \r\nbody\n");
    t.ext("e/b", "plain line");
    t.ext("e/c", &format!("# {}\n", "x".repeat(160)));
    t.ext("e/d", "#\tkept tab\n");
    let f = ok(&[t.p("e")]);
    let lines: Vec<&str> = f.exts.iter().map(|x| x.line.as_str()).collect();
    assert_eq!(lines, ["Title: does x", "plain line", &"x".repeat(160), "kept tab"]);
}

#[test]
fn ext_doc_line_rules() {
    let t = T::new();
    for doc in ["#  \nbody", "", &format!("# {}", "x".repeat(161)), "a\u{200b}b", "red \x1b[31m", "a\rb"] {
        let d = t.ext("e/a", doc);
        assert_eq!(err(&[t.p("e")]), format!("ext {}/doc.md: {LINE_RULE}; {DOC_FIX}", d.display()), "{doc:?}");
    }
    let d = t.p("e/a");
    t.file("e/a/doc.md", b"\xff\xfe summary\n");
    assert_eq!(err(&[t.p("e")]), format!("ext {}/doc.md: {LINE_RULE}; {DOC_FIX}", d.display()));
    fs::remove_file(d.join("doc.md")).unwrap();
    let missing = format!("ext {0}: {0}/doc.md: No such file or directory (os error 2); {DOC_FIX}", d.display());
    assert_eq!(err(&[t.p("e")]), missing);
    symlink(t.p("gone"), d.join("doc.md")).unwrap();
    assert_eq!(err(&[t.p("e")]), missing, "a broken symlink");
    fs::remove_file(d.join("doc.md")).unwrap();
    t.dir("e/a/doc.md");
    assert_eq!(err(&[t.p("e")]), format!("ext {0}: not a regular file: {0}/doc.md (directory); {DOC_FIX}", d.display()));
}

#[test]
fn ext_bin_rules() {
    let t = T::new();
    t.ext("e/a", "# a");
    let x = t.exe("e/a/bin/Tool.1");
    let y = t.exe("e/a/bin/a-b_c");
    let f = ok(&[t.p("e")]);
    let a = &f.exts[0];
    assert_eq!((a.bin.as_deref(), &a.cmds), (Some(t.p("e/a/bin").as_path()), &vec!["Tool.1".to_string(), "a-b_c".to_string()]));
    assert_eq!(a.paths, [t.p("e/a"), t.p("e/a/doc.md"), t.p("e/a/bin"), x, y]);

    let run =
        |p: &Path| format!("ext {}: a command is a file you can run, named with A-Z, a-z, 0-9, ., _ and -; fix: rename it or chmod u+x it, or move it out of bin/", p.display());
    let plain = t.file("e/a/bin/plain", "x");
    assert_eq!(err(&[t.p("e")]), run(&plain), "no exec bit");
    fs::remove_file(&plain).unwrap();
    let sub = t.dir("e/a/bin/sub");
    assert_eq!(err(&[t.p("e")]), run(&sub), "a subdirectory");
    fs::remove_dir(&sub).unwrap();
    let odd = t.exe("e/a/bin/x:y");
    assert_eq!(err(&[t.p("e")]), run(&odd), "a bad name");
    fs::remove_dir_all(t.p("e/a/bin")).unwrap();
    let bin = t.file("e/a/bin", "x");
    assert_eq!(err(&[t.p("e")]), format!("ext {}: Not a directory (os error 20); fix: make bin a directory of commands", bin.display()));
}

#[test]
fn ext_ignored_entries() {
    let t = T::new();
    t.ext("e/a", "# a");
    for n in ["README", "README.md", "LICENSE-MIT", "CHANGELOG.md", ".gitignore"] {
        t.file(&format!("e/a/{n}"), "x");
    }
    t.dir("e/a/.git");
    assert_eq!(names(&ok(&[t.p("e")])), ["a"]);
    let low = t.file("e/a/readme.md", "x");
    assert!(err(&[t.p("e")]).contains(&format!("unknown entry {}", low.display())), "case-sensitive");
}

#[test]
fn ext_unknown_entry_fails() {
    let t = T::new();
    let d = t.ext("e/a", "# a");
    for n in ["notes.txt", "lib", "tool", "gate", "tools.json", "transport"] {
        let p = if n.contains('.') { t.file(&format!("e/a/{n}"), "x") } else { t.dir(&format!("e/a/{n}")) };
        let want = format!("ext {}: unknown entry {}; v1 extensions hold doc.md and bin/ only; fix: move it out of the extension", d.display(), p.display());
        assert_eq!(err(&[t.p("e")]), want);
        let _ = fs::remove_file(&p).or_else(|_| fs::remove_dir(&p));
    }
}

#[test]
fn ext_same_name_twice() {
    let t = T::new();
    let first = t.ext("one/a", "# a");
    let second = t.ext("two/a", "# a");
    assert_eq!(err(&[t.p("one"), t.p("two")]), format!("ext {}: {} has the same name; fix: keep one of them", second.display(), first.display()));
}

#[test]
fn ext_same_cmd_two_exts() {
    let t = T::new();
    t.ext("one/a", "# a");
    t.exe("one/a/bin/x");
    t.ext("one/b", "# b");
    t.exe("one/b/bin/x");
    assert_eq!(err(&[t.p("one")]), "ext b: bin/x is also in ext a; fix: remove one of them");
    fs::remove_dir_all(t.p("one/b")).unwrap();
    t.ext("two/c", "# c");
    t.exe("two/c/bin/x");
    assert_eq!(err(&[t.p("one"), t.p("two")]), "ext c: bin/x is also in ext a; fix: remove one of them");
}

#[test]
fn ext_shadow_requires_entry() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.exe("e/a/bin/x");
    t.file("usr/x", "not executable");
    t.dir("usr2/x");
    let real = t.exe("usr3/x");
    t.exe("usr4/x");
    let path = format!("{}:{}:{}:{}", t.p("usr").display(), t.p("usr2").display(), t.p("usr3").display(), t.p("usr4").display());
    let e = find(&[t.p("e")], &[], &path).err().unwrap();
    assert_eq!(e, format!("ext a: bin/x shadows {}; fix: add shadow = [\"a/x\"] if that is intended", real.display()));
    assert!(find(&[t.p("e")], &["a/y", "b/x"], &path).is_err(), "another entry does not allow it");
}

#[test]
fn ext_shadow_allowed() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.exe("e/a/bin/x");
    t.exe("e/a/bin/y");
    let real = t.exe("usr/x");
    t.exe("usr2/x");
    let path = format!("{}:{}", t.p("usr").display(), t.p("usr2").display());
    let f = find(&[t.p("e")], &["a/x"], &path).map_err(|e| panic!("{e}")).ok().unwrap();
    assert_eq!(f.exts[0].shadows, [real]);
}

#[test]
fn ext_shadow_entry_invalid() {
    for s in ["halti", "Halti/x", "a/b/c", "a/", "/x", "a/x y", ""] {
        let e = find(&[], &[s], "").err().unwrap_or_else(|| panic!("{s:?} passed"));
        assert_eq!(e, format!("shadow entry {s:?} is not EXT/COMMAND; fix: name an extension and one of its bin/ commands, like \"halti/halti\""));
    }
}

#[test]
fn ext_shadow_entry_unused_ignored() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.exe("e/a/bin/x");
    let usr = t.dir("usr");
    let f = find(&[t.p("e")], &["a/nope", "zz/x", "a/x"], usr.to_str().unwrap()).map_err(|e| panic!("{e}")).ok().unwrap();
    assert!(f.exts[0].shadows.is_empty());
}

#[test]
fn ext_user_path_without_bins() {
    let t = T::new();
    t.ext("e/a", "# a");
    let bin = t.exe("e/a/bin/x").parent().unwrap().to_path_buf();
    symlink(&bin, t.p("alias")).unwrap();
    let (u, u2) = (t.dir("usr"), t.dir("usr2"));
    let path = format!("rel/bin:{}:{}:{}:{}::.", u.display(), bin.display(), t.p("alias").display(), u2.display());
    let f = find(&[t.p("e")], &[], &path).map_err(|e| panic!("{e}")).ok().unwrap();
    assert_eq!(f.user_path, format!("{}:{}", u.display(), u2.display()));
    assert_eq!(find(&[t.p("e")], &[], "").map_err(|e| panic!("{e}")).ok().unwrap().user_path, "");
}

#[test]
fn ext_prompt_bytes() {
    let t = T::new();
    t.ext("one/a", "# Alpha does a\nbody");
    t.ext("two/b", "Beta");
    let f = ok(&[t.p("one"), t.p("two")]);
    let want =
        format!("Extensions are commands on PATH; read the doc in brackets before first use.\n- a: Alpha does a [~/a/doc.md]\n- b: Beta [{}]", t.p("two/b/doc.md").display());
    assert_eq!(prompt(&f.exts, &t.p("one")), want);
}

#[test]
fn ext_prompt_empty() {
    assert_eq!(prompt(&[], Path::new("/home/u")), "");
}

#[test]
fn ext_child_path_name_order() {
    let t = T::new();
    t.ext("one/z", "# z");
    t.exe("one/z/bin/z1");
    t.ext("one/m", "# m");
    t.ext("two/b", "# b");
    t.exe("two/b/bin/b1");
    let f = ok(&[t.p("one"), t.p("two")]);
    assert_eq!(names(&f), ["m", "z", "b"]);
    let bins = format!("{}:{}", t.p("two/b/bin").display(), t.p("one/z/bin").display());
    assert_eq!(child_path(&f.exts, "/usr/bin:/bin"), format!("{bins}:/usr/bin:/bin"));
    assert_eq!(child_path(&f.exts, ""), bins);
    assert_eq!(child_path(&[], "/usr/bin"), "/usr/bin");
}

#[test]
fn ext_symlink_targets() {
    let t = T::new();
    t.ext("real/a", "# a");
    t.exe("real/a/bin/x");
    t.dir("e");
    symlink(t.p("real/a"), t.p("e/a")).unwrap();
    t.dir("e/b");
    symlink(t.file("real/b-doc.md", "# b"), t.p("e/b/doc.md")).unwrap();
    t.dir("e/b/bin");
    symlink(t.exe("real/y"), t.p("e/b/bin/y")).unwrap();
    let f = ok(&[t.p("e")]);
    let (a, b) = (&f.exts[0], &f.exts[1]);
    assert_eq!(a.paths, [t.p("e/a"), t.p("e/a/doc.md"), t.p("e/a/bin"), t.p("e/a/bin/x")]);
    assert_eq!(a.targets, [t.p("real/a")]);
    assert_eq!((b.line.as_str(), &b.doc), ("b", &t.p("e/b/doc.md")));
    assert_eq!(b.targets, [t.p("real/b-doc.md"), t.p("real/y")]);
}

#[test]
fn ext_optional_absent() {
    let t = T::new();
    let f = discover(&[t.p("none")], true, &[], OsStr::new("/usr/bin:rel")).map_err(|e| panic!("{e}")).ok().unwrap();
    assert!(f.exts.is_empty());
    assert_eq!(f.user_path, "/usr/bin");
    let file = t.file("file", "x");
    assert!(discover(&[file], true, &[], OsStr::new("")).is_err(), "only absence is optional");
}

#[test]
fn ext_listed_missing_fails() {
    let t = T::new();
    let want = format!("ext {}: No such file or directory (os error 2); fix: make it a readable directory, or set ext to readable ones", t.p("none").display());
    assert_eq!(err(&[t.p("none")]), want);
}

#[test]
fn ext_doc_fifo_or_device_no_hang() {
    let t = T::new();
    let d = t.dir("e/a");
    assert!(std::process::Command::new("mkfifo").arg(d.join("doc.md")).status().unwrap().success());
    // Off the test thread, so a discovery that blocked would fail here instead of hanging the run.
    let in_time = |paths: Vec<PathBuf>| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || tx.send(find(&paths, &[], "").err()).unwrap());
        rx.recv_timeout(Duration::from_secs(30)).expect("discovery blocked")
    };
    assert_eq!(in_time(vec![t.p("e")]).unwrap(), format!("ext {0}: not a regular file: {0}/doc.md (fifo); {DOC_FIX}", d.display()));
    fs::remove_file(d.join("doc.md")).unwrap();
    symlink("/dev/zero", d.join("doc.md")).unwrap();
    assert_eq!(in_time(vec![t.p("e")]).unwrap(), format!("ext {0}: not a regular file: {0}/doc.md (char device); {DOC_FIX}", d.display()));
}

#[test]
fn ext_doc_line_tab() {
    let t = T::new();
    t.ext("e/a", "# a\tb \t\n");
    assert_eq!(ok(&[t.p("e")]).exts[0].line, "a\tb", "a TAB inside the line is kept, around it trimmed");
}

fn bin_refused(p: &Path) -> String {
    format!("ext {}: a command is a file you can run, named with A-Z, a-z, 0-9, ., _ and -; fix: rename it or chmod u+x it, or move it out of bin/", p.display())
}

#[test]
fn ext_bin_group_exec_only() {
    let t = T::new();
    t.ext("e/a", "# a");
    let x = t.exe("e/a/bin/x");
    for mode in [0o055, 0o011, 0o644] {
        fs::set_permissions(&x, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(err(&[t.p("e")]), bin_refused(&x), "{mode:o}");
    }
    fs::set_permissions(&x, fs::Permissions::from_mode(0o100)).unwrap();
    assert_eq!(ok(&[t.p("e")]).exts[0].cmds, ["x"], "the user exec bit is the one");
}

#[test]
fn ext_bin_broken_symlink() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.dir("e/a/bin");
    let x = t.p("e/a/bin/x");
    symlink(t.p("gone"), &x).unwrap();
    assert_eq!(err(&[t.p("e")]), bin_refused(&x));
}

#[test]
fn ext_bin_symlink_to_dir() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.dir("e/a/bin");
    let x = t.p("e/a/bin/x");
    symlink(t.dir("elsewhere"), &x).unwrap();
    assert_eq!(err(&[t.p("e")]), bin_refused(&x));
}

#[test]
fn ext_bin_dotfile() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.exe("e/a/bin/x");
    // A dot entry in bin/ that cannot run is no command (Finder's, or what a copy from a Mac leaves), but it is watched:
    // made runnable, it would be one.
    for n in [".DS_Store", ".gitkeep", "._x"] {
        t.file(&format!("e/a/bin/{n}"), "");
    }
    t.dir("e/a/bin/.git");
    let f = ok(&[t.p("e")]);
    let watched = |names: &[&str]| [t.p("e/a"), t.p("e/a/doc.md"), t.p("e/a/bin")].into_iter().chain(names.iter().map(|n| t.p(&format!("e/a/bin/{n}")))).collect::<Vec<_>>();
    assert_eq!((&f.exts[0].cmds, &f.exts[0].paths), (&vec!["x".to_string()], &watched(&[".DS_Store", "._x", ".git", ".gitkeep", "x"])));
    // One that can run is a command like any other: listed and watched.
    t.exe("e/a/bin/.x");
    let f = ok(&[t.p("e")]);
    assert_eq!((&f.exts[0].cmds, &f.exts[0].paths), (&vec![".x".to_string(), "x".to_string()], &watched(&[".DS_Store", "._x", ".git", ".gitkeep", ".x", "x"])));
    // Without the dot, a file that cannot run is refused as before.
    let p = t.file("e/a/bin/y", "");
    assert_eq!(err(&[t.p("e")]), bin_refused(&p));
}

#[test]
fn ext_path_with_colon_refused() {
    let t = T::new();
    t.ext("a:rel/e/x", "# x");
    assert_eq!(names(&ok(&[t.p("a:rel/e")])), ["x"], "a doc-only extension adds nothing to PATH");
    let tool = t.exe("a:rel/e/x/bin/tool");
    let want = format!("ext {}: a PATH entry cannot hold ':' or bytes that are not UTF-8; fix: move the extensions to a path without them", tool.parent().unwrap().display());
    assert_eq!(err(&[t.p("a:rel/e")]), want);
    let odd = t.0.join(OsStr::from_bytes(b"\xff"));
    fs::create_dir_all(odd.join("y/bin")).unwrap();
    fs::write(odd.join("y/doc.md"), "# y").unwrap();
    assert!(err(&[odd]).ends_with(": a PATH entry cannot hold ':' or bytes that are not UTF-8; fix: move the extensions to a path without them"));
}

#[test]
fn ext_user_path_non_utf8_entry() {
    let path = OsStr::from_bytes(b"/usr/bin:/opt/\xff/bin:/bin");
    let f = discover(&[], false, &[], path).map_err(|e| panic!("{e}")).ok().unwrap();
    assert_eq!(f.user_path, "/usr/bin:/bin", "an entry that is not UTF-8 is dropped, like a relative one");
}

#[test]
fn ext_unreadable_name_dir() {
    let t = T::new();
    let d = t.ext("e/a", "# a");
    fs::set_permissions(&d, fs::Permissions::from_mode(0o000)).unwrap();
    let e = find(&[t.p("e")], &[], "").err();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    if sys::euid() != 0 {
        let want = format!("ext {}: Permission denied (os error 13); fix: make it an extension directory, or move it out of the ext path", d.display());
        assert_eq!(e.unwrap(), want);
    }
}

#[test]
fn ext_optional_dangling_symlink() {
    let t = T::new();
    symlink(t.p("gone"), t.p("ext")).unwrap();
    let f = discover(&[t.p("ext")], true, &[], OsStr::new("")).map_err(|e| panic!("{e}")).ok().unwrap();
    assert!(f.exts.is_empty(), "the optional path is absent through a dangling link too");
    assert_eq!(err(&[t.p("ext")]), format!("ext {}: No such file or directory (os error 2); fix: make it a readable directory, or set ext to readable ones", t.p("ext").display()));
}
