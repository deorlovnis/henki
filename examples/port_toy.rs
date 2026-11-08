#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! A port binding to test henki against, and the smallest whole `history/1` binding in Rust: `port_toy [history
//! [FAULT...]]` speaks `port/1` on stdin and stdout and keeps each session as `STATE/ID.jsonl` (one record per line,
//! fsync per append) with its scope in `STATE/ID.scope`, STATE being the dir `hello` names. A session is held by a
//! lock on its file (`File::try_lock`), which the system drops when the process ends, SIGKILL included. Its pid goes
//! into `STATE/pid`.
//!
//! `port_toy policy MODE [FAULT...]` is a `policy/1` binding with cap `record` that answers every `decide` the same
//! way: MODE `allow`, `ask`, `ask-always` (ask, offering always), `deny`, or `maybe` (a decision no contract has). Each
//! `decide`'s params go as one line into `STATE/decide.log`, each `record`'s into `STATE/record.log`.
//!
//! Faults (each one is how a broken binding behaves; tests run them through a wrapper script, because henki starts a
//! binding with no arguments):
//!   --exit-at M     exits (status 3) when request M comes        --hang-at M     never answers M (M#K: the Kth M)
//!   --garbage-at M  answers M with a line that is not JSON       --huge-at M     answers M with a 17 MiB line
//!   --error-at M    answers M with error `io`                    --slow MS       waits MS before each answer
//!   --wrong-id      answers with another id                      --no-read       reads nothing after hello
//!   --no-durable    keeps appends in memory until bye            --bad-id        names sessions `../x`
//!   --daemon MARK   leaves `sleep MARK` running                  --stderr-flood  writes 8 MiB to stderr first
//!   --chatty        prints a banner before anything else        --name NAME     says NAME in hello
//!   --fork          has cap `fork` (a copy of the parent's first lines)
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Default)]
struct Opts {
    at: HashMap<String, String>, // fault -> method
    slow: u64,
    wrong_id: bool,
    no_read: bool,
    no_durable: bool,
    bad_id: bool,
    fork: bool,
    name: Option<String>,
    policy: Option<String>, // the MODE of a policy binding
}

struct Held {
    id: String,
    file: File,
    fresh: bool,
    gone: bool,
}

struct Toy {
    o: Opts,
    state: PathBuf,
    held: Option<Held>,
    later: Vec<(String, String)>, // --no-durable: (session, line) not written yet
}

