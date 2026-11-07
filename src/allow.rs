//! Allow rules: the user's standing approvals for exact shell commands in one project root, kept in a plain text file
//! the user can read, edit and empty.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub n: usize,
    pub root: String,
    pub cmd: String,
}

#[derive(Debug, Default)]
pub struct Allow {
    pub file: PathBuf,
    pub rules: Vec<Rule>,
    pub hash: Option<String>,
}
