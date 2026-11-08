//! Extensions (`ext/2` modules): directories that teach the model a CLI and give the user commands. `NAME/doc.md`'s
//! first line goes into the prompt; an optional `NAME/bin/` goes first on the tool PATH; `NAME/cmd/X` is the user's
//! `/X` (`handoff`); `NAME/on/EVENT` watches the session (`observe`); `NAME/port/PORT` is a binding that only a link
//! in `CONFIG/port/` runs.
use crate::{allow, config, observe, port, sys, term};
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::{collections::BTreeMap, collections::BTreeSet, ffi::OsStr, ffi::OsString, fmt::Display, fs, os::unix::ffi::OsStrExt, os::unix::fs::MetadataExt};

pub struct Ext {
    pub name: String,
    pub line: String,          // doc.md line 1 without a leading `#` and spaces, trimmed
    pub dir: PathBuf,          // <ext path>/NAME, the module's HENKI_MODULE
    pub doc: PathBuf,          // <ext path>/NAME/doc.md
    pub bin: Option<PathBuf>,  // <ext path>/NAME/bin when present
    pub cmds: Vec<String>,     // bin/ entry names, sorted bytewise
    pub user: Vec<String>,     // cmd/ entry names, sorted bytewise: the user's `/NAME` commands
    pub on: Vec<String>,       // on/ entry names, sorted bytewise: `EVENT` or `EVENT.wait` (observers)
    pub prompt: bool,          // in the system prompt: with bin/, or with no cmd/, on/ or port/ (a doc for the model)
    pub shadows: Vec<PathBuf>, // user-PATH executables its commands shadow (each listed in `shadow`)
    pub paths: Vec<PathBuf>,   // stamped by the tripwire: the NAME dir, doc.md, bin/, cmd/, on/, port/ and each entry
    pub targets: Vec<PathBuf>, // realpaths of the symlinks among `paths` (protected)
}

/// The extensions in ext path order (names sorted bytewise within a path), and the user PATH (the inherited `path`
/// without relative entries and without any extension's `bin/`).
pub struct Found {
    pub exts: Vec<Ext>,
    pub user_path: String,
}

/// Every ext path in order. `optional`: the paths may be absent (the default path of the default config). Nothing is
/// run. Err: `ext PATH: PROBLEM; fix: ...`.
pub fn discover(paths: &[PathBuf], optional: bool, shadow: &[String], path: &OsStr) -> Result<Found, String> {
    let mut exts: Vec<Ext> = Vec::new();
    for p in paths {
        let names = names(p).or_else(|e| if optional && e.kind() == io::ErrorKind::NotFound { Ok(BTreeSet::new()) } else { Err(e) });
        let names = names.map_err(|e| bad(p, e, "make it a readable directory, or set ext to readable ones"))?;
        for n in names.iter().filter(|n| !n.as_bytes().starts_with(b".")) {
            let dir = p.join(n);
            let name = n.to_str().filter(|n| ext_name(n)).ok_or_else(|| bad(&dir, "a name is 1 to 32 of a-z, 0-9, _ and -, starting with a-z or 0-9", "rename it"))?;
            if let Some(o) = exts.iter().find(|o| o.name == name) {
                return Err(bad(&dir, format!("{} has the same name", o.paths[0].display()), "keep one of them"));
            }
            let x = one(&dir, name)?;
            if let Some((o, c)) = exts.iter().find_map(|o| Some((o, x.cmds.iter().find(|c| o.cmds.contains(c))?))) {
                return Err(format!("ext {name}: bin/{c} is also in ext {}; fix: remove one of them", o.name));
            }
            if let Some((o, c)) = exts.iter().find_map(|o| Some((o, x.user.iter().find(|c| o.user.contains(c))?))) {
                return Err(format!("ext {name}: cmd/{c} is also in ext {}; fix: turn one of them off", o.name));
            }
            exts.push(x);
            if exts.iter().map(|x| x.on.len()).sum::<usize>() > observe::MAX {
                return Err(bad(&dir, format!("more than {} observers (on/ files) over all modules", observe::MAX), "turn a module off"));
            }
        }
    }
    if let Some(s) = shadow.iter().find(|s| !s.split_once('/').is_some_and(|(e, c)| ext_name(e) && cmd_name(c))) {
        return Err(format!("shadow entry {s:?} is not EXT/COMMAND; fix: name an extension and one of its bin/ commands, like \"halti/halti\""));
    }
    // A user PATH entry that is an extension's bin/ would make a wrapper's `PATH="$HENKI_USER_PATH" exec NAME` run itself.
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let bins: Vec<PathBuf> = exts.iter().filter_map(|x| x.bin.as_deref()).map(real).collect();
    let dirs: Vec<String> = std::env::split_paths(path).filter(|d| d.is_absolute() && !bins.contains(&real(d))).filter_map(|d| d.into_os_string().into_string().ok()).collect();
    let exe = |p: &PathBuf| fs::metadata(p).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0);
    for x in &mut exts {
        for (c, found) in x.cmds.iter().filter_map(|c| Some((c, dirs.iter().map(|d| Path::new(d).join(c)).find(exe)?))) {
            if !shadow.contains(&format!("{}/{c}", x.name)) {
                return Err(format!("ext {0}: bin/{c} shadows {1}; fix: add shadow = [\"{0}/{c}\"] if that is intended", x.name, found.display()));
            }
            x.shadows.push(found);
        }
    }
    Ok(Found { exts, user_path: dirs.join(":") })
}

