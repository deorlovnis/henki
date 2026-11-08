#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use crate::types::{Call, Usage};
use serde_json::json;
use std::cell::Cell;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

thread_local! {
    /// How many fdatasync calls `write` made on this test's thread.
    pub static SYNCS: Cell<usize> = const { Cell::new(0) };
}

/// A held session of the std binding, written as the kernel writes (`put` adds `ts`), with the kernel's spill counter.
struct J {
    h: Journal,
    id: String,
    n: usize,
}

impl J {
    fn create(state: &Path, cwd: &Path) -> Result<J, Outcome> {
        let mut h = Journal::new(state);
        let id = h.create(cwd, None).map_err(port::outcome)?;
        Ok(J { h, id, n: 0 })
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn bytes(&self) -> u64 {
        self.h.held.as_ref().unwrap().bytes
    }
    fn path(&self) -> PathBuf {
        self.h.held.as_ref().unwrap().path.clone()
    }
    fn write(&mut self, r: Value) -> Result<Value, Outcome> {
        put(&mut self.h, &self.id, r)
    }
    fn discard(&mut self) -> bool {
        self.h.discard(&self.id).unwrap()
    }
    fn spill(&mut self, home: &Path) -> Spill {
        spill(&self.h.state, &self.id, &mut self.n, home)
    }
}

/// What `start` gave a resume, the session still held.
struct Resumed {
    journal: J,
    hist: Vec<Value>,
    from_bytes: u64,
    notices: Vec<String>,
}

/// `start` resuming `id` (or the newest session) of the scope `cwd`, with the dialect, model and tools of `cfg`; the
/// spill counter as a session starts it.
fn resume(state: &Path, cwd: &Path, id: Option<&str>, cfg: &Config) -> Result<Resumed, Outcome> {
    let mut c = Config::test(Path::new("/"));
    (c.cwd, c.dialect, c.model, c.tools) = (cwd.into(), cfg.dialect, cfg.model.clone(), cfg.tools.clone());
    let mut h = Journal::new(state);
    let (id, hist, _, notices) = start(&mut h, &Some(id.map(String::from)), &c)?;
    let (from_bytes, n) = (h.held.as_ref().unwrap().from, spill_n(state, &id, &hist));
    Ok(Resumed { journal: J { h, id, n }, hist, from_bytes, notices })
}

/// A fresh, empty directory under the temp dir, removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("henki-journal-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(fs::canonicalize(&p).unwrap())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A state dir with `sessions/` and `spill/`, a cwd, and a config rooted in the temp dir.
struct Fx {
    tmp: Tmp,
    state: PathBuf,
    cwd: PathBuf,
    cfg: Config,
}

fn fx() -> Fx {
    let tmp = Tmp::new();
    let state = tmp.0.join("state/henki");
    fs::create_dir_all(state.join("sessions")).unwrap();
    fs::create_dir_all(state.join("spill")).unwrap();
    let cwd = tmp.0.join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    let mut cfg = Config::test(&tmp.0);
    cfg.cwd = cwd.clone();
    Fx { tmp, state, cwd, cfg }
}

impl Fx {
    /// A new journal holding its `session` record, then `recs`; unlocked again on return.
    fn session(&self, recs: &[Value]) -> (String, PathBuf) {
        let mut j = J::create(&self.state, &self.cwd).unwrap();
        let id = j.id().to_string();
        j.write(rec::session(&self.cfg, &id, &self.cwd, false)).unwrap();
        for r in recs {
            j.write(r.clone()).unwrap();
        }
        (id, j.path())
    }

    /// The same, ended by `close` at the right offset.
    fn closed(&self, recs: &[Value]) -> (String, PathBuf) {
        let (id, path) = self.session(recs);
        let mut r = self.resume(Some(&id)).unwrap();
        r.journal.write(rec::close()).unwrap();
        (id, path)
    }

    fn resume(&self, id: Option<&str>) -> Result<Resumed, Outcome> {
        resume_at(&self.state, &self.cwd, id, &self.cfg)
    }

    /// One attempt, for the checks that expect a lock this test holds.
    fn busy(&self, id: Option<&str>) -> String {
        journal_err("journal", resume(&self.state, &self.cwd, id, &self.cfg))
    }

    /// Writes `lines` (records as compact JSON, one per line) as the session file `id` of this cwd.
    fn plant(&self, id: &str, lines: &[Value]) -> PathBuf {
        let dir = self.state.join("sessions").join(scope(&self.cwd).unwrap());
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id}.jsonl"));
        fs::write(&path, lines.iter().map(|l| l.to_string() + "\n").collect::<String>()).unwrap();
        path
    }
}

/// `resume`, waiting out a lock that only a forked child of another test still holds: until the child execs, it
/// shares the open file of a journal this test has already dropped, and with it the lock.
fn resume_at(state: &Path, cwd: &Path, id: Option<&str>, cfg: &Config) -> Result<Resumed, Outcome> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match resume(state, cwd, id, cfg) {
            Err(Outcome::Error { message, .. }) if message.ends_with(" is in use") && Instant::now() < until => std::thread::sleep(Duration::from_millis(1)),
            r => return r,
        }
    }
}

/// A lock on `p` held by the test itself, taken once no forked child of another test shares it (see `resume_at`).
fn hold(p: &Path) -> File {
    let (f, until) = (File::open(p).unwrap(), Instant::now() + Duration::from_secs(5));
    while f.try_lock().is_err() {
        assert!(Instant::now() < until, "{} stays locked", p.display());
        std::thread::sleep(Duration::from_millis(1));
    }
    f
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().mode() & 0o7777
}

fn lines(p: &Path) -> Vec<Value> {
    fs::read_to_string(p).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn set_mtime(p: &Path, age_s: u64) {
    let t = SystemTime::now() - Duration::from_secs(age_s);
    fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
}

fn without_ts(v: &Value) -> Value {
    let mut v = v.clone();
    v.as_object_mut().unwrap().remove("ts");
    v
}

fn call(id: &str) -> Call {
    Call { id: id.into(), name: "shell".into(), args: json!({"command": "ls"}), ..Default::default() }
}

fn asst(ids: &[&str]) -> Value {
    let calls: Vec<Call> = ids.iter().map(|i| call(i)).collect();
    rec::assistant("", &calls, if ids.is_empty() { "stop" } else { "tool_calls" }, Usage::default(), None)
}

fn res(id: &str) -> Value {
    rec::tool_result(id, "shell", "out", false, 1, 3)
}

fn journal_err(kind: &str, r: Result<Resumed, Outcome>) -> String {
    match r {
        Err(Outcome::Error { kind: k, message }) if k == kind => message,
        Err(e) => panic!("expected a {kind} error, got {e:?}"),
        Ok(_) => panic!("expected a {kind} error, got a resumed session"),
    }
}

#[test]
fn journal_layout_modes() {
    let tmp = Tmp::new();
    let home = tmp.0.join("home");
    fs::create_dir(&home).unwrap();
    let state = home.join(".local/state/henki");
    state_dir(&state).unwrap();
    for d in [home.join(".local"), home.join(".local/state"), state.clone(), state.join("sessions"), state.join("spill")] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    let cwd = home.join("proj");
    fs::create_dir(&cwd).unwrap();
    let link = tmp.0.join("link");
    symlink(&cwd, &link).unwrap();
    let j = J::create(&state, &link).unwrap();
    let dir = state.join("sessions").join(sys::fnv(cwd.as_os_str().as_bytes()));
    assert_eq!(j.path(), dir.join(format!("{}.jsonl", j.id())));
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&j.path()), 0o600);
    let id = j.id().as_bytes();
    assert_eq!(id.len(), 21, "{}", j.id());
    assert!(id[..8].iter().chain(&id[9..15]).all(u8::is_ascii_digit), "{}", j.id());
    assert_eq!((id[8], id[15], id[16]), (b'T', b'Z', b'-'));
    assert!(id[17..].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)), "{}", j.id());
    assert_eq!(j.bytes(), 0);
}

