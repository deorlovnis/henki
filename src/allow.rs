//! Allow rules: the user's standing approvals for exact shell commands in one project root, kept in a plain text file
//! the user can read, edit and empty.
use std::{io, path::Path, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub n: usize,
    pub root: String,
    pub cmd: String,
}

#[derive(Debug, Default)]
pub struct Allow {
    pub file: PathBuf,        // <config dir>/allow
    pub rules: Vec<Rule>,     // in file order; empty when the file is absent or not read (auto)
    pub hash: Option<String>, // FNV-1a of the bytes read; None when absent or not read
}

/// The allow file for rule use (start). `read` false (auto mode): nothing is read. Fails closed (exit 2 `config`).
pub fn load(file: &Path, read: bool) -> Result<Allow, String> {
    let _ = (file, read);
    todo!()
}

/// Rule `r` holds in the project root `root`.
pub fn applies(r: &Rule, root: &Path) -> bool {
    let _ = (r, root);
    todo!()
}

impl Allow {
    /// The rule whose COMMAND is `cmd` byte for byte and whose ROOT is `root` or `*`.
    pub fn find(&self, cmd: &str, root: &Path) -> Option<&Rule> {
        let _ = (cmd, root);
        todo!()
    }

    /// A confirmed `always`: append `ROOT<TAB>CMD\n`; Ok(the new rule's number).
    pub fn append(&mut self, root: &Path, cmd: &str) -> io::Result<usize> {
        let _ = (root, cmd);
        todo!()
    }
}

/// `cmd` can be a rule's COMMAND (printable ASCII, 1..=200 bytes, not starting with `#` or a space, no run of more than
/// 32 spaces) and `root` a rule's ROOT.
pub fn fits(cmd: &str, root: &Path) -> bool {
    let _ = (cmd, root);
    todo!()
}

/// `/allow` and `henki allow`: the file's absolute path, then one line per rule. Never fails closed.
pub fn list(file: &Path, home: &Path) -> Vec<String> {
    let _ = (file, home);
    todo!()
}

/// Removes rule `n`, comments kept. Ok: `removed rule N: ROOT  COMMAND`; Err: `no rule N in FILE`, or the IO error.
pub fn remove(file: &Path, n: usize, home: &Path) -> Result<String, String> {
    let _ = (file, n, home);
    todo!()
}

/// A path with `$HOME` shown as `~`.
pub fn shown(p: &Path, home: &Path) -> String {
    let _ = (p, home);
    todo!()
}