/// One extension directory: doc.md, then the optional bin/, cmd/, on/ and port/, ignored names, nothing else. Every link in
/// it stays inside it.
fn one(dir: &Path, name: &str) -> Result<Ext, String> {
    let entries = names(dir).map_err(|e| bad(dir, e, "make it an extension directory, or move it out of the ext path"))?;
    // `port/` holds bindings that `CONFIG/port/` links, `eval/` the manager's tasks: henki runs nothing from either here.
    let known = |n: &&OsString| {
        ["doc.md", "bin", "cmd", "on", "port", "eval"].iter().any(|p| *n == p) || ["README", "LICENSE", "CHANGELOG", "."].iter().any(|p| n.as_bytes().starts_with(p.as_bytes()))
    };
    if let Some(n) = entries.iter().find(|n| !known(n)) {
        return Err(bad(dir, format!("unknown entry {}; ext/2 modules hold doc.md, bin/, cmd/, on/, port/ and eval/", dir.join(n).display()), "move it out of the extension"));
    }
    // The whole doc steers the model as AGENTS.md does, so it is held to the same rules; line 1 is the summary. A module
    // ext/1 knows (none of cmd/, on/, port/ and eval/) keeps ext/1's rules for one release more: line 1 of doc.md is all
    // that is read, and a link may leave it (the Stability rule of CONTRACTS.md).
    let (doc, v1) = (dir.join("doc.md"), !["cmd", "on", "port", "eval"].iter().any(|d| entries.contains(OsStr::new(d))));
    let text = if v1 { first(&doc) } else { config::text(&doc, 16 << 10) }.map_err(|e| format!("ext {e}"))?;
    let line = text.split('\n').next().unwrap_or_default().trim_start_matches(['#', ' ']).trim();
    if line.is_empty() || line.len() > 160 || line.contains(term::escaped) {
        return Err(bad(&doc, "line 1 must be a summary of 1 to 160 bytes", "write doc.md with a summary of the extension on line 1"));
    }
    let has = |d: &str| entries.contains(OsStr::new(d)).then(|| dir.join(d));
    let bin = has("bin");
    // The child's PATH is a `:`-joined string: such a bin/ would split into other entries, one of them relative.
    if let Some(b) = bin.as_ref().filter(|b| b.to_str().is_none_or(|s| s.contains(':'))) {
        return Err(bad(b, "a PATH entry cannot hold ':' or bytes that are not UTF-8", "move the extensions to a path without them"));
    }
    let mut paths = vec![dir.to_path_buf(), doc.clone()];
    let mut part = |d: &str| has(d).map_or(Ok(Vec::new()), |p| scan(&p, d, &mut paths));
    let (cmds, user, on, ports) = (part("bin")?, part("cmd")?, part("on")?, part("port")?);
    if let Some(c) = user.iter().find(|c| ["exit", "help", "mods", "allow"].contains(&c.as_str())) {
        return Err(bad(&dir.join("cmd").join(c), "a name henki keeps for its own command", "rename it"));
    }
    if let Some(o) = on.iter().find(|o| observe::parse(o).is_none()) {
        return Err(bad(&dir.join("on").join(o), format!("not an event; on/ holds {}, and turn_start.wait or call_start.wait", observe::EVENTS.join(", ")), "rename it"));
    }
    if let Some(p) = ports.iter().find(|p| !port::PORTS.contains(&p.as_str())) {
        return Err(bad(&dir.join("port").join(p), format!("not a port; ports are {}", port::PORTS.join(", ")), "rename it, or move it out of port/"));
    }
    // The manager copies a module whole: a link out of it would name a file the lock does not pin.
    let real = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let out = |p: &&PathBuf| !fs::read_link(p).is_ok_and(|t| t.is_relative()) || !fs::canonicalize(p).is_ok_and(|t| t.starts_with(&real));
    if let Some(p) = paths.iter().skip(1).filter(|p| !v1 && p.is_symlink()).find(out) {
        return Err(bad(p, "a link that leaves the module", "copy the file in"));
    }
    // Observers run unasked on each event, as a port binding does, so on/ and its files are held to a binding's owner
    // and mode rule: the real file and every directory from it up to the module's, or another user could swap one in
    // (a link into a writable bin/, a writable module dir's on/) after this check, to run with no tripwire before it.
    let mode = |p: &Path| fs::metadata(p).map_or(Ok(()), |m| config::owner_mode(p, m.mode(), m.uid(), sys::euid(), false)).map_err(|e| format!("ext {e}"));
    let chain = |p: &PathBuf| fs::canonicalize(p).map_or(vec![p.clone()], |r| r.ancestors().take_while(|a| a.starts_with(&real)).map(Path::to_path_buf).collect());
    paths.iter().filter(|p| p.starts_with(dir.join("on"))).flat_map(chain).try_for_each(|p| mode(&p))?;
    let targets = paths.iter().filter(|p| p.is_symlink()).filter_map(|p| fs::canonicalize(p).ok()).collect();
    let prompt = bin.is_some() || ["cmd", "on", "port"].iter().all(|d| has(d).is_none());
    Ok(Ext { name: name.into(), line: line.into(), dir: dir.to_path_buf(), doc, bin, cmds, user, on, prompt, shadows: Vec::new(), paths, targets })
}