#[test]
fn journal_state_dir_tightened() {
    let tmp = Tmp::new();
    let xdg = tmp.0.join("xdg");
    fs::create_dir_all(xdg.join("henki/sessions")).unwrap();
    for d in [xdg.join("henki"), xdg.join("henki/sessions")] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let state = xdg.join("henki");
    state_dir(&state).unwrap();
    for d in [state.clone(), state.join("sessions"), state.join("spill")] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    // A symlink in place of the state dir is refused, with the fix.
    let (planted, target) = (tmp.0.join("planted"), tmp.0.join("target"));
    fs::create_dir_all(&planted).unwrap();
    fs::create_dir_all(&target).unwrap();
    symlink(&target, planted.join("henki")).unwrap();
    let e = state_dir(&planted.join("henki")).unwrap_err();
    assert!(e.starts_with(&format!("state dir {}: ", planted.join("henki").display())) && e.contains("; fix: "), "{e}");
    assert!(fs::read_dir(&target).unwrap().next().is_none());
}

#[test]
fn journal_excl_nofollow() {
    let f = fx();
    let dir = f.state.join("sessions/x");
    fs::create_dir_all(&dir).unwrap();
    let target = f.tmp.0.join("target");
    fs::write(&target, "keep").unwrap();
    symlink(&target, dir.join("a.jsonl")).unwrap();
    fs::write(dir.join("b.jsonl"), "old").unwrap();
    for name in ["a", "b"] {
        let r = Journal::new(&f.state).start(dir.join(format!("{name}.jsonl")), name.into());
        assert!(matches!(r, Err(PortError { code: "io", .. })), "{name}");
    }
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert_eq!(fs::read(dir.join("b.jsonl")).unwrap(), b"old");
    // The scope directory itself may not be a symlink either.
    let elsewhere = f.tmp.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, f.state.join("sessions").join(scope(&f.cwd).unwrap())).unwrap();
    assert!(matches!(J::create(&f.state, &f.cwd), Err(Outcome::Error { kind: "journal", .. })));
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
}

#[test]
fn journal_lock_in_use() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let id = j.id().to_string();
    j.write(rec::session(&f.cfg, &id, &f.cwd, false)).unwrap();
    let busy = format!("session {id} is in use");
    assert_eq!(f.busy(Some(&id)), busy);
    assert_eq!(f.busy(None), busy);
    drop(j);
    let r = f.resume(None).unwrap();
    assert_eq!(r.journal.id(), id);
    assert_eq!(f.busy(Some(&id)), busy);
    drop(r);
    assert!(f.resume(Some(&id)).is_ok());
}

