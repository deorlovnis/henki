//! The session journal: one JSON record per line. Its std binding (`Journal`) keeps a session in one file, appended by
//! the main thread alone, byte-counted and synced before the effect it explains. The kernel half (`start`, `put`)
//! reads it back through any history binding, replays it, repairs dangling calls by appending, and checks the
//! call/result pairing.
use crate::{allow, config::Config, port, port::History, port::PortError, port::Record, port::Taken, supervise::Spill, sys, term, tools, types::Outcome};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::{collections::HashSet, fmt::Display, fs, fs::File, io::Read, io::Seek, io::Write, os::unix::ffi::OsStrExt, os::unix::fs::MetadataExt, time::Duration, time::SystemTime};

/// The std history binding: `STATE/sessions/<fnv(realpath(scope))>/<id>.jsonl`, at most one of them held.
pub struct Journal {
    state: PathBuf,
    held: Option<Held>,
}

/// The held session: its file, opened O_APPEND and locked, and the bytes written to it.
struct Held {
    file: File,
    path: PathBuf,
    id: String,
    bytes: u64,
    from: u64,   // its length after the last `read` (a torn tail cut): a resumed `session` record's `from_bytes`
    fresh: bool, // created by this process, so `discard` may remove it
    gone: bool,  // discarded: nothing more is written
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

fn io(path: &Path, e: impl Display) -> PortError {
    port::err("io", format!("{}: {e}", path.display()))
}

fn tampered(place: impl Display) -> PortError {
    port::err("tampered", format!("journal modified outside henki: {place}"))
}

/// The sessions subdirectory of a cwd: FNV-1a of its realpath.
fn scope(cwd: &Path) -> Result<String, PortError> {
    Ok(sys::fnv(fs::canonicalize(cwd).map_err(|e| io(cwd, e))?.as_os_str().as_bytes()))
}

/// The paths in a directory (none when it cannot be read).
fn ls(d: PathBuf) -> impl Iterator<Item = PathBuf> {
    fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path())
}

/// Session `id`'s file in every scope where one exists; none for an id holding `/`.
fn named(state: &Path, id: &str) -> Vec<PathBuf> {
    ls(state.join("sessions")).map(|d| d.join(format!("{id}.jsonl"))).filter(|p| !id.contains('/') && fs::symlink_metadata(p).is_ok()).collect()
}

/// The length of the whole lines at the start of `b`.
fn whole(b: &[u8]) -> usize {
    b.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1)
}

impl Journal {
    pub fn new(state: &Path) -> Journal {
        Journal { state: state.into(), held: None }
    }

    /// `path`, created as session `id` and held: `sys::create_excl(path, 0o600, true)` (O_APPEND), `try_lock` held until
    /// the binding drops it.
    fn start(&mut self, path: PathBuf, id: String) -> Result<String, PortError> {
        let file = sys::create_excl(&path, 0o600, true).map_err(|e| io(&path, e))?;
        file.try_lock().map_err(|e| io(&path, e))?;
        self.held = Some(Held { file, path, id: id.clone(), bytes: 0, from: 0, fresh: true, gone: false });
        Ok(id)
    }

    /// The held session `id`; `not_found` for any other.
    fn held(&mut self, id: &str) -> Result<&mut Held, PortError> {
        self.held.as_mut().filter(|h| h.id == id).ok_or_else(|| port::err("not_found", format!("session {id} is not held")))
    }
}

impl History for Journal {
    /// The id is UTC time and 4 random bytes, or a `hint` that `port::session_id` accepts and no scope has yet.
    fn create(&mut self, cwd: &Path, hint: Option<&str>) -> Result<String, PortError> {
        let dir = self.state.join("sessions").join(scope(cwd)?);
        sys::private_dir(&dir).map_err(|e| io(&dir, e))?;
        let id = match hint.filter(|h| port::session_id(h)) {
            Some(h) if !named(&self.state, h).is_empty() => return Err(port::err("exists", format!("session {h} exists"))),
            Some(h) => h.to_string(),
            None => format!("{}-{}", sys::utc().1, sys::random4().map_err(|e| io(Path::new("/dev/urandom"), e))?),
        };
        self.start(dir.join(format!("{id}.jsonl")), id)
    }

