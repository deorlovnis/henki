//! Extensions: directories that teach the model a CLI. `NAME/doc.md`'s first line goes into the prompt; an optional
//! `NAME/bin/` goes first on the tool PATH.
use crate::{allow, sys, term};
use std::path::{Path, PathBuf};
use std::{
    collections::BTreeMap, collections::BTreeSet, ffi::OsStr, ffi::OsString, fmt::Display, fs, io, io::BufRead, io::Read, os::unix::ffi::OsStrExt, os::unix::fs::MetadataExt,
};

pub struct Ext {
    pub name: String,
    pub line: String,          // doc.md line 1 without a leading `#` and spaces, trimmed
    pub doc: PathBuf,          // <ext path>/NAME/doc.md
    pub bin: Option<PathBuf>,  // <ext path>/NAME/bin when present
    pub cmds: Vec<String>,     // bin/ entry names, sorted bytewise
    pub shadows: Vec<PathBuf>, // user-PATH executables its commands shadow (each listed in `shadow`)
    pub paths: Vec<PathBuf>,   // stamped by the tripwire: the NAME dir, doc.md, bin/, each bin/ entry
    pub targets: Vec<PathBuf>, // realpaths of the symlinks among `paths` (protected)
}

/// The extensions in ext path order (names sorted bytewise within a path), the user PATH (the inherited `path` without
/// relative entries and without any extension's `bin/`), and notices.
pub struct Found {
    pub exts: Vec<Ext>,
    pub user_path: String,
    pub notices: Vec<String>,
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
            exts.push(x);
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
    Ok(Found { exts, user_path: dirs.join(":"), notices: Vec::new() })
}

/// One extension directory: doc.md, an optional bin/, ignored names, nothing else.
fn one(dir: &Path, name: &str) -> Result<Ext, String> {
    let entries = names(dir).map_err(|e| bad(dir, e, "make it an extension directory, or move it out of the ext path"))?;
    let known = |n: &&OsString| *n == "doc.md" || *n == "bin" || ["README", "LICENSE", "CHANGELOG", "."].iter().any(|p| n.as_bytes().starts_with(p.as_bytes()));
    if let Some(n) = entries.iter().find(|n| !known(n)) {
        return Err(bad(dir, format!("unknown entry {}; v1 extensions hold doc.md and bin/ only", dir.join(n).display()), "move it out of the extension"));
    }
    let (doc, fix) = (dir.join("doc.md"), "write doc.md with a summary of the extension on line 1");
    // Line 1, read through at most 4 KiB, without a leading run of `#` and spaces, trimmed.
    let (f, _) = sys::open_regular(&doc).map_err(|e| bad(dir, e, fix))?;
    let mut line = String::new();
    let read = io::BufReader::new(f.take(4 << 10)).read_line(&mut line).is_ok();
    let line = line.trim_start_matches(['#', ' ']).trim();
    if !read || line.is_empty() || line.len() > 160 || line.contains(term::escaped) {
        return Err(bad(&doc, "line 1 must be a summary of 1 to 160 bytes with no invisible characters", fix));
    }
    let bin = entries.contains(OsStr::new("bin")).then(|| dir.join("bin"));
    let (mut paths, mut cmds) = (vec![dir.to_path_buf(), doc.clone()], Vec::new());
    if let Some(b) = &bin {
        paths.push(b.clone());
        for c in names(b).map_err(|e| bad(b, e, "make bin a directory of commands"))? {
            let p = b.join(&c);
            if !c.to_str().is_some_and(cmd_name) || !fs::metadata(&p).is_ok_and(|m| m.is_file() && m.mode() & 0o100 != 0) {
                return Err(bad(&p, "a command is a file you can run, named with A-Z, a-z, 0-9, ., _ and -", "rename it or chmod u+x it, or move it out of bin/"));
            }
            cmds.push(c.to_string_lossy().into_owned());
            paths.push(p);
        }
    }
    let targets = paths.iter().filter(|p| p.is_symlink()).filter_map(|p| fs::canonicalize(p).ok()).collect();
    Ok(Ext { name: name.into(), line: line.into(), doc, bin, cmds, shadows: Vec::new(), paths, targets })
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

/// The system prompt's extensions item; "" without extensions.
pub fn prompt(exts: &[Ext], home: &Path) -> String {
    let lines = exts.iter().map(|x| format!("\n- {}: {} [{}]", x.name, x.line, allow::shown(&x.doc, home)));
    if exts.is_empty() { String::new() } else { "Extensions are commands on PATH; read the doc in brackets before first use.".to_string() + &lines.collect::<String>() }
}

/// The tool child's PATH: every `bin/` in extension name order, then `user_path`.
pub fn child_path(exts: &[Ext], user_path: &str) -> String {
    let bins: BTreeMap<&str, String> = exts.iter().filter_map(|x| Some((x.name.as_str(), x.bin.as_ref()?.display().to_string()))).collect();
    bins.into_values().chain(Some(user_path.to_string()).filter(|u| !u.is_empty())).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests;
