//! What the model sees before its first call (the core prompt and the tool schemas), and what decides a call before it
//! runs: validation, the target it would touch, and whether it needs a human.
use crate::types::{Call, ToolOutput};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const ALL: [&str; 4] = ["shell", "read", "write", "edit"];
/// System part 1, byte-exact (563 B).
pub const PART1: &str = include_str!("prompt.txt");
pub const ARGS_MAX: usize = 1 << 20;
pub const READ_MAX: usize = 32 << 10;
pub const REPAIR: &str = "interrupted; the command may have partly run";

/// `include_str!("tools.json")` (1,104 B for all four) filtered to `enabled`, canonical order.
pub fn schema(enabled: &[&'static str]) -> Value {
    todo!()
}

/// Part 1, then "\n\n" and part 3 when it is not empty (today: the global AGENTS.md).
pub fn system(agents: Option<&str>) -> String {
    todo!()
}

/// Sets `c.error` to the first failing validation string; normalizes integer fields in `c.args`.
pub fn validate(c: &mut Call, enabled: &[&'static str]) {
    todo!()
}

pub struct Ctx<'a> {
    pub cwd: &'a Path,
    pub root: &'a Path,
    pub home: &'a Path,
    pub protected: &'a [PathBuf],
}

pub struct Target {
    pub path: PathBuf,          // resolved
    pub shown: String,          // `PATH`, or `PATH -> TARGET (symlink, outside project)` as they apply
    pub base: PathBuf,          // the nearest existing directory
    pub missing: Vec<OsString>, // components to create below `base` (write)
    pub name: OsString,         // the last component
    pub dir_id: (u64, u64),     // (dev, ino) of `base`
    pub mode: Option<u32>,      // an existing file's mode
    pub outside: bool,          // outside the project root
}

/// Resolve a file tool's path before anything is shown or run.
pub fn target(path: &str, write: bool, cx: &Ctx) -> Result<Target, String> {
    todo!()
}

/// `ask` mode: shell, write and edit always; read only outside the project root. `auto`: never.
pub fn needs_approval(name: &str, t: Option<&Target>, auto: bool) -> bool {
    todo!()
}

pub enum Killed {
    Timeout(u64),
    Interrupted,
}

/// A call's result text and `is_error`. Shell: the output or `(no output)`, then the footer. Files: the worker's text.
pub fn result_text(name: &str, out: &ToolOutput, killed: Option<&Killed>) -> (String, bool) {
    todo!()
}

/// `[not started: ENOENT bash]`.
pub fn not_started(e: &std::io::Error) -> String {
    todo!()
}

/// The plain-mode stderr line: `$ FIRST_LINE -> exit 0, 0.3 s`, `read PATH -> ok, 0.0 s`, `edit PATH -> error, 0.0 s`.
pub fn call_line(c: &Call, out: Option<&ToolOutput>, killed: Option<&Killed>, is_error: bool) -> String {
    todo!()
}

/// min(output_cap, window / 2) bytes.
pub fn cap(output_cap: usize, window: u64) -> usize {
    todo!()
}
