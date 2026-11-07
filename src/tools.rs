//! What the model sees before its first call (the core prompt and the tool schemas), and what decides a call before it
//! runs: validation, the target it would touch, and whether it needs a human.
use crate::{config, term, types::Call, types::End, types::ToolOutput};
use serde_json::{Value, json};
use std::{ffi::OsString, fs, io, os::unix::fs::FileTypeExt, os::unix::fs::MetadataExt, path::Path, path::PathBuf};

pub const ALL: [&str; 4] = ["shell", "read", "write", "edit"];
/// System part 1, byte-exact (563 B).
pub const PART1: &str = include_str!("prompt.txt");
pub const ARGS_MAX: usize = 1 << 20;
pub const READ_MAX: usize = 32 << 10;
pub const REPAIR: &str = "interrupted; the command may have partly run";

/// Per tool: the required string arguments, then the integer ones.
const ARGS: [(&str, &[&str], &[&str]); 4] =
    [("shell", &["command"], &["timeout"]), ("read", &["path"], &["offset", "limit"]), ("write", &["path", "content"], &[]), ("edit", &["path", "old_text", "new_text"], &[])];

/// `include_str!("tools.json")` (1,104 B for all four) filtered to `enabled`, canonical order.
pub fn schema(enabled: &[&'static str]) -> Value {
    let all: Vec<Value> = serde_json::from_str(include_str!("tools.json")).unwrap_or_default();
    all.into_iter().filter(|t| t["function"]["name"].as_str().is_some_and(|n| enabled.contains(&n))).collect()
}

/// Part 1, then "\n\n" and part 3 when it is not empty (today: the global AGENTS.md).
pub fn system(agents: Option<&str>) -> String {
    agents.filter(|a| !a.is_empty()).map_or_else(|| PART1.to_string(), |a| format!("{PART1}\n\n{a}"))
}

/// Sets `c.error` to the first failing validation string; normalizes integer fields in `c.args`.
pub fn validate(c: &mut Call, enabled: &[&'static str]) {
    if c.error.is_none() {
        c.error = check(c, enabled).err();
    }
}

fn check(c: &mut Call, enabled: &[&'static str]) -> Result<(), String> {
    let Some(&(_, strings, ints)) = ARGS.iter().find(|a| a.0 == c.name && enabled.contains(&a.0)) else {
        return Err(format!("unknown tool {}; available: {}", c.name, enabled.join(", ")));
    };
    // The size first: an argument string over the limit is never parsed.
    match c.raw.as_deref().map(|r| (r, (r.len() <= ARGS_MAX).then(|| serde_json::from_str::<Value>(r).err()).flatten())) {
        Some((r, _)) if r.len() > ARGS_MAX => return Err("arguments over 1 MiB; not run".into()),
        Some((r, Some(e))) => {
            return Err(format!("arguments are not valid JSON (line {} col {}): {}", e.line(), e.column(), r.get(..r.floor_char_boundary(200)).unwrap_or_default()));
        }
        _ => {}
    }
    let Some(args) = c.args.as_object_mut() else {
        return Err(format!("arguments must be a JSON object with: {}", strings.join(", ")));
    };
    if let Some(k) = strings.iter().find(|k| !args.get(**k).is_some_and(Value::is_string)) {
        return Err(format!("missing string argument \"{k}\""));
    }
    for &k in ints {
        let Some(v) = args.get(k).filter(|v| !v.is_null()) else { continue };
        let n = int(v).ok_or_else(|| format!("\"{k}\" must be an integer"))?.max(1);
        args.insert(k.into(), json!(if k == "timeout" { n.min(600) } else { n }));
    }
    Ok(())
}

/// An integer as models send it: a number without a fraction, or a string of digits. Negative numbers give 0.
fn int(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as u64),
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => Some(s.parse().unwrap_or(u64::MAX)),
        _ => None,
    }
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
    let fail = io_error(path);
    if write && matches!(path.rsplit('/').next(), Some("" | "." | "..")) {
        return Err(format!("{path}: not a file name"));
    }
    let given = cx.cwd.join(config::expand(path, cx.home));
    // The nearest path that exists (a dangling symlink does); below it, the components a write creates, deepest first.
    let (mut at, mut missing) = (given.as_path(), Vec::new());
    let found = loop {
        match fs::symlink_metadata(at) {
            Ok(m) => break m,
            Err(e) if write && e.kind() == io::ErrorKind::NotFound => {
                missing.push(at.file_name().ok_or_else(|| format!("{path}: a directory to create may not be . or .."))?.to_owned());
                at = at.parent().ok_or_else(|| fail(e))?;
            }
            Err(e) => return Err(fail(e)),
        }
    };
    let real = fs::canonicalize(at).map_err(|e| match fs::read_link(at) {
        Ok(to) if e.kind() == io::ErrorKind::NotFound => format!("dangling symlink: {path} -> {}", to.display()),
        _ => fail(e),
    })?;
    let mut resolved = real.clone();
    resolved.extend(missing.iter().rev());
    if let Some(d) = ["/proc", "/sys", "/dev"].into_iter().find(|d| resolved.starts_with(d)) {
        return Err(format!("refused: {} is under {d}", resolved.display()));
    }
    let meta = fs::metadata(&real).map_err(fail)?;
    if write && missing.is_empty() && !meta.is_file() {
        return Err(not_regular(path, meta.file_type()));
    }
    // Each protected path is resolved as a target is (realpath of its nearest existing ancestor, then the rest), so one
    // that does not exist yet still matches when it is named through a symlink.
    let resolve = |p: &Path| p.ancestors().find_map(|a| Some(fs::canonicalize(a).ok()?.join(p.strip_prefix(a).ok()?)));
    if write && cx.protected.iter().filter_map(|p| resolve(p)).any(|p| resolved.starts_with(p)) {
        return Err(format!("protected: {path} is henki's own file; change it through shell so the user sees it"));
    }
    let root = fs::canonicalize(cx.root).unwrap_or_else(|_| cx.root.to_path_buf());
    let outside = !resolved.starts_with(root);
    let (base, name, mode, missing) = match missing.split_first() {
        Some((name, dirs)) => (real, name.clone(), None, dirs.iter().rev().cloned().collect()),
        None => (real.parent().unwrap_or(&real).to_path_buf(), real.file_name().unwrap_or_default().to_owned(), Some(meta.mode()), Vec::new()),
    };
    let b = fs::metadata(&base).map_err(fail)?;
    let arrow = if resolved == given { String::new() } else { format!(" -> {}", resolved.display()) };
    let tags = [(found.is_symlink(), "symlink"), (outside, "outside project")].iter().filter(|t| t.0).map(|t| t.1).collect::<Vec<_>>().join(", ");
    let shown = if tags.is_empty() { format!("{path}{arrow}") } else { format!("{path}{arrow} ({tags})") };
    Ok(Target { path: resolved, shown, base, missing, name, dir_id: (b.dev(), b.ino()), mode, outside })
}

/// `not a regular file: PATH (fifo)`; the kind is one of directory, fifo, socket, char device, block device.
pub fn not_regular(path: &str, t: fs::FileType) -> String {
    let kinds = [(t.is_dir(), "directory"), (t.is_fifo(), "fifo"), (t.is_socket(), "socket"), (t.is_char_device(), "char device"), (t.is_block_device(), "block device")];
    format!("not a regular file: {path} ({})", kinds.iter().find(|k| k.0).map_or("unknown kind", |k| k.1))
}

/// `PATH: No such file or directory`: an OS error with the path it concerns, without `(os error 2)`.
pub fn io_error(path: &str) -> impl Fn(io::Error) -> String + Copy + '_ {
    move |e| format!("{path}: {}", e.to_string().split(" (os error").next().unwrap_or_default())
}