fn main() {
    // henki runs a binding with no arguments: then it is a plain history binding.
    let (mut args, mut o) = (std::env::args().skip(1).peekable(), Opts::default());
    match args.next().as_deref() {
        None | Some("history") => {}
        Some("policy") => o.policy = args.next(),
        Some(_) => panic!("usage: port_toy [history [FAULT...] | policy MODE [FAULT...]]"),
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "--exit-at" | "--hang-at" | "--garbage-at" | "--huge-at" | "--error-at" => _ = o.at.insert(a, args.next().unwrap()),
            "--slow" => o.slow = args.next().unwrap().parse().unwrap(),
            "--wrong-id" => o.wrong_id = true,
            "--no-read" => o.no_read = true,
            "--no-durable" => o.no_durable = true,
            "--bad-id" => o.bad_id = true,
            "--fork" => o.fork = true,
            "--name" => o.name = args.next(),
            "--daemon" => std::mem::forget(std::process::Command::new("sleep").arg(args.next().unwrap()).spawn().unwrap()), // left behind on purpose
            "--stderr-flood" => (0..128).for_each(|_| _ = std::io::stderr().write_all(&[b'e'; 64 << 10])),
            "--chatty" => println!("port_toy: a history binding for tests"),
            f => panic!("unknown fault {f}"),
        }
    }
    let mut t = Toy { o, state: PathBuf::new(), held: None, later: Vec::new() };
    let (stdin, mut seen) = (std::io::stdin(), HashMap::<String, u32>::new());
    for line in stdin.lock().lines() {
        let q: Value = serde_json::from_str(&line.unwrap()).unwrap_or_default();
        let (id, method) = (q["id"].clone(), q["method"].as_str().unwrap_or_default().to_string());
        std::thread::sleep(Duration::from_millis(t.o.slow));
        let k = *seen.entry(method.clone()).and_modify(|n| *n += 1).or_insert(1);
        let fault = |f: &str| t.o.at.get(f).is_some_and(|m| *m == method || *m == format!("{method}#{k}"));
        if fault("--exit-at") {
            std::process::exit(3);
        }
        if fault("--hang-at") {
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        let out = if fault("--garbage-at") {
            "this is not JSON".to_string()
        } else if fault("--huge-at") {
            format!("{{\"id\":{id},\"result\":\"{}\"}}", "x".repeat(17 << 20))
        } else {
            let a = if fault("--error-at") { Err(("io", "a fault".into())) } else { t.answer(&method, &q["params"]) };
            let id = if t.o.wrong_id { json!(id.as_u64().unwrap_or(0) + 1000) } else { id };
            match a {
                Ok(r) => json!({"id": id, "result": r}).to_string(),
                Err((code, msg)) => json!({"id": id, "error": {"code": code, "message": msg}}).to_string(),
            }
        };
        let mut so = std::io::stdout().lock();
        so.write_all(format!("{out}\n").as_bytes()).unwrap();
        so.flush().unwrap();
        if method == "bye" {
            t.flush_later();
            std::process::exit(0);
        }
        if method == "hello" && t.o.no_read {
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    }
    t.flush_later();
}

type Answer = Result<Value, (&'static str, String)>;

impl Toy {
    fn path(&self, id: &str, ext: &str) -> PathBuf {
        self.state.join(format!("{id}.{ext}"))
    }

    fn valid(id: &str) -> bool {
        (1..=64).contains(&id.len()) && !id.starts_with(['.', '-']) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }

    fn named(&self, id: &str) -> Value {
        json!({"session": if self.o.bad_id { "../x" } else { id }})
    }

    /// A policy binding's answer: the same decision every time, each request logged.
    fn policy(&mut self, mode: &str, method: &str, p: &Value) -> Answer {
        let log = |f: &str| OpenOptions::new().create(true).append(true).open(self.state.join(f)).unwrap().write_all(format!("{p}\n").as_bytes()).unwrap();
        match method {
            "hello" => {
                self.state = PathBuf::from(p["state"].as_str().unwrap_or("."));
                fs::create_dir_all(&self.state).map_err(|e| ("io", e.to_string()))?;
                fs::write(self.state.join("pid"), std::process::id().to_string()).map_err(|e| ("io", e.to_string()))?;
                let name = self.o.name.clone().unwrap_or("port_toy".into());
                Ok(json!({"caps": ["record"], "impl": "0.1.0", "name": name, "protocol": 1, "version": 1}))
            }
            "decide" => {
                log("decide.log");
                Ok(match mode {
                    "allow" => json!({"decision": "allow", "reason": "port_toy allows every call"}),
                    "ask" => json!({"decision": "ask"}),
                    "ask-always" => json!({"decision": "ask", "always": true}),
                    "deny" => json!({"decision": "deny", "reason": "port_toy denies every call"}),
                    m => json!({"decision": m}),
                })
            }
            "record" => Ok((log("record.log"), json!({"shown": "kept by port_toy"})).1),
            "bye" => Ok(Value::Null),
            m => Err(("unsupported", format!("no method {m}"))),
        }
    }

    fn answer(&mut self, method: &str, p: &Value) -> Answer {
        if !p["versions"].as_array().is_none_or(|v| v.contains(&json!(1))) {
            return Err(("version", "port_toy speaks version 1 only".into()));
        }
        if let Some(mode) = self.o.policy.clone() {
            return self.policy(&mode, method, p);
        }
        match method {
            "hello" => {
                self.state = PathBuf::from(p["state"].as_str().unwrap_or("."));
                fs::create_dir_all(&self.state).map_err(|e| ("io", e.to_string()))?;
                fs::write(self.state.join("pid"), std::process::id().to_string()).map_err(|e| ("io", e.to_string()))?;
                let caps: &[&str] = if self.o.fork { &["fork"] } else { &[] };
                Ok(json!({"caps": caps, "impl": "0.1.0", "name": self.o.name.clone().unwrap_or("port_toy".into()), "protocol": 1, "version": 1}))
            }
            "create" => {
                let id = match p["hint"].as_str() {
                    Some(h) if !Toy::valid(h) => return Err(("invalid", format!("{h} is not a session id"))),
                    Some(h) if self.path(h, "jsonl").exists() => return Err(("exists", format!("session {h} exists"))),
                    Some(h) => h.to_string(),
                    None => format!("toy-{}-{}", std::process::id(), UNIX_EPOCH.elapsed().unwrap().as_nanos()),
                };
                let file = OpenOptions::new().create_new(true).append(true).read(true).open(self.path(&id, "jsonl")).map_err(|e| ("io", e.to_string()))?;
                file.try_lock().map_err(|e| ("io", e.to_string()))?;
                fs::write(self.path(&id, "scope"), p["scope"].as_str().unwrap_or_default()).map_err(|e| ("io", e.to_string()))?;
                self.held = Some(Held { id: id.clone(), file, fresh: true, gone: false });
                Ok(self.named(&id))
            }
            "open" => {
                self.held = None;
                let ids = match p["session"].as_str() {
                    Some(id) if Toy::valid(id) && self.path(id, "jsonl").exists() => vec![id.to_string()],
                    Some(id) => return Err(("not_found", format!("no session {id}"))),
                    None => self.list(p["scope"].as_str().unwrap_or_default()).into_iter().map(|(id, _)| id).collect(),
                };
                let mut busy = false;
                for id in ids {
                    let file = OpenOptions::new().append(true).read(true).open(self.path(&id, "jsonl")).map_err(|e| ("io", e.to_string()))?;
                    if file.try_lock().is_ok() {
                        self.held = Some(Held { id: id.clone(), file, fresh: false, gone: false });
                        return Ok(self.named(&id));
                    }
                    busy = true;
                }
                Err(if busy { ("busy", "every session is in use".into()) } else { ("not_found", "no session in this scope".into()) })
            }
            "read" => {
                let id = p["session"].as_str().unwrap_or_default();
                let text = Toy::valid(id).then(|| fs::read_to_string(self.path(id, "jsonl")).ok()).flatten().ok_or(("not_found", format!("no session {id}")))?;
                let mut lines: Vec<String> = text.lines().map(String::from).collect();
                lines.extend(self.later.iter().filter(|(s, _)| s == id).map(|(_, l)| l.clone()));
                let from = p["from"].as_u64().unwrap_or(0) as usize;
                let records: Vec<Value> = lines.iter().skip(from).map(|l| serde_json::from_str(l).unwrap_or(Value::Null)).collect();
                Ok(json!({"records": records}))
            }
            "append" => {
                let (id, rec) = (p["session"].as_str().unwrap_or_default(), &p["record"]);
                let h = self.held.as_mut().filter(|h| h.id == id).ok_or(("not_found", format!("session {id} is not held")))?;
                if !rec.is_object() {
                    return Err(("invalid", "a record is a JSON object".into()));
                }
                if h.gone {
                    return Ok(Value::Null);
                }
                if self.o.no_durable {
                    self.later.push((id.to_string(), rec.to_string()));
                    return Ok(Value::Null);
                }
                h.file.write_all(format!("{rec}\n").as_bytes()).and_then(|()| h.file.sync_data()).map_err(|e| ("io", e.to_string()))?;
                Ok(Value::Null)
            }
            "discard" => {
                let id = p["session"].as_str().unwrap_or_default().to_string();
                let path = self.path(&id, "jsonl");
                let h = self.held.as_mut().filter(|h| h.id == id).ok_or(("not_found", format!("session {id} is not held")))?;
                let user = fs::read_to_string(&path).unwrap_or_default().contains("\"t\":\"user\"");
                if h.fresh && !user && !h.gone {
                    h.gone = fs::remove_file(&path).is_ok();
                    let _ = fs::remove_file(self.path(&id, "scope"));
                }
                Ok(json!({"removed": self.held.as_ref().is_some_and(|h| h.gone)}))
            }
            "fork" if self.o.fork => {
                let (id, n) = (p["session"].as_str().unwrap_or_default(), p["line"].as_u64().unwrap_or(0) as usize);
                let text = Toy::valid(id).then(|| fs::read_to_string(self.path(id, "jsonl")).ok()).flatten().ok_or(("not_found", format!("no session {id}")))?;
                let lines: Vec<&str> = text.lines().collect();
                if !lines.get(n.wrapping_sub(1)).and_then(|l| serde_json::from_str::<Value>(l).ok()).is_some_and(|r| r["t"] == "user") {
                    return Err(("invalid", format!("line {n} of {id} is not a user record")));
                }
                let made = self.answer("create", &json!({"hint": null, "scope": p["scope"]}))?;
                let h = self.held.as_mut().unwrap();
                h.file
                    .write_all(lines[..n - 1].iter().map(|l| format!("{l}\n")).collect::<String>().as_bytes())
                    .and_then(|()| h.file.sync_data())
                    .map_err(|e| ("io", e.to_string()))?;
                Ok(made)
            }
            "list" => {
                Ok(json!({"sessions": self.list(p["scope"].as_str().unwrap_or_default()).into_iter().map(|(id, t)| json!({"session": id, "updated": t})).collect::<Vec<_>>()}))
            }
            "bye" => Ok(Value::Null),
            m => Err(("unsupported", format!("no method {m}"))),
        }
    }

    /// The sessions of `scope`, newest first: (id, mtime in seconds).
    fn list(&self, scope: &str) -> Vec<(String, u64)> {
        let mut out: Vec<(Duration, String)> = Vec::new();
        for e in fs::read_dir(&self.state).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".scope") else { continue };
            let t = fs::metadata(self.path(id, "jsonl")).and_then(|m| m.modified()).map(|m| m.duration_since(UNIX_EPOCH).unwrap_or_default());
            if let (Ok(t), true) = (t, fs::read_to_string(e.path()).is_ok_and(|s| s == scope)) {
                out.push((t, id.to_string()));
            }
        }
        out.sort();
        out.into_iter().rev().map(|(t, id)| (id, t.as_secs())).collect()
    }

    fn flush_later(&mut self) {
        for (id, l) in std::mem::take(&mut self.later) {
            let _ = OpenOptions::new().append(true).open(self.path(&id, "jsonl")).and_then(|mut f| f.write_all(format!("{l}\n").as_bytes()));
        }
    }
}