#[test]
fn journal_record_shapes() {
    let s = |v: Value| serde_json::to_string(&v).unwrap();
    let cfg = Config::test(Path::new("/d"));
    let v = env!("CARGO_PKG_VERSION");
    assert_eq!(
        s(rec::session(&cfg, "20261105T171203Z-3f9a", Path::new("/p"), false)),
        format!(
            r#"{{"agents_hash":null,"allow_hash":null,"approve":"ask","approve_from":"default","base_url":"http://127.0.0.1:9/v1","base_url_from":"config","config":null,"config_hash":null,"ctx":32768,"cwd":"/p","dialect":"openai","exts":[],"henki":"{v}","id":"20261105T171203Z-3f9a","model":"m","pass_env":[],"project_agents_hash":null,"resumed":null,"shell":"/bin/bash","t":"session","tools":["shell","read","write","edit"],"v":1}}"#
        )
    );
    let mut cfg = Config::test(Path::new("/d"));
    (cfg.auto, cfg.approve_from, cfg.url_from) = (true, "--approve".into(), "HENKI_BASE_URL");
    (cfg.url.host, cfg.url.port, cfg.url.path) = ("::1".into(), 8080, String::new());
    (cfg.file, cfg.hash, cfg.agents_hash) = (Some("/c/config.toml".into()), Some("9c1d0e2a77f0b431".into()), Some("1a2b3c4d5e6f7081".into()));
    (cfg.pass_env, cfg.tools, cfg.window) = (vec!["FOO".into()], vec!["shell", "read"], 8192);
    assert_eq!(
        s(rec::session(&cfg, "x", Path::new("/p"), true)),
        format!(
            r#"{{"agents_hash":"1a2b3c4d5e6f7081","allow_hash":null,"approve":"auto","approve_from":"--approve","base_url":"http://[::1]:8080","base_url_from":"HENKI_BASE_URL","config":"/c/config.toml","config_hash":"9c1d0e2a77f0b431","ctx":8192,"cwd":"/p","dialect":"openai","exts":[],"henki":"{v}","id":"x","model":"m","pass_env":["FOO"],"project_agents_hash":null,"resumed":{{}},"shell":"/bin/bash","t":"session","tools":["shell","read"],"v":1}}"#
        )
    );
    assert_eq!(s(rec::user("hi")), r#"{"msg":{"content":"hi","role":"user"},"t":"user"}"#);
    assert_eq!(s(rec::delta(false, "a")), r#"{"delta":"a","t":"text"}"#);
    assert_eq!(s(rec::delta(true, "b")), r#"{"delta":"b","t":"reasoning"}"#);
    let calls = [call("h1"), Call { id: "h2".into(), name: "edit".into(), args: json!({}), raw: Some("{bad".into()), error: Some("x".into()) }];
    let u = Usage { input: 1840, output: 22, cached: 1664 };
    assert_eq!(
        s(rec::assistant("x", &calls, "tool_calls", u, None)),
        r#"{"finish":"tool_calls","msg":{"content":"x","role":"assistant","tool_calls":[{"args":{"command":"ls"},"id":"h1","name":"shell"},{"args":{},"id":"h2","name":"edit","raw":"{bad"}]},"t":"assistant","usage":{"cached":1664,"in":1840,"out":22}}"#
    );
    assert_eq!(
        s(rec::assistant("done", &[], "interrupted", Usage::default(), None)),
        r#"{"finish":"interrupted","msg":{"content":"done","role":"assistant","tool_calls":[]},"t":"assistant","usage":{"cached":0,"in":0,"out":0}}"#
    );
    assert_eq!(s(rec::ask(&call("h1"), None)), r#"{"args":{"command":"ls"},"id":"h1","name":"shell","t":"ask"}"#);
    assert_eq!(s(rec::ask(&call("h1"), Some("/p/a"))), r#"{"args":{"command":"ls"},"id":"h1","name":"shell","t":"ask","target":"/p/a"}"#);
    assert_eq!(s(rec::approval("h1", "auto", None)), r#"{"id":"h1","t":"approval","verdict":"auto"}"#);
    assert_eq!(
        s(rec::tool_result("call_7", "shell", "ws1 open\n[exit 0, 0.3 s]", false, 312, 402)),
        r#"{"bytes":402,"content":"ws1 open\n[exit 0, 0.3 s]","id":"call_7","is_error":false,"ms":312,"name":"shell","t":"tool_result"}"#
    );
    assert_eq!(s(rec::notice("n")), r#"{"t":"notice","text":"n"}"#);
    assert_eq!(s(rec::elide(&["h1".into(), "h2".into()])), r#"{"ids":["h1","h2"],"t":"elide"}"#);
    let u = Usage { input: 4092, output: 119, cached: 1702 };
    assert_eq!(s(rec::turn_end(&Outcome::Ok, 2, u)), r#"{"status":"ok","steps":2,"t":"turn_end","usage":{"cached":1702,"in":4092,"out":119}}"#);
    assert_eq!(
        s(rec::turn_end(&Outcome::GaveUp("stuck"), 4, Usage::default())),
        r#"{"reason":"stuck","status":"gave_up","steps":4,"t":"turn_end","usage":{"cached":0,"in":0,"out":0}}"#
    );
    assert_eq!(
        s(rec::result(&Outcome::Ok, "1 test fails: ...", u, Some("20261105T171203Z-3f9a"), 2, 49_149)),
        r#"{"duration_s":49.1,"exit":0,"session":"20261105T171203Z-3f9a","status":"ok","steps":2,"t":"result","text":"1 test fails: ...","usage":{"cached":1702,"in":4092,"out":119}}"#
    );
    assert_eq!(
        s(rec::result(&Outcome::error("config", "bad"), "", Usage::default(), None, 0, 0)),
        r#"{"duration_s":0.0,"error":{"kind":"config","message":"bad"},"exit":2,"session":null,"status":"error","steps":0,"t":"result","text":"","usage":{"cached":0,"in":0,"out":0}}"#
    );
    let c = Call { args: json!({"command": "rm -r x\ny"}), ..call("h1") };
    assert_eq!(
        s(rec::result(&Outcome::NeedsApproval(c), "", Usage::default(), Some("x"), 1, 1_950)),
        r#"{"call":{"args":{"command":"rm -r x\ny"},"id":"h1","name":"shell"},"duration_s":2.0,"exit":3,"session":"x","status":"needs_approval","steps":1,"t":"result","text":"","usage":{"cached":0,"in":0,"out":0}}"#
    );
    assert_eq!(
        s(rec::result(&Outcome::GaveUp("max_steps"), "t", Usage::default(), Some("x"), 12, 140)),
        r#"{"duration_s":0.1,"exit":1,"reason":"max_steps","session":"x","status":"gave_up","steps":12,"t":"result","text":"t","usage":{"cached":0,"in":0,"out":0}}"#
    );
    assert_eq!(s(rec::close()), r#"{"t":"close"}"#);
}

#[test]
fn journal_session_record_fields() {
    let mut cfg = Config::test(Path::new("/d"));
    let ext = |name: &str| crate::ext::Ext {
        name: name.into(),
        line: "a CLI".into(),
        doc: PathBuf::from(format!("/e/{name}/doc.md")),
        bin: None,
        cmds: vec![],
        shadows: vec![],
        paths: vec![],
        targets: vec![],
    };
    (cfg.dialect, cfg.exts, cfg.allow.hash) = ("anthropic", vec![ext("halti"), ext("itse")], Some("5d0c9e1f2a3b4c6d".into()));
    let r = rec::session(&cfg, "x", Path::new("/p"), false);
    assert_eq!((&r["dialect"], &r["exts"], &r["allow_hash"]), (&json!("anthropic"), &json!(["halti", "itse"]), &json!("5d0c9e1f2a3b4c6d")));
    assert_eq!(r["project_agents_hash"], Value::Null);
    // No rules read (an absent file, or auto mode): null, as in journal_record_shapes.
    assert_eq!(rec::session(&Config::test(Path::new("/d")), "x", Path::new("/p"), false)["allow_hash"], Value::Null);
}

#[test]
fn journal_assistant_blocks() {
    let s = |v: Value| serde_json::to_string(&v).unwrap();
    let blocks = json!([{"signature": "EqQB", "thinking": "", "type": "thinking"}, {"id": "toolu_1", "input": {"command": "ls"}, "name": "shell", "type": "tool_use"}]);
    let c = [call("toolu_1")];
    assert_eq!(
        s(rec::assistant("", &c, "tool_calls", Usage::default(), Some(&blocks))),
        r#"{"blocks":[{"signature":"EqQB","thinking":"","type":"thinking"},{"id":"toolu_1","input":{"command":"ls"},"name":"shell","type":"tool_use"}],"finish":"tool_calls","msg":{"content":"","role":"assistant","tool_calls":[{"args":{"command":"ls"},"id":"toolu_1","name":"shell"}]},"t":"assistant","usage":{"cached":0,"in":0,"out":0}}"#
    );
    assert!(rec::assistant("", &c, "tool_calls", Usage::default(), None).get("blocks").is_none());
}

#[test]
fn journal_approval_rule() {
    let s = |v: Value| serde_json::to_string(&v).unwrap();
    assert_eq!(s(rec::approval("call_7", "rule", Some(3))), r#"{"id":"call_7","rule":3,"t":"approval","verdict":"rule"}"#);
    assert_eq!(s(rec::approval("h1", "always", Some(1))), r#"{"id":"h1","rule":1,"t":"approval","verdict":"always"}"#);
    for v in ["yes", "no", "auto"] {
        assert_eq!(s(rec::approval("h1", v, None)), format!(r#"{{"id":"h1","t":"approval","verdict":"{v}"}}"#));
    }
}

#[test]
fn journal_sync_table() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let c = call("h1");
    let recs = [
        (rec::session(&f.cfg, j.id(), &f.cwd, false), true),
        (rec::user("u"), true),
        (asst(&["h1"]), true),
        (rec::ask(&c, None), false),
        (rec::approval("h1", "auto", None), false),
        (res("h1"), true),
        (rec::notice("n"), false),
        (rec::elide(&["h1".into()]), true),
        (rec::turn_end(&Outcome::Ok, 1, Usage::default()), true),
        (rec::result(&Outcome::Ok, "", Usage::default(), Some("x"), 1, 0), true),
        (rec::close(), true),
    ];
    for (r, synced) in recs {
        let before = SYNCS.with(Cell::get);
        j.write(r.clone()).unwrap();
        assert_eq!(SYNCS.with(Cell::get) - before, usize::from(synced), "{}", r["t"]);
    }
}

#[test]
fn journal_write_adds_ts_and_counts() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let w = j.write(rec::notice("a")).unwrap();
    let ts = w["ts"].as_str().unwrap();
    assert_eq!((ts.len(), &ts[10..11], &ts[19..]), (20, "T", "Z"));
    assert_eq!(without_ts(&w), rec::notice("a"));
    let line = format!("{}\n", term::json_line(&w));
    assert_eq!(fs::read_to_string(j.path()).unwrap(), line);
    assert_eq!(j.bytes(), line.len() as u64);
}

#[test]
fn journal_tamper_live_append() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    j.write(rec::notice("a")).unwrap();
    fs::File::options().append(true).open(j.path()).unwrap().write_all(b"{}\n").unwrap();
    let tampered = Outcome::error("tampered", format!("journal modified outside henki: {}", j.path().display()));
    assert_eq!(j.write(rec::user("b")).unwrap_err(), tampered);
    assert!(fs::read(j.path()).unwrap().ends_with(b"\n{}\n"));
    // A cut is caught as well.
    fs::File::options().write(true).open(j.path()).unwrap().set_len(3).unwrap();
    assert_eq!(j.write(rec::user("b")).unwrap_err(), tampered);
    assert_eq!(fs::metadata(j.path()).unwrap().len(), 3);
}

/// Another file in the journal's place, with the same bytes (as `sed -i` leaves it): the size alone matches.
#[test]
fn journal_tamper_replaced() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    j.write(rec::notice("a")).unwrap();
    let copy = j.path().with_extension("tmp");
    fs::copy(j.path(), &copy).unwrap();
    fs::rename(&copy, j.path()).unwrap();
    let tampered = Outcome::error("tampered", format!("journal modified outside henki: {}", j.path().display()));
    assert_eq!(j.write(rec::user("b")).unwrap_err(), tampered);
    assert_eq!(fs::metadata(j.path()).unwrap().len(), j.bytes(), "nothing written to either file");
}

#[test]
fn journal_write_fail() {
    let f = fx();
    let path = f.tmp.0.join("read-only.jsonl");
    fs::write(&path, "").unwrap();
    let held = Held { file: File::open(&path).unwrap(), path: path.clone(), id: "x".into(), bytes: 0, from: 0, fresh: false, gone: false };
    let mut j = J { h: Journal { state: f.state.clone(), held: Some(held) }, id: "x".into(), n: 0 };
    match j.write(rec::user("a")) {
        Err(Outcome::Error { kind: "journal", message }) => assert!(message.starts_with(&format!("cannot write {}: ", path.display())), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(j.bytes(), 0);
}

#[test]
fn journal_torn_every_offset() {
    let f = fx();
    let (id, path) = f.session(&[rec::user("héllo ✓ wörld"), asst(&["h1", "h2"]), res("h1"), rec::notice("ünïcode")]);
    let full = fs::read(&path).unwrap();
    let first = full.iter().position(|&b| b == b'\n').unwrap() + 1;
    for cut in 0..=full.len() {
        fs::write(&path, &full[..cut]).unwrap();
        let keep = full[..cut].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        if cut < first {
            let e = if cut == 0 { format!("no session to resume in {}", f.cwd.display()) } else { format!("{}: no session record", path.display()) };
            assert_eq!(journal_err("journal", f.resume(Some(&id))), e, "cut {cut}");
            assert_eq!(fs::metadata(&path).unwrap().len(), 0, "cut {cut}");
            continue;
        }
        let mut r = f.resume(Some(&id)).unwrap();
        assert_eq!(r.from_bytes, keep as u64, "cut {cut}");
        assert_eq!(r.notices.iter().any(|n| n.starts_with("dropped an incomplete last line (")), keep < cut, "cut {cut}");
        let repaired = fs::read(&path).unwrap();
        assert!(repaired.starts_with(&full[..keep]), "cut {cut}");
        r.journal.write(rec::session(&f.cfg, &id, &f.cwd, true)).unwrap();
        r.journal.write(rec::user("again")).unwrap();
        drop(r);
        let r = f.resume(Some(&id)).unwrap();
        let got = fs::read(&path).unwrap();
        assert!(got.starts_with(&repaired), "cut {cut}");
        assert_eq!(got.iter().filter(|&&b| b == b'\n').count(), repaired.iter().filter(|&&b| b == b'\n').count() + 2, "cut {cut}");
        assert_eq!(r.from_bytes, got.len() as u64, "cut {cut}: nothing left to repair");
        assert_eq!(r.notices, ["previous run did not close cleanly"], "cut {cut}");
        pairing(&r.hist).unwrap();
        assert_eq!(r.hist.last().unwrap()["msg"]["content"], "again");
    }
    // The notice gives the size of what it dropped in grouped digits.
    fs::write(&path, [&full[..], &[b'x'; 1500][..]].concat()).unwrap();
    let r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.notices[0], format!("dropped an incomplete last line (1,500 bytes) from {}", path.display()));
    assert_eq!(r.from_bytes, full.len() as u64);
}

#[test]
fn journal_corrupt_line() {
    let f = fx();
    let ok = [rec::session(&f.cfg, "c", &f.cwd, false), rec::user("a")];
    let path = f.plant("c", &ok);
    for bad in [b"not json\n".as_slice(), b"5\n", b"\n", b"{\"t\":\"user\"} {}\n", b"{\"t\":\"us\xffer\"}\n"] {
        let mut bytes = fs::read(&path).unwrap();
        bytes.truncate(ok.iter().map(|l| l.to_string().len() + 1).sum());
        bytes.extend(bad);
        bytes.extend(b"{\"t\":\"notice\",\"text\":\"after\"}\n");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(journal_err("journal", f.resume(None)), format!("corrupt journal {}:3", path.display()), "{:?}", String::from_utf8_lossy(bad));
    }
    let mut bytes = fs::read(&path).unwrap();
    bytes.splice(0..0, *b"[1,2]\n");
    fs::write(&path, &bytes).unwrap();
    assert_eq!(journal_err("journal", f.resume(None)), format!("corrupt journal {}:1", path.display()));
}

#[test]
fn journal_tamper_after_close() {
    let f = fx();
    let (id, path) = f.closed(&[rec::user("a")]);
    let full = fs::read(&path).unwrap();
    let r = f.resume(Some(&id)).unwrap();
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    drop(r);
    let tampered = format!("journal modified outside henki: {}", path.display());
    // A record after the close.
    fs::write(&path, [&full[..], b"{\"t\":\"notice\",\"text\":\"x\"}\n"].concat()).unwrap();
    assert_eq!(journal_err("tampered", f.resume(Some(&id))), tampered);
    // A close at the wrong offset (a line removed before it).
    let lines: Vec<&[u8]> = full.split_inclusive(|&b| b == b'\n').collect();
    fs::write(&path, [lines[0], lines[2]].concat()).unwrap();
    assert_eq!(journal_err("tampered", f.resume(Some(&id))), tampered);
    // Two closes.
    let mut two = full.clone();
    two.extend(format!("{}\n", json!({"bytes": full.len(), "t": "close"})).bytes());
    fs::write(&path, &two).unwrap();
    assert_eq!(journal_err("tampered", f.resume(Some(&id))), tampered);
    // An earlier segment's close is not checked again once a later session record starts a new segment.
    let mut next = full.clone();
    next.extend(format!("{}\n", rec::session(&f.cfg, &id, &f.cwd, true)).bytes());
    fs::write(&path, &next).unwrap();
    assert_eq!(f.resume(Some(&id)).unwrap().notices, ["previous run did not close cleanly"]);
}

#[test]
fn journal_no_close_notice() {
    let f = fx();
    let (id, _) = f.session(&[rec::user("a")]);
    assert_eq!(f.resume(Some(&id)).unwrap().notices, ["previous run did not close cleanly"]);
    let f = fx();
    let (id, _) = f.closed(&[rec::user("a")]);
    assert!(f.resume(Some(&id)).unwrap().notices.is_empty());
}

#[test]
fn journal_resume_dialect_mismatch_both_ways() {
    let f = fx();
    let mut anthropic = Config::test(&f.tmp.0);
    (anthropic.cwd, anthropic.dialect) = (f.cwd.clone(), "anthropic");
    let (open, other) = (rec::session(&f.cfg, "d", &f.cwd, false), rec::session(&anthropic, "d", &f.cwd, false));
    assert_eq!((&open["dialect"], &other["dialect"]), (&json!("openai"), &json!("anthropic")));
    let path = f.plant("d", &[other.clone(), rec::user("a")]);
    assert_eq!(journal_err("journal", f.resume(None)), format!("{}: the session's dialect is anthropic, not openai", path.display()));
    assert!(resume_at(&f.state, &f.cwd, None, &anthropic).is_ok());
    f.plant("d", &[open.clone(), rec::user("a")]);
    let e = journal_err("journal", resume_at(&f.state, &f.cwd, None, &anthropic));
    assert_eq!(e, format!("{}: the session's dialect is openai, not anthropic", path.display()));
    assert!(f.resume(None).is_ok());
    // The last session record decides.
    f.plant("d", &[open.clone(), rec::user("a"), other.clone()]);
    journal_err("journal", f.resume(None));
    assert!(resume_at(&f.state, &f.cwd, None, &anthropic).is_ok());
    f.plant("d", &[other, rec::user("a"), open.clone()]);
    assert!(f.resume(None).is_ok());
    journal_err("journal", resume_at(&f.state, &f.cwd, None, &anthropic));
    // A session record without a dialect matches neither.
    let mut none = open;
    none.as_object_mut().unwrap().remove("dialect");
    f.plant("d", &[none, rec::user("a")]);
    assert_eq!(journal_err("journal", f.resume(None)), format!("{}: the session's dialect is unknown, not openai", path.display()));
    let path = f.plant("d", &[rec::user("a")]);
    assert_eq!(journal_err("journal", f.resume(None)), format!("{}: no session record", path.display()));
}

/// A session of the Anthropic dialect replays its blocks exactly: the request body after a resume is the live one.
#[test]
fn journal_resume_keeps_blocks() {
    let mut f = fx();
    f.cfg.dialect = "anthropic";
    let blocks = json!([{"signature": "EqQBCkYI", "thinking": "", "type": "thinking"}, {"data": "ZGF0YQ==", "type": "redacted_thinking"},
        {"text": "\u{e9} \u{2713} \u{1b}[0m \u{202e}", "type": "text"}, {"id": "toolu_1", "input": {"command": "ls", "n": 1.5, "z": [true, null]}, "name": "shell", "type": "tool_use"}]);
    let c = Call { id: "toolu_1".into(), args: blocks[3]["input"].clone(), ..call("x") };
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let id = j.id().to_string();
    j.write(rec::session(&f.cfg, &id, &f.cwd, false)).unwrap();
    let text = blocks[2]["text"].as_str().unwrap();
    let recs = [rec::user("go"), rec::assistant(text, &[c], "tool_calls", Usage::default(), Some(&blocks)), rec::tool_result("toolu_1", "shell", "ok", false, 1, 2)];
    let live: Vec<Value> = recs.into_iter().map(|r| j.write(r).unwrap()).collect();
    drop(j);
    let r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.hist, live);
    assert_eq!(r.hist[1]["blocks"], blocks);
    let body = |h: &[Value]| crate::anthropic::body("m", 1, "SYS", &json!([]), h);
    assert_eq!(body(&r.hist), body(&live));
}

/// Only the session file this process created is removed; after that nothing more is written. A resumed one stays.
#[test]
fn journal_discard_fresh_only() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let id = j.id().to_string();
    j.write(rec::session(&f.cfg, &id, &f.cwd, false)).unwrap();
    let (path, bytes) = (j.path(), j.bytes());
    assert!(j.discard());
    assert!(!path.exists());
    assert_eq!(without_ts(&j.write(rec::close()).unwrap()), json!({"bytes": bytes, "t": "close"}));
    assert_eq!((path.exists(), j.bytes()), (false, bytes));
    assert_eq!(journal_err("journal", f.resume(None)), format!("no session to resume in {}", f.cwd.display()));
    let (id, path) = f.session(&[rec::user("a")]);
    let mut r = f.resume(Some(&id)).unwrap();
    assert!(!r.journal.discard());
    let before = fs::metadata(&path).unwrap().len();
    r.journal.write(rec::notice("still written")).unwrap();
    assert!(fs::metadata(&path).unwrap().len() > before);
}

/// A second discard reports the file gone again and removes nothing; a removal that fails is not tried again, and the
/// session goes on being written.
#[test]
fn journal_discard_twice() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    j.write(rec::session(&f.cfg, j.id(), &f.cwd, false)).unwrap();
    let path = j.path();
    assert!(j.discard());
    fs::write(&path, "not henki's\n").unwrap();
    assert!(j.discard());
    assert_eq!(fs::read_to_string(&path).unwrap(), "not henki's\n");
    fs::remove_file(&path).unwrap();
    // Removed by someone else first: the unlink fails, so the journal is not gone and its writes still count.
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    j.write(rec::session(&f.cfg, j.id(), &f.cwd, false)).unwrap();
    let bytes = j.bytes();
    fs::remove_file(j.path()).unwrap();
    assert!(!j.discard());
    // Its path no longer names the file written to, so the writes are `tampered` (nothing goes into an unlinked file).
    assert!(matches!(j.write(rec::close()), Err(Outcome::Error { kind: "tampered", .. })));
    assert_eq!(j.bytes(), bytes);
    fs::write(j.path(), "").unwrap();
    assert!(!j.discard());
    assert!(j.path().exists(), "one try only");
}

#[test]
fn journal_resume_changes_noticed() {
    let f = fx();
    let (id, _) = f.session(&[rec::user("a")]);
    let mut cfg = Config::test(&f.tmp.0);
    (cfg.model, cfg.tools) = ("other".into(), vec!["read"]);
    let r = resume_at(&f.state, &f.cwd, Some(&id), &cfg).unwrap();
    assert_eq!(
        r.notices,
        [
            "previous run did not close cleanly",
            r#"model changed since the session started: "m" -> "other""#,
            r#"tools changed since the session started: ["shell","read","write","edit"] -> ["read"]"#,
        ]
    );
}

#[test]
fn journal_resume_counters() {
    let f = fx();
    let (id, path) = f.session(&[rec::user("u"), asst(&["h3", "h9"]), res("h3"), res("h9"), asst(&[])]);
    for name in [format!("{id}-3.log"), format!("{id}-7.log"), format!("{id}-x.log"), "other-9.log".into(), format!("{id}-12.txt")] {
        fs::write(f.state.join("spill").join(name), "").unwrap();
    }
    let mut r = f.resume(None).unwrap();
    assert_eq!(r.journal.id(), id);
    assert_eq!(r.journal.bytes(), fs::metadata(&path).unwrap().len());
    assert_eq!(r.journal.spill(&f.tmp.0).path, f.state.join(format!("spill/{id}-8.log")));
    assert_eq!(next_h(&r.hist), 10);
}

#[test]
fn journal_repair_appended() {
    let f = fx();
    let (id, path) = f.session(&[rec::user("u"), asst(&["h1", "h2", "h3"]), res("h1")]);
    let before = fs::metadata(&path).unwrap().len();
    let r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.from_bytes, before);
    let all = lines(&path);
    let repairs = &all[all.len() - 2..];
    for (rep, cid) in repairs.iter().zip(["h2", "h3"]) {
        assert_eq!(without_ts(rep), rec::tool_result(cid, "shell", tools::REPAIR, true, 0, 0));
        assert!(rep["ts"].is_string());
    }
    assert_eq!(&r.hist[r.hist.len() - 2..], repairs);
    assert_eq!(r.journal.bytes(), fs::metadata(&path).unwrap().len());
    pairing(&r.hist).unwrap();
    drop(r);
    let r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.from_bytes, fs::metadata(&path).unwrap().len());
    assert_eq!(lines(&path).len(), all.len());
}

#[test]
fn journal_resume_elide() {
    let f = fx();
    let big = rec::tool_result("h1", "shell", &"é".repeat(750), false, 1, 1500);
    let mut live = vec![rec::user("u"), asst(&["h1"]), big, asst(&["h2"]), res("h2"), asst(&["h3"]), res("h3")];
    let journaled = live.clone();
    let ids = elide(&mut live);
    assert_eq!(ids, ["h1"]);
    let (id, _) = f.session(&[journaled, vec![rec::elide(&ids), rec::notice("n")]].concat());
    let r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.hist.iter().map(without_ts).collect::<Vec<_>>(), live);
    assert_eq!(r.hist[2]["content"], "[output elided: 1,500 bytes]");
}

#[test]
fn record_hist_kinds() {
    let f = fx();
    let c = call("h1");
    let recs = [
        rec::user("u"),
        rec::assistant("", std::slice::from_ref(&c), "tool_calls", Usage::default(), None),
        rec::ask(&c, Some("/x")),
        rec::approval("h1", "auto", None),
        rec::notice("n"),
        res("h1"),
        rec::elide(&[]),
        rec::turn_end(&Outcome::Ok, 1, Usage::default()),
        rec::result(&Outcome::Ok, "", Usage::default(), Some("x"), 1, 0),
    ];
    let (id, path) = f.session(&recs);
    let r = f.resume(Some(&id)).unwrap();
    let kinds: Vec<&str> = r.hist.iter().map(|h| h["t"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["user", "assistant", "tool_result"]);
    let written: Vec<Value> = lines(&path).into_iter().filter(|l| ["user", "assistant", "tool_result"].contains(&l["t"].as_str().unwrap())).collect();
    assert_eq!(r.hist, written);
}

#[test]
fn journal_resume_scoped_cwd() {
    let f = fx();
    let other = f.tmp.0.join("other");
    fs::create_dir(&other).unwrap();
    let (a, path_a) = f.session(&[rec::user("a")]);
    let mut j = J::create(&f.state, &other).unwrap();
    let b = j.id().to_string();
    j.write(rec::session(&f.cfg, &b, &other, false)).unwrap();
    drop(j);
    set_mtime(&path_a, 3600);
    assert_eq!(f.resume(None).unwrap().journal.id(), a);
    let r = resume_at(&f.state, &other, None, &f.cfg).unwrap();
    assert_eq!((r.journal.id(), r.notices.len()), (b.as_str(), 1));
    drop(r);
    // An id is found in any scope; the moved cwd is noticed.
    let r = f.resume(Some(&b)).unwrap();
    assert_eq!(r.journal.id(), b);
    assert!(r.notices.iter().any(|n| n.starts_with("cwd changed since the session started: ")), "{:?}", r.notices);
    let third = f.tmp.0.join("third");
    fs::create_dir(&third).unwrap();
    assert_eq!(journal_err("journal", resume_at(&f.state, &third, None, &f.cfg)), format!("no session to resume in {}", third.display()));
}

#[test]
fn journal_resume_none() {
    let f = fx();
    let none = format!("no session to resume in {}", f.cwd.display());
    assert_eq!(journal_err("journal", f.resume(None)), none);
    let path = f.plant("real", &[rec::session(&f.cfg, "real", &f.cwd, false)]);
    for id in ["nope", "../real", "", "/real"] {
        assert_eq!(journal_err("journal", f.resume(Some(id))), none, "{id:?}");
    }
    // Only regular `.jsonl` files count.
    fs::rename(&path, path.with_extension("log")).unwrap();
    symlink(path.with_extension("log"), &path).unwrap();
    assert_eq!(journal_err("journal", f.resume(None)), none);
    assert_eq!(journal_err("journal", resume_at(&f.tmp.0.join("missing"), &f.cwd, None, &f.cfg)), none);
}

#[test]
fn journal_resume_newest_unlocked() {
    let f = fx();
    let (old, new) = ("20260101T000000Z-aaaa", "20260102T000000Z-bbbb");
    let path_old = f.plant(old, &[rec::session(&f.cfg, old, &f.cwd, false)]);
    let path_new = f.plant(new, &[rec::session(&f.cfg, new, &f.cwd, false)]);
    set_mtime(&path_old, 7200);
    set_mtime(&path_new, 3600);
    assert_eq!(f.resume(None).unwrap().journal.id(), new);
    // mtime decides, not the name.
    set_mtime(&path_old, 60);
    assert_eq!(f.resume(None).unwrap().journal.id(), old);
    set_mtime(&path_old, 7200);
    let _held = hold(&path_new);
    assert_eq!(f.resume(None).unwrap().journal.id(), old);
    let _held_too = hold(&path_old);
    assert_eq!(f.busy(None), format!("session {new} is in use"));
}

/// A run that died before its first record leaves an empty file, or one torn line; neither blocks `--resume` of the
/// session before it.
#[test]
fn journal_resume_skips_empty() {
    let f = fx();
    let (old, path_old) = f.session(&[rec::user("a")]);
    set_mtime(&path_old, 60);
    let empty = f.plant("20990101T000000Z-0000", &[]);
    assert_eq!(f.resume(None).unwrap().journal.id(), old);
    let none = format!("no session to resume in {}", f.cwd.display());
    assert_eq!(journal_err("journal", f.resume(Some("20990101T000000Z-0000"))), none);
    // A torn first line is cut and reported once; the empty file left behind is then skipped.
    fs::write(&empty, b"{\"t\":\"sess").unwrap();
    assert_eq!(journal_err("journal", f.resume(None)), format!("{}: no session record", empty.display()));
    assert_eq!(fs::metadata(&empty).unwrap().len(), 0);
    assert_eq!(f.resume(None).unwrap().journal.id(), old);
}

/// A write the kernel cuts short (here at the file size limit) ends with its cause, and every later write finds the
/// size off and stops instead of appending after the partial line.
#[test]
fn journal_short_write() {
    // The size limit is per process, so the body runs in a child test process under `ulimit -f 1` (1 KiB), with
    // SIGXFSZ ignored so that the write past the limit fails instead of killing it.
    if std::env::var_os("HENKI_TEST_FSIZE").is_none() {
        let script = r#"ulimit -f 1 && trap '' XFSZ && exec "$0" --exact journal::tests::journal_short_write --test-threads=1"#;
        let out = Command::new("bash").args(["-c", script]).arg(std::env::current_exe().unwrap()).env("HENKI_TEST_FSIZE", "1").output().unwrap();
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 passed"), "{out:?}");
        return;
    }
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    j.write(rec::notice(&"a".repeat(600))).unwrap();
    let written = j.bytes();
    let e = Outcome::error("journal", format!("cannot write {}: File too large (os error 27)", j.path().display()));
    assert_eq!(j.write(rec::notice(&"b".repeat(600))).unwrap_err(), e);
    assert_eq!((j.bytes(), fs::metadata(j.path()).unwrap().len()), (written, 1024));
    let tampered = Outcome::error("tampered", format!("journal modified outside henki: {}", j.path().display()));
    assert_eq!(j.write(rec::notice("c")).unwrap_err(), tampered);
    assert_eq!(fs::metadata(j.path()).unwrap().len(), 1024);
}

#[test]
fn journal_resume_appends_at_end() {
    let f = fx();
    let (id, path) = f.session(&[rec::user("u"), asst(&["h1"])]);
    let whole = fs::read(&path).unwrap();
    fs::File::options().append(true).open(&path).unwrap().write_all(b"{\"bytes\":3,\"content\":\"tor").unwrap();
    let mut r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.from_bytes, whole.len() as u64);
    r.journal.write(rec::notice("next")).unwrap();
    let got = fs::read(&path).unwrap();
    assert!(got.starts_with(&whole));
    let tail: Vec<Value> = lines(&path).split_off(whole.iter().filter(|&&b| b == b'\n').count());
    assert_eq!(tail.iter().map(without_ts).collect::<Vec<_>>(), [rec::tool_result("h1", "shell", tools::REPAIR, true, 0, 0), rec::notice("next")]);
    assert_eq!(r.journal.bytes(), got.len() as u64);
}

#[test]
fn pairing_ok() {
    let hist = [rec::user("u"), asst(&["h1", "h2"]), res("h2"), res("h1"), asst(&[]), rec::user("v"), asst(&["h3"]), res("h3"), asst(&[])];
    assert_eq!(pairing(&hist), Ok(()));
    assert_eq!(pairing(&[]), Ok(()));
}

#[test]
fn pairing_duplicate_id() {
    let e = pairing(&[rec::user("u"), asst(&["h1"]), res("h1"), asst(&["h1"]), res("h1")]).unwrap_err();
    assert_eq!(e, r#"call id "h1" is used twice"#);
    assert!(pairing(&[rec::user("u"), asst(&["h1", "h1"]), res("h1"), res("h1")]).is_err());
}

#[test]
fn pairing_missing_result() {
    assert_eq!(pairing(&[rec::user("u"), asst(&["h1", "h2"]), res("h1")]).unwrap_err(), r#"call "h2" has no result"#);
    let e = pairing(&[rec::user("u"), asst(&["h1", "h2"]), res("h2"), asst(&[])]).unwrap_err();
    assert_eq!(e, r#"call "h1" has no result before the next assistant record"#);
}

#[test]
fn pairing_orphan_result() {
    assert_eq!(pairing(&[rec::user("u"), res("h9")]).unwrap_err(), r#"result "h9" has no open call"#);
    assert!(pairing(&[rec::user("u"), asst(&["h1"]), res("h1"), res("h1")]).is_err());
    assert!(pairing(&[rec::user("u"), asst(&["h1"]), res("h1"), asst(&[]), res("h1")]).is_err());
}

#[test]
fn pairing_user_between() {
    let e = pairing(&[rec::user("u"), asst(&["h1"]), rec::user("v"), res("h1")]).unwrap_err();
    assert_eq!(e, r#"call "h1" has no result before the next user record"#);
}

#[test]
fn pairing_ignores_non_message_kinds() {
    let c = call("h1");
    let recs = [
        rec::user("u"),
        asst(&["h1"]),
        rec::ask(&c, None),
        rec::approval("h1", "auto", None),
        rec::notice("n"),
        res("h1"),
        rec::elide(&["h1".into()]),
        rec::turn_end(&Outcome::Ok, 1, Usage::default()),
    ];
    assert_eq!(pairing(&recs), Ok(()));
    // A whole journal file, session, result and close included.
    let f = fx();
    let end = [rec::result(&Outcome::Ok, "", Usage::default(), Some("x"), 1, 0)];
    let (_, path) = f.closed(&[&recs[..], &end[..]].concat());
    let all = lines(&path);
    assert_eq!((all[0]["t"].as_str(), all[all.len() - 1]["t"].as_str()), (Some("session"), Some("close")));
    assert_eq!(pairing(&all), Ok(()));
}

#[test]
fn elide_last_two_steps() {
    let mut hist = vec![rec::user("u"), asst(&["h1"]), res("h1"), asst(&["h2"]), res("h2"), asst(&["h3"]), res("h3")];
    assert_eq!(elide(&mut hist), ["h1"]);
    assert_eq!(hist[2]["content"], "[output elided: 3 bytes]");
    assert_eq!((&hist[4]["content"], &hist[6]["content"]), (&json!("out"), &json!("out")));
    hist.extend([asst(&["h4", "h5"]), res("h4"), res("h5")]);
    assert_eq!(elide(&mut hist), ["h2"]);
    hist.extend([rec::user("v"), asst(&[])]);
    assert_eq!(elide(&mut hist), ["h3"]);
    assert_eq!(hist.iter().filter(|h| h["content"] == "out").count(), 2);
}

#[test]
fn elide_deterministic() {
    let step = |content: &str| vec![rec::user("u"), asst(&["h1"]), rec::tool_result("h1", "shell", content, false, 1, 9), asst(&[]), asst(&[])];
    let (mut a, mut b) = (step(&"a".repeat(2_941_512)), step(&"✓".repeat(980_504)));
    assert_eq!((elide(&mut a), elide(&mut b)), (vec!["h1".to_string()], vec!["h1".to_string()]));
    assert_eq!(a, b);
    assert_eq!(a[2]["content"], "[output elided: 2,941,512 bytes]");
    let once = a.clone();
    assert!(elide(&mut a).is_empty());
    assert_eq!(a, once);
}

#[test]
fn elide_nothing_left() {
    let mut hist = vec![rec::user("u"), asst(&["h1"]), res("h1"), asst(&["h2"]), res("h2")];
    let before = hist.clone();
    assert!(elide(&mut hist).is_empty());
    assert_eq!(hist, before);
    assert!(elide(&mut []).is_empty());
    hist.extend([asst(&[])]);
    assert_eq!(elide(&mut hist), ["h1"]);
    assert!(elide(&mut hist).is_empty());
}

/// Output that only starts like the marker is elided like any other; only the marker itself counts as elided.
#[test]
fn elide_marker_lookalike() {
    let big = format!("{ELIDED}{}", "x".repeat(20_000));
    let small = "[output elided: 3 bytes]";
    let mut hist = vec![rec::user("u"), asst(&["h1", "h2"]), rec::tool_result("h1", "shell", &big, false, 1, 9), rec::tool_result("h2", "read", small, false, 1, 9)];
    hist.extend([asst(&["h3"]), res("h3"), asst(&[])]);
    assert_eq!(elide(&mut hist), ["h1"]);
    assert_eq!((&hist[2]["content"], &hist[3]["content"]), (&json!("[output elided: 20,016 bytes]"), &json!(small)));
    assert!(elide(&mut hist).is_empty());
}

#[test]
fn next_h_max_plus_one() {
    assert_eq!(next_h(&[]), 1);
    assert_eq!(next_h(&[rec::user("h7"), res("h99")]), 1);
    assert_eq!(next_h(&[asst(&["call_7", "h2", "h10", "hx", "h", "h-3"]), asst(&["h4"])]), 11);
    assert_eq!(next_h(&[asst(&["h4294967295"])]), u32::MAX);
}

#[test]
fn spill_name_counter() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let id = j.id().to_string();
    let s = j.spill(&f.tmp.0);
    assert_eq!(s.path, f.state.join(format!("spill/{id}-1.log")));
    assert_eq!(s.shown, format!("~/state/henki/spill/{id}-1.log"));
    let s = j.spill(Path::new("/nowhere"));
    assert_eq!((s.shown, s.path.clone()), (s.path.display().to_string(), f.state.join(format!("spill/{id}-2.log"))));
}

/// The 24 h sweep may remove a spill file whose name the history still holds; a resumed session never hands that name
/// out again.
#[test]
fn spill_counter_after_sweep() {
    let f = fx();
    let mut j = J::create(&f.state, &f.cwd).unwrap();
    let id = j.id().to_string();
    j.write(rec::session(&f.cfg, &id, &f.cwd, false)).unwrap();
    j.write(asst(&["h1", "h2"])).unwrap();
    let first = j.spill(&f.tmp.0);
    fs::write(&first.path, "full output").unwrap();
    j.write(rec::tool_result("h1", "shell", &format!("head\n[full output: {}]", first.shown), false, 1, 11)).unwrap();
    j.write(res("h2")).unwrap();
    drop(j);
    fs::remove_file(&first.path).unwrap();
    let mut r = f.resume(Some(&id)).unwrap();
    assert_eq!(r.journal.spill(&f.tmp.0).path, f.state.join(format!("spill/{id}-3.log")));
}

#[test]
fn state_dir_creates_and_sweeps() {
    let tmp = Tmp::new();
    let state = tmp.0.join("a/b/henki");
    state_dir(&state).unwrap();
    for d in [tmp.0.join("a"), tmp.0.join("a/b"), state.clone(), state.join("sessions"), state.join("spill")] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    let (old, new, session) = (state.join("spill/a-1.log"), state.join("spill/b-1.log"), state.join("sessions/s.jsonl"));
    for (p, age) in [(&old, 25), (&new, 23), (&session, 25)] {
        fs::write(p, "x").unwrap();
        set_mtime(p, age * 3600);
    }
    state_dir(&state).unwrap();
    assert_eq!((old.exists(), new.exists(), session.exists()), (false, true, true), "only spill files older than 24 h go");
}

/// Random journals of records and record-like lines (wrong types, missing fields, duplicate ids, junk bytes) never panic
/// any reader, nor `Journal::resume` over the same bytes as a file; the counters show the inputs reach the history, the
/// repairs, the elision and both ends of resume.
#[test]
fn journal_line_never_panics() {
    let mut x = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    };
    let kinds = ["session", "user", "assistant", "assistant", "tool_result", "tool_result", "elide", "close", "notice", "x"];
    let keys = ["id", "id", "ids", "msg", "msg", "content", "content", "bytes", "dialect", "name", "model", "tools", "cwd"];
    let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
    let values = [
        "null",
        "0",
        "-3",
        "1e400",
        "18446744073709551616",
        "true",
        "\"\"",
        "\"h1\"",
        "\"h2\"",
        "\"h7\"",
        "\"out\"",
        "\"openai\"",
        "\"[output elided: \"",
        "[]",
        "{}",
        "[\"h1\",\"h2\",5]",
        "{\"tool_calls\":[{\"id\":\"h1\",\"name\":\"shell\"},{\"id\":\"h2\"},{\"id\":7},5]}",
        "{\"tool_calls\":[{\"id\":\"h7\",\"name\":3}]}",
        "{\"tool_calls\":{}}",
        "[{\"id\":\"h2\"}]",
        "\"\\u00e9\\ud800\"",
        deep.as_str(),
    ];
    let f = fx();
    let session = f.plant("fuzz", &[]);
    let (mut read, mut repaired, mut elided, mut resumed, mut refused) = (0, 0, 0, 0, 0);
    for _ in 0..10_000 {
        let mut buf = Vec::new();
        if next(4) != 0 {
            buf.extend(format!("{}\n", rec::session(&f.cfg, "x", &f.cwd, false)).bytes());
        }
        for _ in 0..next(12) {
            match next(25) {
                0 => buf.extend((0..next(40)).map(|_| [b'{', b'}', b'"', b'\\', 0xff, 0xc3, b'a', b':', b','][next(9)])),
                1 => buf.extend(format!("{}", json!({"bytes": next(4000), "t": "close"})).bytes()),
                2..=13 => {
                    let ids = ["h1", "h2", "h7", ""];
                    let r = match next(4) {
                        0 => rec::user("u"),
                        1 => asst(&(0..next(3)).map(|_| ids[next(4)]).collect::<Vec<_>>()),
                        2 => rec::tool_result(ids[next(4)], "shell", "out", false, 1, 3),
                        _ => rec::elide(&[ids[next(4)].into()]),
                    };
                    buf.extend(r.to_string().bytes());
                }
                _ => {
                    let fields: Vec<String> = (0..next(5)).map(|_| format!(",\"{}\":{}", keys[next(keys.len())], values[next(values.len())])).collect();
                    let t = if next(10) == 0 { values[next(values.len())].to_string() } else { format!("\"{}\"", kinds[next(kinds.len())]) };
                    buf.extend(format!("{{\"t\":{t}{}}}", fields.concat()).bytes());
                }
            }
            buf.push(b'\n');
        }
        fs::write(&session, &buf).unwrap();
        if let Ok(mut hist) = Journal::new(&f.state).read("fuzz").map_err(port::outcome).and_then(|mut t| replay(&mut t, &f.cfg)) {
            read += 1;
            repaired += usize::from(!dangling(&hist).is_empty());
            let _ = (pairing(&hist), next_h(&hist));
            elided += usize::from(!elide(&mut hist).is_empty());
        }
        // One in ten goes through the whole file pipeline as well, with a torn tail now and then.
        if next(10) == 0 {
            fs::write(&session, &buf[..buf.len() - next(buf.len().min(30) + 1)]).unwrap();
            match resume_at(&f.state, &f.cwd, Some("fuzz"), &f.cfg) {
                Ok(r) => {
                    let _ = (pairing(&r.hist), next_h(&r.hist));
                    resumed += 1;
                }
                Err(_) => refused += 1,
            }
        }
    }
    assert!(read > 1000 && repaired > 100 && elided > 20, "read {read}, repaired {repaired}, elided {elided}");
    assert!(resumed > 100 && refused > 100, "resumed {resumed}, refused {refused}");
}

/// `create` with a hint: the id when `port::session_id` accepts it and no scope has it (`exists` when one does); a
/// hint it refuses gets a new id as without one.
#[test]
fn journal_create_hint() {
    let f = fx();
    let mut h = Journal::new(&f.state);
    assert_eq!(h.create(&f.cwd, Some("imported.1")).unwrap(), "imported.1");
    assert!(f.state.join("sessions").join(scope(&f.cwd).unwrap()).join("imported.1.jsonl").is_file());
    let other = f.tmp.0.join("other");
    fs::create_dir(&other).unwrap();
    let e = Journal::new(&f.state).create(&other, Some("imported.1")).unwrap_err();
    assert_eq!((e.code, e.message.as_str()), ("exists", "session imported.1 exists"));
    for bad in ["", ".x", "-x", "a/b", "a b", &"x".repeat(65)] {
        let id = Journal::new(&f.state).create(&f.cwd, Some(bad)).unwrap();
        assert_eq!(id.len(), 21, "{bad:?}: {id}");
    }
    assert!(port::session_id(&"x".repeat(64)) && port::session_id("a.b_c-Dz9"));
}

/// Every `close` gives its own offset, in an earlier segment too.
#[test]
fn journal_every_close_checked() {
    let f = fx();
    let (id, path) = f.closed(&[rec::user("a")]);
    let full = fs::read(&path).unwrap();
    let lines: Vec<&[u8]> = full.split_inclusive(|&b| b == b'\n').collect();
    let next = format!("{}\n", rec::session(&f.cfg, &id, &f.cwd, true));
    // A line removed before the first close, and a new segment after it: today's last segment is fine.
    fs::write(&path, [lines[0], lines[2], next.as_bytes()].concat()).unwrap();
    assert_eq!(journal_err("tampered", f.resume(Some(&id))), format!("journal modified outside henki: {}", path.display()));
}

/// An unheld read takes no lock and changes nothing: a torn tail is skipped, not cut; a held one is cut.
#[test]
fn journal_read_unheld() {
    let f = fx();
    let (id, path) = f.session(&[rec::user("a")]);
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"t\":\"us").unwrap();
    let before = fs::read(&path).unwrap();
    let _held = hold(&path);
    let t = Journal::new(&f.state).read(&id).unwrap();
    assert_eq!(t.records.iter().map(|r| r["t"].as_str().unwrap()).collect::<Vec<_>>(), ["session", "user"]);
    assert_eq!((t.notices.len(), t.place), (0, path.display().to_string()));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(matches!(Journal::new(&f.state).read("none"), Err(PortError { code: "not_found", .. })));
}