    /// Through one `sys::open_append` fd, locked; `read` then reads through it (and cuts a torn tail).
    fn open(&mut self, scope: &Path, session: Option<&str>) -> Result<String, PortError> {
        self.held = None; // its lock would make it busy
        let (path, file) = pick(&self.state, scope, session)?;
        let (id, n) = (path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), file.metadata().map_err(|e| io(&path, e))?.len());
        self.held = Some(Held { file, path, id: id.clone(), bytes: n, from: n, fresh: false, gone: false });
        Ok(id)
    }

    /// Held: through its fd, a torn tail cut (`set_len`) with a notice, so the next record starts a line. Unheld: read
    /// only, a torn tail skipped. Every line is an object (else `corrupt`), and every `close` gives its own offset, the
    /// file's length when it was written (else `tampered`).
    fn read(&mut self, session: &str) -> Result<Taken, PortError> {
        let (mut buf, mut notices) = (Vec::new(), Vec::new());
        let path = match self.held.as_mut().filter(|h| h.id == session && !h.gone) {
            Some(h) => {
                (&h.file).rewind().and_then(|()| (&h.file).read_to_end(&mut buf)).map_err(|e| io(&h.path, e))?;
                let keep = whole(&buf);
                if keep < buf.len() {
                    h.file.set_len(keep as u64).map_err(|e| io(&h.path, e))?;
                    notices.push(format!("dropped an incomplete last line ({} bytes) from {}", term::num((buf.len() - keep) as u64), h.path.display()));
                }
                (h.bytes, h.from) = (keep as u64, keep as u64);
                h.path.clone()
            }
            None => {
                let path = named(&self.state, session).into_iter().next().ok_or_else(|| port::err("not_found", format!("no session {session}")))?;
                sys::open_read(&path).and_then(|(mut f, _)| f.read_to_end(&mut buf)).map_err(|e| io(&path, e))?;
                path
            }
        };
        buf.truncate(whole(&buf));
        let (mut records, mut off) = (Vec::new(), 0);
        for (i, line) in buf.split_inclusive(|&b| b == b'\n').enumerate() {
            match serde_json::from_slice::<Value>(line) {
                Ok(v) if v["t"] == "close" && v["bytes"].as_u64() != Some(off) => return Err(tampered(path.display())),
                Ok(v) if v.is_object() => records.push(v),
                _ => return Err(port::err("corrupt", format!("corrupt journal {}:{}", path.display(), i + 1))),
            }
            off += line.len() as u64;
        }
        Ok(Taken { records, notices, place: path.display().to_string() })
    }

    /// Fills `close.bytes` and a resumed `session`'s `from_bytes`; checks fstat size = bytes written and that the path
    /// still names the file (else `tampered`); appends `term::json_line` + "\n" on the O_APPEND fd; fdatasync unless the
    /// kind is `ask`, `approval` or `notice`. After `discard`, returns the record as it would have been.
    fn append(&mut self, session: &str, mut rec: Record) -> Result<Record, PortError> {
        let h = self.held(session)?;
        rec.as_object().ok_or_else(|| port::err("invalid", "a record is a JSON object"))?;
        if rec["t"] == "close" {
            rec["bytes"] = h.bytes.into();
        }
        if rec["t"] == "session" && rec["resumed"].is_object() {
            rec["resumed"]["from_bytes"] = h.from.into();
        }
        if h.gone {
            return Ok(rec);
        }
        let line = term::json_line(&rec) + "\n";
        let fail = |e: std::io::Error| port::err("io", format!("cannot write {}: {e}", h.path.display()));
        // The path must still name the file held open: an edit that writes a new file in its place (`sed -i`) keeps the size.
        let m = h.file.metadata().map_err(fail)?;
        if m.len() != h.bytes || fs::symlink_metadata(&h.path).map_or(true, |p| (p.dev(), p.ino()) != (m.dev(), m.ino())) {
            return Err(tampered(h.path.display()));
        }
        // One write(2), and a second only after a short one (whose error then names the cause, e.g. a full disk). A
        // failure leaves `bytes` behind the file, so every later write fails the size check instead of appending.
        (&h.file).write_all(line.as_bytes()).map_err(fail)?;
        h.bytes += line.len() as u64;
        // `ask`, `approval` and `notice` explain no effect that a crash could leave behind.
        if !matches!(rec["t"].as_str(), Some("ask" | "approval" | "notice")) {
            h.file.sync_data().map_err(fail)?;
            #[cfg(test)]
            tests::SYNCS.with(|n| n.set(n.get() + 1));
        }
        Ok(rec)
    }

    /// The REPL ended before its first message: removes the session file this process created (never a resumed one).
    /// True once the file is gone, also on a later call. One try: when the removal fails the file stays a complete
    /// session, and later writes go on in it.
    fn discard(&mut self, session: &str) -> Result<bool, PortError> {
        let h = self.held(session)?;
        h.gone |= std::mem::take(&mut h.fresh) && fs::remove_file(&h.path).is_ok();
        Ok(h.gone)
    }
}