/// The runnable entries of `dir` (`bin/`, `cmd/`, `on/` or `port/` of a module) named by `cmd_name`, sorted bytewise; every
/// entry joins `paths`. A dot file that cannot run is no command (Finder's .DS_Store, the ._NAME files a copy from a
/// Mac leaves), but it is watched: made runnable, it would be one. In cmd/, a NAME.md is a command's data, held to
/// doc.md's rules. Anything else is refused.
fn scan(dir: &Path, sub: &str, paths: &mut Vec<PathBuf>) -> Result<Vec<String>, String> {
    let runs = |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.mode() & 0o100 != 0);
    let mut found = Vec::new();
    paths.push(dir.to_path_buf());
    for c in names(dir).map_err(|e| bad(dir, e, &format!("make {sub} a directory of commands")))? {
        let p = dir.join(&c);
        if runs(&p) && c.to_str().is_some_and(cmd_name) {
            found.push(c.to_string_lossy().into_owned());
        } else if sub == "cmd" && !runs(&p) && c.as_bytes().ends_with(b".md") {
            config::text(&p, 16 << 10).map_err(|e| format!("ext {e}"))?;
        } else if runs(&p) || !c.as_bytes().starts_with(b".") {
            return Err(bad(&p, "a command is a file you can run, named with A-Z, a-z, 0-9, ., _ and -", &format!("rename it or chmod u+x it, or move it out of {sub}/")));
        }
        paths.push(p);
    }
    Ok(found)
}

/// ext/1's doc.md: a regular file, its line 1 read through 4 KiB as UTF-8.
fn first(doc: &Path) -> Result<String, String> {
    let fail = |e: String| format!("{e}; fix: make {} a readable regular file", doc.display());
    let (f, mut line) = (sys::open_regular(doc).map_err(fail)?.0, String::new());
    io::BufReader::new(f.take(4 << 10)).read_line(&mut line).map(|_| line).map_err(|e| fail(format!("{}: {e}", doc.display())))
}

/// The names in `dir`, sorted bytewise.
fn names(dir: &Path) -> io::Result<BTreeSet<OsString>> {
    fs::read_dir(dir)?.map(|e| e.map(|e| e.file_name())).collect()
}

/// `[a-z0-9][a-z0-9_-]{0,31}`
fn ext_name(n: &str) -> bool {
    (1..=32).contains(&n.len()) && !n.starts_with(['_', '-']) && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// `[A-Za-z0-9._-]+`
fn cmd_name(n: &str) -> bool {
    !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn bad(path: &Path, problem: impl Display, fix: &str) -> String {
    format!("ext {}: {problem}; fix: {fix}", path.display())
}

/// The system prompt's extensions item: the modules with a `prompt`; "" without one.
pub fn prompt(exts: &[Ext], home: &Path) -> String {
    let lines: String = exts.iter().filter(|x| x.prompt).map(|x| format!("\n- {}: {} [{}]", x.name, x.line, allow::shown(&x.doc, home))).collect();
    if lines.is_empty() { lines } else { "Extensions are commands on PATH; read the doc in brackets before first use.".to_string() + &lines }
}

/// The tool child's PATH: every `bin/` in extension name order, then `user_path`.
pub fn child_path(exts: &[Ext], user_path: &str) -> String {
    let bins: BTreeMap<&str, String> = exts.iter().filter_map(|x| Some((x.name.as_str(), x.bin.as_ref()?.display().to_string()))).collect();
    bins.into_values().chain(Some(user_path.to_string()).filter(|u| !u.is_empty())).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests;
