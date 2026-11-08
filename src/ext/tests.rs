#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use crate::sys;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{sync::mpsc, time::Duration};

const DOC_FIX: &str = "fix: write doc.md with a summary of the extension on line 1";
const LINE_RULE: &str = "line 1 must be a summary of 1 to 160 bytes";

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
    t.ext("e/a", "## #  Title: does x  \nbody\n");
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
    for doc in ["#  \nbody", "", &format!("# {}", "x".repeat(161))] {
        let d = t.ext("e/a", doc);
        assert_eq!(err(&[t.p("e")]), format!("ext {}/doc.md: {LINE_RULE}; {DOC_FIX}", d.display()), "{doc:?}");
    }
    let d = t.p("e/a");
    let missing = format!("ext {0}/doc.md: No such file or directory (os error 2); fix: make {0}/doc.md a readable regular file", d.display());
    fs::remove_file(d.join("doc.md")).unwrap();
    assert_eq!(err(&[t.p("e")]), missing);
    symlink("gone", d.join("doc.md")).unwrap();
    assert_eq!(err(&[t.p("e")]), missing, "a broken symlink");
    fs::remove_file(d.join("doc.md")).unwrap();
    t.dir("e/a/doc.md");
    assert_eq!(err(&[t.p("e")]), format!("ext not a regular file: {0}/doc.md (directory); fix: make {0}/doc.md a readable regular file", d.display()));
}

