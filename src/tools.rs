//! What the model sees before its first call (the core prompt and the tool schemas), and what decides a call before it
//! runs: validation, the target it would touch, whether it needs a human, and what that human is shown.
use crate::{config, config::Config, sys, term, types::ARGS_MAX, types::Call, types::End, types::ToolOutput};
use serde_json::{Value, json};
use std::{ffi::OsString, fmt::Write, fs, io, os::unix::fs::MetadataExt, path::Path, path::PathBuf};

/// System part 1, byte-exact (563 B).
pub const PART1: &str = include_str!("prompt.txt");
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

/// Part 1, then "\n\n" and part 3 when it is not empty; part 3 = the non-empty of the global AGENTS.md and the
/// extensions item (`exts`), joined by "\n\n".
pub fn system(agents: Option<&str>, exts: &str) -> String {
    [PART1, agents.unwrap_or_default(), exts].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
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

pub struct Target {
    pub path: PathBuf,          // resolved
    pub shown: String,          // `PATH`, or `PATH -> TARGET (symlink, outside project)` as they apply
    pub base: PathBuf,          // the nearest existing directory
    pub missing: Vec<OsString>, // components to create below `base` (write)
    pub name: OsString,         // the last component
    pub dir_id: (u64, u64),     // (dev, ino) of `base`
    pub mode: Option<u32>,      // an existing file's mode
    pub outside: bool,          // outside the project root
    pub prot: bool,             // under a protected path: a write is refused here, and a policy binding cannot allow a read
}

/// Resolve a file tool's path before anything is shown or run: against `cfg.cwd`, with `~/` as `cfg.home`, `prot` under
/// `cfg.protected` (a write there is refused), `outside` of `cfg.root`.
pub fn target(path: &str, write: bool, cfg: &Config) -> Result<Target, String> {
    let fail = io_error(path);
    if write && matches!(path.rsplit('/').next(), Some("" | "." | "..")) {
        return Err(format!("{path}: not a file name"));
    }
    let given = cfg.cwd.join(config::expand(path, &cfg.home));
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
        return Err(sys::not_regular(path, meta.file_type()));
    }
    // Each protected path is resolved as a target is (realpath of its nearest existing ancestor, then the rest), so one
    // that does not exist yet still matches when it is named through a symlink.
    let resolve = |p: &Path| p.ancestors().find_map(|a| Some(fs::canonicalize(a).ok()?.join(p.strip_prefix(a).ok()?)));
    let prot = cfg.protected.iter().filter_map(|p| resolve(p)).any(|p| resolved.starts_with(p));
    if write && prot {
        return Err(format!("protected: {path} is henki's own file; change it through shell so the user sees it"));
    }
    let root = fs::canonicalize(&cfg.root).unwrap_or_else(|_| cfg.root.clone());
    let outside = !resolved.starts_with(root);
    let (base, name, mode, missing) = match missing.split_first() {
        Some((name, dirs)) => (real, name.clone(), None, dirs.iter().rev().cloned().collect()),
        None => (real.parent().unwrap_or(&real).to_path_buf(), real.file_name().unwrap_or_default().to_owned(), Some(meta.mode()), Vec::new()),
    };
    let b = fs::metadata(&base).map_err(fail)?;
    let arrow = if resolved == given { String::new() } else { format!(" -> {}", resolved.display()) };
    let tags = [(found.is_symlink(), "symlink"), (outside, "outside project")].iter().filter(|t| t.0).map(|t| t.1).collect::<Vec<_>>().join(", ");
    let shown = if tags.is_empty() { format!("{path}{arrow}") } else { format!("{path}{arrow} ({tags})") };
    Ok(Target { path: resolved, shown, base, missing, name, dir_id: (b.dev(), b.ino()), mode, outside, prot })
}

/// `PATH: No such file or directory`: an OS error with the path it concerns, without `(os error 2)`.
pub fn io_error(path: &str) -> impl Fn(io::Error) -> String + Copy + '_ {
    move |e| format!("{path}: {}", e.to_string().split(" (os error").next().unwrap_or_default())
}

/// `ask` mode: shell, write and edit always; read only outside the project root, except the files henki points the
/// model at (`pointed`). `auto`: never.
pub fn needs_approval(name: &str, t: Option<&Target>, cfg: &Config, session: &str) -> bool {
    !cfg.auto && (name != "read" || !t.is_some_and(|t| !t.outside || pointed(t, cfg, session)))
}