/// The session to resume, opened and locked: `id` in any scope, or the newest unlocked one in this cwd's scope. Empty
/// files (a run that died before its first record) are not sessions.
fn pick(state: &Path, cwd: &Path, id: Option<&str>) -> Result<(PathBuf, File), PortError> {
    let paths: Vec<PathBuf> = match id {
        Some(id) => named(state, id),
        None => ls(state.join("sessions").join(scope(cwd)?)).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect(),
    };
    let mtime = |p: &Path| fs::symlink_metadata(p).ok().filter(|m| m.is_file() && m.len() > 0)?.modified().ok();
    let mut found: Vec<(SystemTime, PathBuf)> = paths.into_iter().filter_map(|p| Some((mtime(&p)?, p))).collect();
    found.sort();
    let mut busy = None;
    for (_, path) in found.into_iter().rev() {
        let file = sys::open_append(&path).map_err(|e| io(&path, e))?;
        if file.try_lock().is_ok() {
            return Ok((path, file));
        }
        busy.get_or_insert(path);
    }
    Err(match busy.as_ref().and_then(|p| p.file_stem()) {
        Some(id) => port::err("busy", format!("session {} is in use", id.to_string_lossy())),
        None => port::err("not_found", format!("no session to resume in {}", cwd.display())),
    })
}

/// What `start` found: the session id, the history, the repair records as kept (`--json` stdout shows them before
/// `session`), and the notices: the binding's, then replay's.
pub type Begun = (String, Vec<Value>, Vec<Value>, Vec<String>);

/// A new session, or the resumed one: opened, read, replayed, its dangling calls repaired through the binding, and
/// paired.
pub fn start(h: &mut dyn History, resume: &Option<Option<String>>, cfg: &Config) -> Result<Begun, Outcome> {
    let Some(id) = resume else { return Ok((h.create(&cfg.cwd, None).map_err(port::outcome)?, Vec::new(), Vec::new(), Vec::new())) };
    let sid = h.open(&cfg.cwd, id.as_deref()).map_err(port::outcome)?;
    let mut t = h.read(&sid).map_err(port::outcome)?;
    let mut hist = replay(&mut t, cfg)?;
    let repairs = dangling(&hist).into_iter().map(|(id, name)| put(h, &sid, rec::tool_result(&id, &name, tools::REPAIR, true, 0, 0)));
    let repairs = repairs.collect::<Result<Vec<_>, _>>()?;
    hist.extend(repairs.iter().cloned());
    pairing(&hist).map_err(|e| Outcome::error("journal", format!("{}: {e}", t.place)))?;
    Ok((sid, hist, repairs, t.notices))
}

/// `rec` with `ts`, appended through the binding: the record as kept.
pub fn put(h: &mut dyn History, sid: &str, mut rec: Value) -> Result<Value, Outcome> {
    rec.as_object_mut().map(|o| o.insert("ts".into(), sys::utc().0.into()));
    h.append(sid, rec).map_err(port::outcome)
}