/// `ask` mode: shell, write and edit always; read only outside the project root. `auto`: never.
pub fn needs_approval(name: &str, t: Option<&Target>, auto: bool) -> bool {
    !auto && (name != "read" || t.is_none_or(|t| t.outside))
}

pub enum Killed {
    Timeout(u64),
    Interrupted,
}

/// A call's result text and `is_error`. Shell: the output or `(no output)`, then the footer. Files: the worker's text.
pub fn result_text(name: &str, out: &ToolOutput, killed: Option<&Killed>) -> (String, bool) {
    let err = killed.is_some() || !matches!(out.end, End::Exit(0) | End::File { failed: false });
    if name != "shell" {
        return (out.text.clone(), err);
    }
    let foot = format!("[{}{}]", ended(Some(out), killed, err), if killed.is_some() { "; process group killed" } else { "" });
    let text = match (out.text.as_str(), killed) {
        ("", Some(_)) => foot,
        ("", None) => format!("(no output)\n{foot}"),
        (t, _) => format!("{t}{}{foot}", if t.ends_with('\n') { "" } else { "\n" }),
    };
    (text, err)
}

/// How a call ended: `timed out after 120 s`, `interrupted`, `exit 1, 2.4 s`, `killed by signal 9, 2.4 s`, for a file
/// call `ok, 0.0 s` or `error, 0.0 s`, and for a call with no output to report (it never ran) `ok` or `error`.
fn ended(out: Option<&ToolOutput>, killed: Option<&Killed>, err: bool) -> String {
    let time = out.map_or(String::new(), |o| format!(", {} s", term::secs(o.ms)));
    match (killed, out.map(|o| o.end)) {
        (Some(Killed::Timeout(s)), _) => format!("timed out after {} s", term::num(*s)),
        (Some(Killed::Interrupted), _) => "interrupted".into(),
        (None, Some(End::Exit(c))) => format!("exit {c}{time}"),
        (None, Some(End::Signal(s))) => format!("killed by signal {s}{time}"),
        _ => format!("{}{time}", if err { "error" } else { "ok" }),
    }
}

/// `[not started: ENOENT bash]`.
pub fn not_started(e: &io::Error) -> String {
    const NAMES: [(i32, &str); 5] = [(libc::ENOENT, "ENOENT"), (libc::EACCES, "EACCES"), (libc::E2BIG, "E2BIG"), (libc::EAGAIN, "EAGAIN"), (libc::ENOMEM, "ENOMEM")];
    let name = NAMES.iter().find(|n| e.raw_os_error() == Some(n.0)).map_or_else(|| e.to_string(), |n| n.1.to_string());
    format!("[not started: {name} bash]")
}

/// The plain-mode stderr line: `$ FIRST_LINE -> exit 0, 0.3 s`, `read PATH -> ok, 0.0 s`, `edit PATH -> error, 0.0 s`.
pub fn call_line(c: &Call, out: Option<&ToolOutput>, killed: Option<&Killed>, is_error: bool) -> String {
    let head = match (c.name.as_str(), c.args["command"].as_str(), c.args["path"].as_str()) {
        ("shell", Some(cmd), _) => format!("$ {}", cmd.lines().next().unwrap_or_default()),
        (name, _, Some(p)) => format!("{name} {p}"),
        (name, ..) => name.to_string(),
    };
    format!("{head} -> {}", ended(out, killed, is_error))
}

/// min(output_cap, window / 2) bytes.
pub fn cap(output_cap: usize, window: u64) -> usize {
    output_cap.min(usize::try_from(window / 2).unwrap_or(usize::MAX))
}

#[cfg(test)]
mod tests;
