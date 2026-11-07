//! Extensions: directories that teach the model a CLI. `NAME/doc.md`'s first line goes into the prompt; an optional
//! `NAME/bin/` goes first on the tool PATH.
use std::{ffi::OsStr, path::Path, path::PathBuf};

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

/// Discovery over `paths`. `optional`: the paths may be absent (the default path of the default config). Err: `ext PATH:
/// PROBLEM; fix: ...`.
pub fn discover(paths: &[PathBuf], optional: bool, shadow: &[String], path: &OsStr) -> Result<Found, String> {
    let _ = (paths, optional, shadow, path);
    todo!()
}

/// The system prompt's extensions item; "" without extensions.
pub fn prompt(exts: &[Ext], home: &Path) -> String {
    let _ = (exts, home);
    todo!()
}

/// The tool child's PATH: every `bin/` in extension name order, then `user_path`.
pub fn child_path(exts: &[Ext], user_path: &str) -> String {
    let _ = (exts, user_path);
    todo!()
}