/// henki's own files the model is pointed at: an extension's `doc.md` and this session's spill files
/// (`<state>/spill/<session>-N.log`).
pub fn pointed(t: &Target, cfg: &Config, session: &str) -> bool {
    let real = |p: &Path| fs::canonicalize(p).ok();
    let n = t.name.to_str().and_then(|n| n.strip_prefix(session)?.strip_prefix('-')?.strip_suffix(".log"));
    let spill = n.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) && t.path.parent() == real(&cfg.state.join("spill")).as_deref();
    spill || cfg.exts.iter().any(|e| real(&e.doc).as_ref() == Some(&t.path))
}

/// The approval display, without the reset and the question: a header rule (the tool, a file tool's target, and for a
/// write whether it replaces a file), every line of each primary argument under its own rule, never truncated, then the
/// other arguments as rules, sorted by key. A line is marked `!| ` instead of `| ` when it shows anything other than
/// itself: an escaped or non-ASCII code point, a tab (as `\t`) or a run of more than 32 spaces (as `[N spaces]`).
pub fn display(c: &Call, t: Option<&Target>) -> Vec<String> {
    // One row each, so a newline or a tab in a path or a key is escaped too.
    let rule = |text: &str| {
        let text = term::escape(text).replace('\n', "\\x0a").replace('\t', "\\x09");
        format!("-- {text} {}", "-".repeat(72usize.saturating_sub(text.chars().count() + 4).max(2)))
    };
    let (strings, ints) = ARGS.iter().find(|a| a.0 == c.name).map_or((&[][..], &[][..]), |a| (a.1, a.2));
    // A write over a file that exists says so: whatever the new content leaves out is gone.
    let over = if c.name == "write" && t.is_some_and(|t| t.mode.is_some()) { " (replaces an existing file)" } else { "" };
    let mut lines = vec![rule(&t.map_or_else(|| c.name.clone(), |t| format!("{} {}{over}", c.name, t.shown)))];
    for &k in strings.iter().filter(|&&k| k != "path") {
        let v = c.args[k].as_str().unwrap_or_default();
        let n = v.split_terminator('\n').count() as u64;
        lines.push(rule(&format!("{k} ({} lines, {} bytes)", term::num(n), term::num(v.len() as u64))));
        lines.extend(v.split_terminator('\n').map(gutter));
    }
    for (k, v) in c.args.as_object().into_iter().flatten().filter(|(k, _)| !strings.contains(&k.as_str())) {
        let ignored = if ints.contains(&k.as_str()) { "" } else { "ignored: " };
        lines.push(rule(&format!("{ignored}{k}: {}", term::json_line(v))));
    }
    lines
}

/// One line of an argument with its gutter.
fn gutter(line: &str) -> String {
    let (mut o, mut rest, run) = (String::with_capacity(line.len()), line, " ".repeat(33));
    while let Some((head, spaces)) = rest.find(&run).map(|i| rest.split_at(i)) {
        rest = spaces.trim_start_matches(' ');
        let _ = write!(o, "{head}[{} spaces]", term::num((spaces.len() - rest.len()) as u64));
    }
    o.push_str(rest);
    // A collapsed run always shortens the line.
    let mark = o != line || line.contains(|c: char| c == '\t' || !c.is_ascii() || term::escaped(c));
    format!("{}{}", if mark { "!| " } else { "| " }, term::escape(&o.replace('\t', "\\t")))
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

/// The plain-mode stderr line: `$ FIRST_LINE -> exit 0, 0.3 s`, `read PATH -> ok, 0.0 s`, `edit PATH -> error, 0.0 s`;
/// a shell call that printed nothing adds `, no output`, so what the model says about its output can be checked.
pub fn call_line(c: &Call, out: Option<&ToolOutput>, killed: Option<&Killed>, is_error: bool) -> String {
    let silent = if c.name == "shell" && killed.is_none() && out.is_some_and(|o| o.bytes == 0) { ", no output" } else { "" };
    format!("{} -> {}{silent}", head(c), ended(out, killed, is_error))
}

/// A call's head: `$ FIRST_LINE` (shell), `read PATH`, `write PATH`, `edit PATH`, else the name. One row: a newline in a path
/// or in a name the model chose shows as `\x0a`, so neither can start a row that looks like henki's own.
pub fn head(c: &Call) -> String {
    let head = match (c.name.as_str(), c.args["command"].as_str(), c.args["path"].as_str()) {
        ("shell", Some(cmd), _) => format!("$ {}", cmd.lines().next().unwrap_or_default()),
        (name @ ("read" | "write" | "edit"), _, Some(p)) => format!("{name} {p}"),
        (name, ..) => name.to_string(),
    };
    head.replace('\n', "\\x0a")
}

/// min(output_cap, window / 2) bytes.
pub fn cap(output_cap: usize, window: u64) -> usize {
    output_cap.min(usize::try_from(window / 2).unwrap_or(usize::MAX))
}

#[cfg(test)]
mod tests;
