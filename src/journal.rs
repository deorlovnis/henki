//! The session journal: one JSON record per line, appended by the main thread alone, byte-counted and synced before the
//! effect it explains; resume reads it back, repairs dangling calls by appending, and checks the call/result pairing.
use crate::{config::Config, supervise::Spill, sys, term, tools, types::Outcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::{collections::HashSet, fmt::Display, fs, fs::File, io::Read, io::Write, os::unix::ffi::OsStrExt, time::Duration, time::SystemTime};

pub struct Journal {
    file: File,
    path: PathBuf,
    bytes: u64,
    id: String,
    spill: PathBuf,
    n: usize,
    fresh: bool, // created by this process, so `discard` may remove it
    gone: bool,  // discarded: nothing more is written
}

pub struct Resumed {
    pub journal: Journal,
    pub hist: Vec<Value>,
    pub from_bytes: u64,
    pub notices: Vec<String>,
    /// The repair records as written, also at the end of `hist`; `--json` stdout shows them before `session`.
    pub repairs: Vec<Value>,
}

const ELIDED: &str = "[output elided: ";

/// Creates the state dir `dir` with `sessions/` and `spill/` through `sys::private_dir`, and deletes spill files older
/// than 24 h.
pub fn state_dir(dir: &Path) -> Result<(), String> {
    for d in [dir.to_path_buf(), dir.join("sessions"), dir.join("spill")] {
        sys::private_dir(&d).map_err(|e| format!("state dir {}: {e}; fix: make it a directory you own, or set XDG_STATE_HOME", d.display()))?;
    }
    let old = |e: &fs::DirEntry| e.metadata().ok().and_then(|m| m.modified().ok()?.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(86_400));
    fs::read_dir(dir.join("spill")).into_iter().flatten().flatten().filter(old).for_each(|e| _ = fs::remove_file(e.path()));
    Ok(())
}

fn jerr(path: &Path, e: impl Display) -> Outcome {
    Outcome::error("journal", format!("{}: {e}", path.display()))
}

fn tampered(path: &Path) -> Outcome {
    Outcome::error("tampered", format!("journal modified outside henki: {}", path.display()))
}

/// The sessions subdirectory of a cwd: FNV-1a of its realpath.
fn scope(cwd: &Path) -> Result<String, Outcome> {
    Ok(sys::fnv(fs::canonicalize(cwd).map_err(|e| jerr(cwd, e))?.as_os_str().as_bytes()))
}

impl Journal {
    /// `sessions/<fnv(realpath(cwd))>/<id>.jsonl`: `sys::create_excl(path, 0o600, true)` (O_APPEND), `try_lock` held
    /// for the session.
    pub fn create(state: &Path, cwd: &Path) -> Result<Journal, Outcome> {
        let dir = state.join("sessions").join(scope(cwd)?);
        sys::private_dir(&dir).map_err(|e| jerr(&dir, e))?;
        let id = format!("{}-{}", sys::utc().1, sys::random4().map_err(|e| jerr(Path::new("/dev/urandom"), e))?);
        Journal::start(dir.join(format!("{id}.jsonl")), id, state)
    }

    fn start(path: PathBuf, id: String, state: &Path) -> Result<Journal, Outcome> {
        let file = sys::create_excl(&path, 0o600, true).map_err(|e| jerr(&path, e))?;
        file.try_lock().map_err(|e| jerr(&path, e))?;
        Ok(Journal { file, path, bytes: 0, id, spill: state.join("spill"), n: 0, fresh: true, gone: false })
    }

    /// Resume through one `sys::open_append` fd: pick, open, lock, read, truncate a torn tail (`set_len`), parse, check,
    /// rebuild, repair (appended through the same O_APPEND fd, synced).
    pub fn resume(state: &Path, cwd: &Path, id: Option<&str>, cfg: &Config) -> Result<Resumed, Outcome> {
        let (path, file) = pick(state, cwd, id)?;
        let mut buf = Vec::new();
        (&file).read_to_end(&mut buf).map_err(|e| jerr(&path, e))?;
        let mut notices = Vec::new();
        let keep = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        if keep < buf.len() {
            file.set_len(keep as u64).map_err(|e| jerr(&path, e))?;
            notices.push(format!("dropped an incomplete last line ({} bytes) from {}", term::num((buf.len() - keep) as u64), path.display()));
            buf.truncate(keep);
        }
        let mut hist = replay(&buf, &path, cfg, cwd, &mut notices)?;
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let prefix = format!("{id}-");
        let spill = state.join("spill");
        let names = fs::read_dir(&spill).into_iter().flatten().flatten().map(|e| e.file_name());
        // A name whose file the 24 h sweep removed may still be named in the history: each call took at most one name.
        let n = names.filter_map(|f| f.to_str()?.strip_prefix(&prefix)?.strip_suffix(".log")?.parse().ok()).max().unwrap_or(0).max(calls(&hist).count());
        let mut journal = Journal { file, path, bytes: keep as u64, id, spill, n, fresh: false, gone: false };
        let repairs = dangling(&hist).into_iter().map(|(id, name)| journal.write(rec::tool_result(&id, &name, tools::REPAIR, true, 0, 0)));
        let repairs = repairs.collect::<Result<Vec<_>, _>>()?;
        hist.extend(repairs.iter().cloned());
        Ok(Resumed { journal, hist, from_bytes: keep as u64, notices, repairs })
    }

    /// Adds `ts`; checks fstat size = bytes written (else `tampered`); appends `term::json_line` + "\n" on the O_APPEND
    /// fd; fdatasync unless the kind is `ask`, `approval` or `notice`. Returns the record as written (after `discard`,
    /// as it would have been).
    pub fn write(&mut self, mut rec: Value) -> Result<Value, Outcome> {
        rec.as_object_mut().map(|o| o.insert("ts".into(), sys::utc().0.into()));
        if self.gone {
            return Ok(rec);
        }
        let line = term::json_line(&rec) + "\n";
        let fail = |e: std::io::Error| Outcome::error("journal", format!("cannot write {}: {e}", self.path.display()));
        if self.file.metadata().map_err(fail)?.len() != self.bytes {
            return Err(tampered(&self.path));
        }
        // One write(2), and a second only after a short one (whose error then names the cause, e.g. a full disk). A
        // failure leaves `bytes` behind the file, so every later write fails the size check instead of appending.
        (&self.file).write_all(line.as_bytes()).map_err(fail)?;
        self.bytes += line.len() as u64;
        // `ask`, `approval` and `notice` explain no effect that a crash could leave behind.
        if !matches!(rec["t"].as_str(), Some("ask" | "approval" | "notice")) {
            self.file.sync_data().map_err(fail)?;
            #[cfg(test)]
            tests::SYNCS.with(|n| n.set(n.get() + 1));
        }
        Ok(rec)
    }

    /// The REPL ended before its first message: removes the session file this process created (never a resumed one).
    /// True when it was removed.
    pub fn discard(&mut self) -> bool {
        self.gone |= std::mem::take(&mut self.fresh) && fs::remove_file(&self.path).is_ok();
        self.gone
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The next spill name, `<id>-<n>.log` (n from the counter, never from a model string). One per tool call at most:
    /// resume continues the counter past both the names on disk and the number of calls in the history.
    pub fn spill(&mut self, home: &Path) -> Spill {
        self.n = self.n.saturating_add(1);
        let path = self.spill.join(format!("{}-{}.log", self.id, self.n));
        let shown = path.strip_prefix(home).map_or_else(|_| path.display().to_string(), |rest| format!("~/{}", rest.display()));
        Spill { path, shown }
    }
}

/// The session to resume, opened and locked: `id` in any scope, or the newest unlocked one in this cwd's scope. Empty
/// files (a run that died before its first record) are not sessions.
fn pick(state: &Path, cwd: &Path, id: Option<&str>) -> Result<(PathBuf, File), Outcome> {
    let ls = |d: PathBuf| fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path());
    let sessions = state.join("sessions");
    let paths: Vec<PathBuf> = match id {
        Some(id) => ls(sessions).map(|d| d.join(format!("{id}.jsonl"))).filter(|_| !id.contains('/')).collect(),
        None => ls(sessions.join(scope(cwd)?)).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect(),
    };
    let mtime = |p: &Path| fs::symlink_metadata(p).ok().filter(|m| m.is_file() && m.len() > 0)?.modified().ok();
    let mut found: Vec<(SystemTime, PathBuf)> = paths.into_iter().filter_map(|p| Some((mtime(&p)?, p))).collect();
    found.sort();
    let mut busy = None;
    for (_, path) in found.into_iter().rev() {
        let file = sys::open_append(&path).map_err(|e| jerr(&path, e))?;
        if file.try_lock().is_ok() {
            return Ok((path, file));
        }
        busy.get_or_insert(path);
    }
    Err(match busy.as_ref().and_then(|p| p.file_stem()) {
        Some(id) => Outcome::error("journal", format!("session {} is in use", id.to_string_lossy())),
        None => Outcome::error("journal", format!("no session to resume in {}", cwd.display())),
    })
}

/// Over whole lines: parse each, check the segment after the last `session` record, the dialect and what changed since,
/// and rebuild the history from the message kinds with every `elide` applied.
fn replay(buf: &[u8], path: &Path, cfg: &Config, cwd: &Path, notices: &mut Vec<String>) -> Result<Vec<Value>, Outcome> {
    let (mut recs, mut off) = (Vec::new(), 0);
    for (i, line) in buf.split_inclusive(|&b| b == b'\n').enumerate() {
        match serde_json::from_slice::<Value>(line) {
            Ok(v) if v.is_object() => recs.push((off, v)),
            _ => return Err(Outcome::error("journal", format!("corrupt journal {}:{}", path.display(), i + 1))),
        }
        off += line.len();
    }
    let Some(last) = recs.iter().rposition(|(_, r)| r["t"] == "session") else { return Err(jerr(path, "no session record")) };
    let seg = &recs[last..];
    match seg.iter().position(|(_, r)| r["t"] == "close") {
        None => notices.push("previous run did not close cleanly".into()),
        Some(i) if i + 1 == seg.len() && seg[i].1["bytes"].as_u64() == Some(seg[i].0 as u64) => {}
        Some(_) => return Err(tampered(path)),
    }
    let s = &recs[last].1;
    if s["dialect"] != cfg.dialect {
        let was = s["dialect"].as_str().unwrap_or("unknown");
        return Err(Outcome::error("journal", format!("{}: the session's dialect is {was}, not {}", path.display(), cfg.dialect)));
    }
    for (k, now) in [("model", cfg.model.as_str().into()), ("tools", Value::from(cfg.tools.clone())), ("cwd", cwd.to_string_lossy().into())] {
        if s[k] != now {
            notices.push(format!("{k} changed since the session started: {} -> {now}", s[k]));
        }
    }
    let mut hist: Vec<Value> = Vec::new();
    for (_, r) in recs {
        match r["t"].as_str() {
            Some("user" | "assistant" | "tool_result") => hist.push(r),
            Some("elide") => hist.iter_mut().filter(|h| h["t"] == "tool_result" && r["ids"].as_array().is_some_and(|ids| ids.contains(&h["id"]))).for_each(|h| _ = shrink(h)),
            _ => {}
        }
    }
    Ok(hist)
}

/// Every call of every assistant record, in history order.
fn calls(hist: &[Value]) -> impl Iterator<Item = &Value> {
    hist.iter().filter(|r| r["t"] == "assistant").flat_map(|r| r["msg"]["tool_calls"].as_array().into_iter().flatten())
}

/// (id, name) of every call without a result, in history order.
fn dangling(hist: &[Value]) -> Vec<(String, String)> {
    let done: HashSet<&str> = hist.iter().filter(|r| r["t"] == "tool_result").filter_map(|r| r["id"].as_str()).collect();
    calls(hist).filter_map(|c| Some((c["id"].as_str().filter(|id| !done.contains(id))?.to_string(), c["name"].as_str().unwrap_or_default().to_string()))).collect()
}

/// Over `user`, `assistant` and `tool_result` records only (every other kind is skipped, so this also checks a whole
/// journal file): call ids unique; each call has exactly one result after its assistant record and before the next
/// `assistant` or `user` record; no result without an open call.
pub fn pairing(hist: &[Value]) -> Result<(), String> {
    let (mut seen, mut open) = (HashSet::new(), Vec::new());
    for r in hist {
        let t = r["t"].as_str().unwrap_or_default();
        if t == "user" || t == "assistant" {
            open.first().map_or(Ok(()), |id| Err(format!("call {id} has no result before the next {t} record")))?;
            for c in r["msg"]["tool_calls"].as_array().into_iter().flatten() {
                let id = c["id"].to_string();
                seen.insert(id.clone()).then_some(()).ok_or_else(|| format!("call id {id} is used twice"))?;
                open.push(id);
            }
        } else if t == "tool_result" {
            let id = r["id"].to_string();
            open.remove(open.iter().position(|o| *o == id).ok_or_else(|| format!("result {id} has no open call"))?);
        }
    }
    open.first().map_or(Ok(()), |id| Err(format!("call {id} has no result")))
}

/// `[output elided: N bytes]` in place of a result's content (N = its length), unless it is that marker already.
fn shrink(r: &mut Value) -> bool {
    let marker = |c: &str| c.strip_prefix(ELIDED).and_then(|n| n.strip_suffix(" bytes]")).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit() || b == b','));
    let Some(c) = r["content"].as_str().filter(|c| !marker(c)) else { return false };
    r["content"] = format!("{ELIDED}{} bytes]", term::num(c.len() as u64)).into();
    true
}