/// The whole doc.md is held to AGENTS.md's rules: at most 16 KiB of UTF-8, no character a terminal does not show on
/// any line, and no other user may change it.
#[test]
fn ext_doc_whole_rules() {
    let t = T::new();
    let doc = t.p("e/a/doc.md");
    for (text, line, u) in [("a\u{200b}b", 1, 0x200B), ("red \x1b[31m", 1, 0x1B), ("a\rb", 1, 0xD), ("summary\n\nbody \u{202e}", 3, 0x202E)] {
        t.ext("e/a", text);
        let want = format!("ext {}:{line}: holds U+{u:04X}, which a terminal does not show; fix: remove it", doc.display());
        assert_eq!(err(&[t.p("e")]), want, "{text:?}");
    }
    t.file("e/a/doc.md", b"\xff\xfe summary\n");
    assert_eq!(err(&[t.p("e")]), format!("ext {}: not UTF-8; fix: save it as UTF-8", doc.display()));
    t.ext("e/a", &format!("a: summary\n{}", "x".repeat(16 << 10)));
    assert_eq!(err(&[t.p("e")]), format!("ext {}: over 16,384 bytes; fix: shorten it", doc.display()));
    t.ext("e/a", &format!("a: summary\n{}", "x".repeat((16 << 10) - 11)));
    assert_eq!(ok(&[t.p("e")]).exts[0].line, "a: summary", "exactly 16 KiB");
    fs::set_permissions(&doc, fs::Permissions::from_mode(0o664)).unwrap();
    assert_eq!(err(&[t.p("e")]), format!("ext {0} is group- or world-writable; fix: chmod go-w {0}", doc.display()));
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
    for n in ["notes.txt", "lib", "tool", "gate", "tools.json", "transport", "ctx", "cmds", "binary", "evals", "events"] {
        let p = if n.contains('.') { t.file(&format!("e/a/{n}"), "x") } else { t.dir(&format!("e/a/{n}")) };
        let want = format!("ext {}: unknown entry {}; ext/2 modules hold doc.md, bin/, cmd/, on/, port/ and eval/; fix: move it out of the extension", d.display(), p.display());
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
    t.dir("e/b/bin");
    t.file("e/b/.henki/doc.md", "# b");
    symlink(".henki/doc.md", t.p("e/b/doc.md")).unwrap();
    t.exe("e/b/.henki/y");
    symlink("../.henki/./y", t.p("e/b/bin/y")).unwrap();
    let f = ok(&[t.p("e")]);
    let (a, b) = (&f.exts[0], &f.exts[1]);
    assert_eq!(a.paths, [t.p("e/a"), t.p("e/a/doc.md"), t.p("e/a/bin"), t.p("e/a/bin/x")]);
    assert_eq!(a.targets, [t.p("real/a")]);
    assert_eq!((b.line.as_str(), &b.doc), ("b", &t.p("e/b/doc.md")));
    assert_eq!(b.targets, [t.p("e/b/.henki/doc.md"), t.p("e/b/.henki/y")]);
}

/// Inside a module a link must be relative and stay in it: an absolute one, or one that leaves through `..` or through
/// another link, names a file the manager's copy does not pin.
#[test]
fn ext_link_rule() {
    let t = T::new();
    t.ext("e/a", "# a");
    t.exe("e/a/.henki/tool");
    let outside = t.exe("elsewhere/tool");
    t.dir("e/a/bin");
    let link = t.p("e/a/bin/tool");
    let want = format!("ext {}: a link that leaves the module; fix: copy the file in", link.display());
    symlink("../../../elsewhere", t.p("e/a/.henki/out")).unwrap();
    for target in [outside.to_str().unwrap(), t.p("e/a/.henki/tool").to_str().unwrap(), "../../../elsewhere/tool", "../.henki/out/tool"] {
        symlink(target, &link).unwrap();
        assert_eq!(err(&[t.p("e")]), want, "{target}");
        fs::remove_file(&link).unwrap();
    }
    symlink("../.henki/tool", &link).unwrap();
    assert_eq!(ok(&[t.p("e")]).exts[0].cmds, ["tool"]);
    // The module dir itself may be a link (the manager links the store copy); doc.md may not leave it.
    fs::remove_file(t.p("e/a/doc.md")).unwrap();
    symlink(t.file("doc.md", "# a"), t.p("e/a/doc.md")).unwrap();
    assert_eq!(err(&[t.p("e")]), format!("ext {}: a link that leaves the module; fix: copy the file in", t.p("e/a/doc.md").display()));
}

/// cmd/: runnable entries are the user's commands, NAME.md files their data (held to doc.md's rules), names henki keeps
/// for itself are refused, and a module with cmd/ or port/ but no bin/ is not in the prompt.
#[test]
fn ext_cmd_rules() {
    let t = T::new();
    t.ext("e/a", "# a: commands");
    let (r, md) = (t.exe("e/a/cmd/review"), t.file("e/a/cmd/review.md", "Review this.\n"));
    let dot = t.file("e/a/cmd/.DS_Store", "x");
    let f = ok(&[t.p("e")]);
    let a = &f.exts[0];
    assert_eq!((&a.user, a.cmds.is_empty(), a.prompt, &a.dir), (&vec!["review".to_string()], true, false, &t.p("e/a")));
    assert_eq!(a.paths, [t.p("e/a"), t.p("e/a/doc.md"), t.p("e/a/cmd"), dot, r.clone(), md.clone()]);
    assert_eq!(prompt(&f.exts, &t.0), "", "a module of commands costs no prompt bytes");
    t.file("e/a/cmd/review.md", "Review \u{202e}this.\n");
    assert_eq!(err(&[t.p("e")]), format!("ext {}:1: holds U+202E, which a terminal does not show; fix: remove it", md.display()));
    t.file("e/a/cmd/review.md", "x".repeat((16 << 10) + 1));
    assert_eq!(err(&[t.p("e")]), format!("ext {}: over 16,384 bytes; fix: shorten it", md.display()));
    fs::remove_file(&md).unwrap();
    let plain = t.file("e/a/cmd/notes.txt", "x");
    let refused =
        |p: &Path| format!("ext {}: a command is a file you can run, named with A-Z, a-z, 0-9, ., _ and -; fix: rename it or chmod u+x it, or move it out of cmd/", p.display());
    assert_eq!(err(&[t.p("e")]), refused(&plain));
    fs::remove_file(&plain).unwrap();
    for n in ["exit", "help", "mods", "allow"] {
        let p = t.exe(&format!("e/a/cmd/{n}"));
        assert_eq!(err(&[t.p("e")]), format!("ext {}: a name henki keeps for its own command; fix: rename it", p.display()));
        fs::remove_file(&p).unwrap();
    }
    t.exe("e/a/bin/tool");
    assert!(ok(&[t.p("e")]).exts[0].prompt, "with bin/ the model needs the doc");
    // A command name in two modules stops the start.
    t.ext("e/b", "# b");
    t.exe("e/b/cmd/review");
    assert_eq!(err(&[t.p("e")]), "ext b: cmd/review is also in ext a; fix: turn one of them off");
}

/// port/: programs named for a port, stamped; a module of bindings only is not in the prompt.
#[test]
fn ext_port_rules() {
    let t = T::new();
    t.ext("e/a", "# a: a store");
    let h = t.exe("e/a/port/history");
    t.dir("e/a/eval/x");
    let f = ok(&[t.p("e")]);
    assert_eq!((f.exts[0].prompt, &f.exts[0].paths[2..]), (false, &[t.p("e/a/port"), h][..]));
    let p = t.exe("e/a/port/journal");
    assert_eq!(err(&[t.p("e")]), format!("ext {}: not a port; ports are model, history, context, exec, policy, frontend; fix: rename it, or move it out of port/", p.display()));
}

/// on/: an event, or a `.wait` one for turn_start and call_start; stamped; a module of observers only is not in the
/// prompt; at most 16 over all modules.
#[test]
fn ext_on_rules() {
    let t = T::new();
    t.ext("e/a", "# a: watches");
    let (w, e, dot) = (t.exe("e/a/on/call_start.wait"), t.exe("e/a/on/turn_end"), t.file("e/a/on/.keep", ""));
    symlink("turn_end", t.p("e/a/on/question")).unwrap();
    let f = ok(&[t.p("e")]);
    assert_eq!((&f.exts[0].on, f.exts[0].prompt), (&vec!["call_start.wait".to_string(), "question".into(), "turn_end".into()], false));
    assert_eq!(f.exts[0].paths[2..], [t.p("e/a/on"), dot, w, t.p("e/a/on/question"), e]);
    assert_eq!(f.exts[0].targets, [t.p("e/a/on/turn_end")], "the link's target is protected");
    assert_eq!(prompt(&f.exts, &t.0), "", "a module of observers costs no prompt bytes");
    let events = "session_start, turn_start, call_start, call_end, turn_end, question, session_end, record";
    for n in ["call_end.wait", "turn_end.wait", "record.wait", "stop", "turn_start.WAIT", "call_start.wait.wait", "session_start.sh"] {
        let p = t.exe(&format!("e/a/on/{n}"));
        let want = format!("ext {}: not an event; on/ holds {events}, and turn_start.wait or call_start.wait; fix: rename it", p.display());
        assert_eq!(err(&[t.p("e")]), want);
        fs::remove_file(&p).unwrap();
    }
    let plain = t.file("e/a/on/turn_start", "#!/bin/sh\n");
    assert!(err(&[t.p("e")]).contains("a command is a file you can run"), "an observer that cannot run is refused");
    fs::remove_file(&plain).unwrap();
    for (i, ev) in ["session_start", "turn_start", "turn_start.wait", "call_start", "call_end", "session_end"].iter().enumerate() {
        t.ext(&format!("e/b{i}"), "# b");
        t.exe(&format!("e/b{i}/on/{ev}"));
        t.exe(&format!("e/b{i}/on/turn_end"));
    }
    t.exe("e/b0/on/record");
    assert_eq!(ok(&[t.p("e")]).exts.iter().map(|x| x.on.len()).sum::<usize>(), 16);
    t.exe("e/b1/on/record");
    assert_eq!(err(&[t.p("e")]), format!("ext {}: more than 16 observers (on/ files) over all modules; fix: turn a module off", t.p("e/b5").display()));
}

/// Nothing in a module, nor the ext path above it, that another user can write: it would run with the user's rights.
#[test]
fn ext_other_writable_refused() {
    let t = T::new();
    t.ext("e/a", "# a: watches");
    let (o, b, c) = (t.exe("e/a/on/turn_end"), t.exe("e/a/bin/x"), t.exe("e/a/cmd/y"));
    for p in [t.p("e"), t.p("e/a"), t.p("e/a/on"), o, t.p("e/a/bin"), b, t.p("e/a/cmd"), c] {
        let m = fs::metadata(&p).unwrap().mode() & 0o7777;
        for bits in [0o020, 0o002] {
            fs::set_permissions(&p, fs::Permissions::from_mode(m | bits)).unwrap();
            assert_eq!(err(&[t.p("e")]), format!("ext {0} is group- or world-writable; fix: chmod go-w {0}", p.display()));
        }
        fs::set_permissions(&p, fs::Permissions::from_mode(m)).unwrap();
    }
    assert_eq!(ok(&[t.p("e")]).exts[0].on, ["turn_end"]);
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
    let fix = format!("fix: make {}/doc.md a readable regular file", d.display());
    assert_eq!(in_time(vec![t.p("e")]).unwrap(), format!("ext not a regular file: {}/doc.md (fifo); {fix}", d.display()));
    fs::remove_file(d.join("doc.md")).unwrap();
    symlink("/dev/zero", d.join("doc.md")).unwrap();
    assert_eq!(in_time(vec![t.p("e")]).unwrap(), format!("ext not a regular file: {}/doc.md (char device); {fix}", d.display()));
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