/// Over the records: the segment after the last `session` record ends with its `close` if it has one, the dialect is
/// the config's, and what changed since is noticed (in `t.notices`); then the history is rebuilt from the message kinds with every
/// `elide` applied.
fn replay(t: &mut Taken, cfg: &Config) -> Result<Vec<Value>, Outcome> {
    let Some(last) = t.records.iter().rposition(|r| r["t"] == "session") else { return Err(Outcome::error("journal", format!("{}: no session record", t.place))) };
    let seg = &t.records[last..];
    match seg.iter().position(|r| r["t"] == "close") {
        None => t.notices.push("previous run did not close cleanly".into()),
        Some(i) if i + 1 == seg.len() => {}
        Some(_) => return Err(port::outcome(tampered(&t.place))),
    }
    let s = &t.records[last];
    if s["dialect"] != cfg.dialect {
        let was = s["dialect"].as_str().unwrap_or("unknown");
        return Err(Outcome::error("journal", format!("{}: the session's dialect is {was}, not {}", t.place, cfg.dialect)));
    }
    for (k, now) in [("model", cfg.model.as_str().into()), ("tools", Value::from(cfg.tools.clone())), ("cwd", cfg.cwd.to_string_lossy().into())] {
        if s[k] != now {
            t.notices.push(format!("{k} changed since the session started: {} -> {now}", s[k]));
        }
    }
    let mut hist: Vec<Value> = Vec::new();
    for r in &t.records {
        match r["t"].as_str() {
            Some("user" | "assistant" | "tool_result") => hist.push(r.clone()),
            Some("elide") => hist.iter_mut().filter(|h| h["t"] == "tool_result" && r["ids"].as_array().is_some_and(|ids| ids.contains(&h["id"]))).for_each(|h| _ = shrink(h)),
            _ => {}
        }
    }
    Ok(hist)
}

/// The number of session `sid`'s last spill file: the highest `<sid>-N.log` in the spill dir, or the number of calls in
/// `hist` when that is more (the 24 h sweep may have removed a file whose name the history holds; each call took one at
/// most).
pub fn spill_n(state: &Path, sid: &str, hist: &[Value]) -> usize {
    let (names, prefix) = (fs::read_dir(state.join("spill")).into_iter().flatten().flatten().map(|e| e.file_name()), format!("{sid}-"));
    names.filter_map(|f| f.to_str()?.strip_prefix(&prefix)?.strip_suffix(".log")?.parse().ok()).max().unwrap_or(0).max(calls(hist).count())
}

/// The next spill name after `n`, `<sid>-<n>.log` (n from the counter, never from a model string): one per tool call
/// at most.
pub fn spill(state: &Path, sid: &str, n: &mut usize, home: &Path) -> Spill {
    *n = n.saturating_add(1);
    let path = state.join("spill").join(format!("{sid}-{n}.log"));
    Spill { shown: allow::shown(&path, home), path }
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

/// One builder per record kind, so the module reads as the record table; each sets "t"; `put` adds "ts".
pub mod rec {
    use crate::{config::Config, types::Call, types::Outcome, types::Usage};
    use serde_json::{Value, json};
    use std::path::Path;

    /// `resumed`: an object the std binding fills with `from_bytes`; `ports`: (port, display name) of each one bound
    /// outside std, only when there is one.
    pub fn session(cfg: &Config, id: &str, cwd: &Path, resumed: bool, ports: &[(&str, String)]) -> Value {
        let u = &cfg.url;
        let v = json!({"t": "session", "v": 1, "henki": env!("CARGO_PKG_VERSION"), "id": id, "cwd": cwd.to_string_lossy(), "model": cfg.model,
            "dialect": cfg.dialect, "base_url": format!("http://{}{}", crate::http::host_port(u), u.path), "base_url_from": cfg.url_from,
            "approve": if cfg.auto { "auto" } else { "ask" }, "approve_from": cfg.approve_from, "config": cfg.file.as_ref().map(|f| f.to_string_lossy()),
            "tools": cfg.tools, "shell": cfg.bash.to_string_lossy(), "exts": cfg.exts.iter().map(|e| &e.name).collect::<Vec<_>>(), "pass_env": cfg.pass_env, "ctx": cfg.window,
            "config_hash": cfg.hash, "allow_hash": cfg.allow.hash, "agents_hash": cfg.agents_hash, "project_agents_hash": null,
            "resumed": resumed.then(|| json!({}))});
        with(v, "ports", (!ports.is_empty()).then(|| ports.iter().map(|(p, n)| (p.to_string(), Value::from(n.as_str()))).collect::<serde_json::Map<_, _>>()))
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
    /// The std binding fills `bytes`.
    pub fn close() -> Value {
        json!({"t": "close"})
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