/// Every tool_result older than the last two assistant records and not yet elided gets `[output elided: N bytes]`;
/// returns their ids (empty: nothing to elide).
pub fn elide(hist: &mut [Value]) -> Vec<String> {
    let Some(cut) = hist.iter().rposition(|r| r["t"] == "assistant").and_then(|i| hist[..i].iter().rposition(|r| r["t"] == "assistant")) else { return Vec::new() };
    let olds = hist[..cut].iter_mut().filter(|r| r["t"] == "tool_result");
    olds.filter_map(|r| shrink(r).then(|| r["id"].as_str().unwrap_or_default().to_string())).collect()
}

/// 1 + the highest `h<N>` call id in `hist` (1 when none).
pub fn next_h(hist: &[Value]) -> u32 {
    calls(hist).filter_map(|c| c["id"].as_str()?.strip_prefix('h')?.parse::<u32>().ok()).max().map_or(1, |n| n.saturating_add(1))
}

/// One builder per record kind, so the module reads as the record table; each sets "t"; `write` adds "ts".
pub mod rec {
    use crate::{config::Config, types::Call, types::Outcome, types::Usage};
    use serde_json::{Value, json};
    use std::path::Path;

    pub fn session(cfg: &Config, id: &str, cwd: &Path, resumed: Option<u64>) -> Value {
        let u = &cfg.url;
        json!({"t": "session", "v": 1, "henki": env!("CARGO_PKG_VERSION"), "id": id, "cwd": cwd.to_string_lossy(), "model": cfg.model,
            "dialect": cfg.dialect, "base_url": format!("http://{}{}", crate::http::host_port(u), u.path), "base_url_from": cfg.url_from,
            "approve": if cfg.auto { "auto" } else { "ask" }, "approve_from": cfg.approve_from, "config": cfg.file.as_ref().map(|f| f.to_string_lossy()),
            "tools": cfg.tools, "shell": cfg.bash.to_string_lossy(), "exts": cfg.exts.iter().map(|e| &e.name).collect::<Vec<_>>(), "pass_env": cfg.pass_env, "ctx": cfg.window,
            "config_hash": cfg.hash, "allow_hash": cfg.allow.hash, "agents_hash": cfg.agents_hash, "project_agents_hash": null,
            "resumed": resumed.map(|b| json!({"from_bytes": b}))})
    }
    pub fn user(content: &str) -> Value {
        json!({"t": "user", "msg": {"content": content, "role": "user"}})
    }
    /// "text" | "reasoning"; stdout only.
    pub fn delta(reasoning: bool, text: &str) -> Value {
        json!({"t": if reasoning { "reasoning" } else { "text" }, "delta": text})
    }
    /// `blocks`: the reply's content blocks as received (Anthropic), replayed as they are.
    pub fn assistant(text: &str, calls: &[Call], finish: &str, u: Usage, blocks: Option<&Value>) -> Value {
        let calls: Vec<Value> = calls.iter().map(|c| with(json!({"args": c.args, "id": c.id, "name": c.name}), "raw", c.raw.as_deref())).collect();
        with(json!({"t": "assistant", "msg": {"content": text, "role": "assistant", "tool_calls": calls}, "finish": finish, "usage": usage(u)}), "blocks", blocks.cloned())
    }
    pub fn ask(c: &Call, target: Option<&str>) -> Value {
        with(json!({"t": "ask", "id": c.id, "name": c.name, "args": c.args}), "target", target)
    }
    /// `verdict`: yes, no, always, rule or auto; `rule`: the allow rule's number.
    pub fn approval(id: &str, verdict: &str, rule: Option<usize>) -> Value {
        with(json!({"t": "approval", "id": id, "verdict": verdict}), "rule", rule)
    }
    pub fn tool_result(id: &str, name: &str, content: &str, is_error: bool, ms: u64, bytes: u64) -> Value {
        json!({"t": "tool_result", "id": id, "name": name, "content": content, "is_error": is_error, "ms": ms, "bytes": bytes})
    }
    pub fn notice(text: &str) -> Value {
        json!({"t": "notice", "text": text})
    }
    pub fn elide(ids: &[String]) -> Value {
        json!({"t": "elide", "ids": ids})
    }
    pub fn turn_end(o: &Outcome, steps: u32, u: Usage) -> Value {
        with(json!({"t": "turn_end", "status": o.status(), "steps": steps, "usage": usage(u)}), "reason", o.reason())
    }
    pub fn result(o: &Outcome, text: &str, u: Usage, session: Option<&str>, steps: u32, duration_ms: u64) -> Value {
        let v = json!({"t": "result", "status": o.status(), "exit": o.exit(), "text": text, "usage": usage(u), "session": session, "steps": steps,
            "duration_s": (duration_ms.saturating_add(50) / 100) as f64 / 10.0});
        let mut v = with(v, "reason", o.reason());
        match o {
            Outcome::Error { kind, message } => v["error"] = json!({"kind": kind, "message": message}),
            Outcome::NeedsApproval(c) => v["call"] = json!({"args": c.args, "id": c.id, "name": c.name}),
            _ => {}
        }
        v
    }
    pub fn close(bytes: u64) -> Value {
        json!({"t": "close", "bytes": bytes})
    }

    fn usage(u: Usage) -> Value {
        json!({"cached": u.cached, "in": u.input, "out": u.output})
    }
    /// `v` with the optional field `k` when it is given.
    fn with(mut v: Value, k: &str, x: Option<impl Into<Value>>) -> Value {
        if let Some(x) = x {
            v[k] = x.into();
        }
        v
    }
}

#[cfg(test)]
mod tests;
