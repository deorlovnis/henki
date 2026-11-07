//! The configuration: one TOML-subset file, two environment variables and the flags, checked so that anything wrong
//! stops henki with the file, the line and the fix.
use crate::{http, sys, term, tools::ALL as TOOLS, types::Url};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::{ffi::OsStr, ffi::OsString, fs, io::ErrorKind, io::Read, net::Ipv6Addr, os::unix::ffi::OsStrExt, os::unix::fs::MetadataExt};

/// A snapshot of the process environment, taken once in `main`; tests build their own with `Env(vec![...])`.
#[derive(Clone, Debug, Default)]
pub struct Env(pub Vec<(OsString, OsString)>);

impl Env {
    pub fn os() -> Env {
        Env(std::env::vars_os().collect())
    }

    pub fn get(&self, name: &str) -> Option<&OsStr> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_os_str())
    }
}

/// The flags of `run` that override the file.
pub struct Flags {
    pub config: Option<PathBuf>,
    pub approve: Option<bool /* auto */>,
    pub model: Option<String>,
    pub max_steps: Option<u32>,
    pub max_seconds: Option<u64>,
}

pub struct Config {
    pub model: String,
    pub url: Url,
    pub url_from: &'static str, // "config" | "HENKI_BASE_URL"
    pub api_key: String,        // "" = none
    pub auto: bool,
    pub approve_from: String,     // "default" | "FILE:LINE" | "--approve"
    pub tools: Vec<&'static str>, // canonical order: shell, read, write, edit
    pub pass_env: Vec<String>,
    pub max_steps: u32,        // default 50
    pub max_seconds: u64,      // 0 = none
    pub tool_timeout_s: u64,   // default 120
    pub output_cap: usize,     // default 16,384
    pub window: u64,           // context_window; 0 becomes 32,768 with a notice
    pub max_tokens: u64,       // default 4,096
    pub file: Option<PathBuf>, // the file read
    pub dir: PathBuf,          // the config dir
    pub explicit: bool,        // --config given
    pub hash: Option<String>,  // FNV-1a of the file bytes
    pub home: PathBuf,
    pub cwd: PathBuf,           // the session cwd (`load`'s argument)
    pub env: Env,               // the snapshot `load` read; the child environment is built from it
    pub root: PathBuf,          // the project root of the cwd
    pub user_path: String,      // the inherited PATH without relative entries
    pub bash: PathBuf,          // the first executable `bash` on an absolute user PATH entry
    pub agents: Option<String>, // <config dir>/AGENTS.md, trailing newlines trimmed
    pub agents_hash: Option<String>,
    pub protected: Vec<PathBuf>, // the config file and dir, each also as its realpath (main adds the state dir)
    pub notices: Vec<String>,
}

/// The HENKI_* variables henki reads or sets for its children; any other gives a notice.
const KNOWN: [&str; 6] = ["HENKI_MODEL", "HENKI_BASE_URL", "HENKI_DEPTH", "HENKI_SESSION", "HENKI_USER_PATH", "HENKI_VERSION"];
const KEYS: &str = "model, base_url, dialect, api_key, approve, tools, ext, pass_env, max_steps, max_seconds, tool_timeout_s, output_cap, context_window, max_tokens";
/// The integer keys and their ranges; `context_window` also takes 0 (unknown).
#[rustfmt::skip]
const INTS: [(&str, u64, u64); 6] = [("max_steps", 1, 10_000), ("max_seconds", 0, 604_800), ("tool_timeout_s", 1, 600), ("output_cap", 1_024, 262_144),
    ("context_window", 1_024, 2_147_483_647), ("max_tokens", 1, 1_000_000)];
const WS: [char; 2] = [' ', '\t'];
const NO_CONFIG: &str = "no config; copy one: mkdir -p ~/.config/henki && cp $(brew --prefix)/share/henki/config.mac.toml ~/.config/henki/config.toml";

/// Defaults, then one file, then HENKI_MODEL and HENKI_BASE_URL, then the flags. Err: "FILE:LINE: PROBLEM; fix: ..."
/// (or "PROBLEM; fix: ..." when no file is involved); `main` turns it into exit 2 `config`.
pub fn load(f: &Flags, env: &Env, cwd: &Path) -> Result<Config, String> {
    let home = PathBuf::from(env.get("HOME").filter(|h| Path::new(h).is_absolute()).ok_or("HOME is not set; fix: set HOME to your home directory")?);
    let var = |name: &str| env.get(name).filter(|v| !v.is_empty()).map(|v| v.to_str().map(Value::from).ok_or(format!("{name} is not UTF-8; fix: unset it"))).transpose();
    let (env_model, env_url) = (var("HENKI_MODEL")?, var("HENKI_BASE_URL")?);
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    // A relative --config is taken from the cwd, a real path (getcwd's), so each leading `..` drops one of its names.
    let up = f.config.as_ref().map_or(0, |p| p.components().take_while(|c| *c == Component::ParentDir).count());
    let base = cwd.ancestors().nth(up).unwrap_or(Path::new("/"));
    let file = f.config.as_ref().map_or_else(|| home.join(".config/henki/config.toml"), |p| base.join(p.components().skip(up).collect::<PathBuf>()));
    let (dir, explicit, root) = (file.parent().unwrap_or(&home).to_path_buf(), f.config.is_some(), project_root(cwd, &real(&home)));
    #[rustfmt::skip]
    let mut c = Config { model: String::new(), url: Url { host: String::new(), port: 80, path: String::new() }, url_from: "config", api_key: String::new(),
        auto: false, approve_from: "default".into(), tools: TOOLS.to_vec(), pass_env: Vec::new(), max_steps: 50, max_seconds: 0, tool_timeout_s: 120,
        output_cap: 16_384, window: 0, max_tokens: 4_096, file: None, dir, explicit, hash: None, home, cwd: cwd.to_path_buf(), env: env.clone(), root,
        user_path: String::new(), bash: PathBuf::new(), agents: None, agents_hash: None, protected: Vec::new(), notices: Vec::new() };
    // Each layer is (where, key, value): the file's lines, then the two variables, then the flags; the last one wins.
    let mut layers: Vec<(String, String, Value)> = Vec::new();
    if explicit || file.exists() {
        let text = read(&file, 64 << 10)?;
        // Named through the project (even by a link out of it) or stored in it, the project could choose the config.
        if real(&c.root) != real(&c.home) && c.root != Path::new("/") && (file.starts_with(&c.root) || real(&file).starts_with(real(&c.root))) {
            let fix = "fix: keep the config outside the project, in ~/.config/henki, and name it by a path outside the project";
            return Err(format!("{} is inside the project {} or named through it; {fix}", file.display(), c.root.display()));
        }
        let lines = parse(&text).map_err(|e| format!("{}:{e}", file.display()))?;
        layers.extend(lines.into_iter().map(|(n, k, v)| (format!("{}:{n}", file.display()), k, v)));
        (c.hash, c.file) = (Some(sys::fnv(text.as_bytes())), Some(file.clone()));
    } else if env_model.is_none() || env_url.is_none() {
        return Err(NO_CONFIG.into());
    }
    fs::metadata(&c.dir).map_or(Ok(()), |m| owner_mode(&c.dir, m.mode(), m.uid(), sys::euid(), true))?;
    let approve = f.approve.map(|a| Value::from(if a { "auto" } else { "ask" }));
    let late = [
        ("HENKI_MODEL", "model", env_model),
        ("HENKI_BASE_URL", "base_url", env_url),
        ("--model", "model", f.model.clone().map(Value::from)),
        ("--approve", "approve", approve),
        ("--max-steps", "max_steps", f.max_steps.map(Value::from)),
        ("--max-seconds", "max_seconds", f.max_seconds.map(Value::from)),
    ];
    layers.extend(late.into_iter().filter_map(|(at, k, v)| Some((at.to_string(), k.to_string(), v?))));
    for (at, k, v) in &layers {
        set(&mut c, k, v).map_err(|e| format!("{at}: {e}"))?;
        if k == "approve" {
            c.approve_from.clone_from(at);
        }
        if at == "HENKI_BASE_URL" && !http::loopback_literal(&c.url.host) {
            return Err(format!("HENKI_BASE_URL: {} is not a loopback host; fix: use localhost, 127.0.0.1 or [::1], or set base_url in the config", c.url.host));
        }
    }
    let at = |key: &str| layers.iter().rfind(|l| l.1 == key);
    c.url_from = if at("base_url").is_some_and(|l| l.0 == "HENKI_BASE_URL") { "HENKI_BASE_URL" } else { "config" };
    if c.model.is_empty() || c.url.host.is_empty() {
        let key = if c.model.is_empty() { "model" } else { "base_url" };
        return Err(format!("no {key}; fix: set {key} in {}", file.display()));
    }
    if let Some((at, ..)) = at("api_key").filter(|_| !c.api_key.is_empty() && !http::loopback_literal(&c.url.host)) {
        return Err(format!("{at}: api_key is sent only to a loopback host, not to {}; fix: remove api_key or use a loopback base_url", c.url.host));
    }
    let unknown = env.0.iter().filter_map(|(k, _)| k.to_str()).filter(|k| k.starts_with("HENKI_") && !KNOWN.contains(k));
    c.notices.extend(unknown.map(|k| format!("ignoring {k}: henki does not read it")));
    let listed = at("ext");
    let paths = listed.map_or_else(|| vec![c.dir.join("ext")], |l| l.2.as_array().into_iter().flatten().filter_map(Value::as_str).map(|p| expand(p, &c.home)).collect());
    let from = listed.map(|l| l.0.clone()).or_else(|| c.file.as_ref().map(|f| f.display().to_string()));
    for p in paths {
        ext(&p, listed.is_none() && !c.explicit).map_err(|e| from.as_ref().map_or_else(|| e.clone(), |at| format!("{at}: {e}")))?;
    }
    let dirs: Vec<String> = std::env::split_paths(env.get("PATH").unwrap_or_default()).filter(|p| p.is_absolute()).filter_map(|p| p.into_os_string().into_string().ok()).collect();
    c.user_path = dirs.join(":");
    let exe = |b: &PathBuf| fs::metadata(b).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0);
    c.bash = dirs.iter().map(|d| Path::new(d).join("bash")).find(exe).ok_or("bash is not on PATH; fix: install bash, or add its directory to PATH")?;
    let agents = c.dir.join("AGENTS.md");
    if agents.exists() {
        let text = read(&agents, 4 << 10)?;
        if let Some((i, ch)) = text.split('\n').enumerate().find_map(|(i, l)| Some((i, l.chars().find(|&ch| term::escaped(ch))?))) {
            return Err(format!("{}:{}: holds U+{:04X}, which a terminal does not show; fix: remove it", agents.display(), i + 1, ch as u32));
        }
        (c.agents_hash, c.agents) = (Some(sys::fnv(text.as_bytes())), Some(text.trim_end_matches('\n').to_string()).filter(|a| !a.is_empty()));
    }
    if c.window == 0 {
        c.window = 32_768;
        c.notices.push("context_window is unknown; using 32,768 tokens".into());
    }
    c.protected = c.file.iter().chain([&c.dir]).flat_map(|p| [Some(p.clone()), fs::canonicalize(p).ok().filter(|r| r != p)]).flatten().collect();
    Ok(c)
}

/// One key into `c`. Err: "PROBLEM; fix: ...".
fn set(c: &mut Config, k: &str, v: &Value) -> Result<(), String> {
    let s = || v.as_str().ok_or(format!("{k} must be a string; fix: write it in double quotes"));
    let list = || v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>()).ok_or(format!("{k} must be a list; fix: write {k} = [\"...\"]"));
    match k {
        "model" | "base_url" if s()?.is_empty() => return Err(format!("{k} is empty; fix: set {k}")),
        "model" => c.model = s()?.to_string(),
        "base_url" => c.url = parse_url(s()?).map_err(|e| format!("{e}; fix: use http://HOST:PORT/PATH"))?,
        "dialect" if s()? == "anthropic" => return Err("dialect anthropic is not supported by this build; fix: use dialect = \"openai\" with an OpenAI-compatible server".into()),
        "dialect" if s()? != "openai" => return Err(format!("unknown dialect {}; fix: use dialect = \"openai\"", s()?)),
        "dialect" => {}
        "api_key" if !s()?.bytes().all(|b| b.is_ascii_graphic()) => return Err("api_key holds a space or a byte outside printable ASCII; fix: correct the key".into()),
        "api_key" => c.api_key = s()?.to_string(),
        "approve" if !["ask", "auto"].contains(&s()?) => return Err(format!("approve is \"ask\" or \"auto\", not {:?}; fix: use one of them", s()?)),
        "approve" => c.auto = s()? == "auto",
        "tools" => c.tools = tools(&list()?)?,
        "ext" => drop(list()?), // checked once the config dir and the flags are known
        "pass_env" => c.pass_env = pass_env(&list()?, c.explicit)?,
        _ => {
            let Some(&(_, lo, hi)) = INTS.iter().find(|i| i.0 == k) else { return Err(format!("unknown key {k}; fix: remove it (this build knows {KEYS})")) };
            let n = v.as_u64().ok_or(format!("{k} must be an integer; fix: write it without quotes"))?;
            let zero = if k == "context_window" { ", or 0 when unknown" } else { "" };
            if !(lo..=hi).contains(&n) && (n > 0 || zero.is_empty()) {
                return Err(format!("{k} = {n} is out of range; fix: use {} to {}{zero}", term::num(lo), term::num(hi)));
            }
            match k {
                "max_steps" => c.max_steps = n as u32,
                "max_seconds" => c.max_seconds = n,
                "tool_timeout_s" => c.tool_timeout_s = n,
                "output_cap" => c.output_cap = n as usize,
                "context_window" => c.window = n,
                _ => c.max_tokens = n,
            }
        }
    }
    Ok(())
}

/// A non-empty set of the built-in tools without repeats, in canonical order.
fn tools(l: &[&str]) -> Result<Vec<&'static str>, String> {
    let fix = "fix: list some of shell, read, write, edit, each once";
    if let Some((_, t)) = l.iter().enumerate().find(|&(i, t)| !TOOLS.contains(t) || l[..i].contains(t)) {
        return Err(format!("tool {t} is {}; {fix}", if TOOLS.contains(t) { "listed twice" } else { "unknown" }));
    }
    let t: Vec<_> = TOOLS.into_iter().filter(|t| l.contains(t)).collect();
    if t.is_empty() { Err(format!("tools is empty; {fix}")) } else { Ok(t) }
}

/// Names a child may inherit: valid names, never PATH or HENKI_*, and credential-shaped ones only from `--config`.
fn pass_env(l: &[&str], explicit: bool) -> Result<Vec<String>, String> {
    for n in l {
        if !n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(format!("pass_env name {n:?} is not a variable name; fix: use letters, digits and _"));
        }
        if *n == "PATH" || n.starts_with("HENKI_") {
            return Err(format!("pass_env cannot hold {n}; fix: remove it (henki sets it for its children)"));
        }
        let upper = n.to_ascii_uppercase();
        if !explicit && ["KEY", "TOKEN", "SECRET", "PASS", "CREDENTIAL"].iter().any(|w| upper.contains(w)) {
            return Err(format!("pass_env {n} looks like a credential; fix: pass it only from a config given with --config"));
        }
    }
    Ok(l.iter().map(|n| n.to_string()).collect())
}

/// An ext path must be an absolute, readable directory holding nothing but dot entries: this build has no extensions.
fn ext(p: &Path, optional: bool) -> Result<(), String> {
    let d = p.display();
    if !p.is_absolute() {
        return Err(format!("ext path {d} is relative; fix: use an absolute path or ~/"));
    }
    let names = match fs::read_dir(p) {
        Err(e) if optional && e.kind() == ErrorKind::NotFound => return Ok(()),
        r => r.map_err(|e| format!("ext {d}: {e}; fix: make it a readable directory, or set ext to readable ones"))?,
    };
    match names.filter_map(|e| Some(e.ok()?.file_name())).find(|n| !n.as_bytes().starts_with(b".")) {
        Some(n) => Err(format!("ext {d}: {0}: extensions are not supported by this build; fix: move {0} out of {d}", n.to_string_lossy())),
        None => Ok(()),
    }
}

/// A UTF-8 regular file of at most `max` bytes, opened without blocking on a FIFO, that no other user can write (the
/// config file and AGENTS.md: both steer what henki runs).
fn read(path: &Path, max: u64) -> Result<String, String> {
    let fail = |e: String| format!("{e}; fix: make {} a readable regular file", path.display());
    let (f, _) = sys::open_regular(path).map_err(fail)?;
    let mut b = Vec::new();
    let m = f.metadata().and_then(|m| (&f).take(max + 1).read_to_end(&mut b).map(|_| m)).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    if b.len() as u64 > max {
        return Err(format!("{}: over {} bytes; fix: shorten it", path.display(), term::num(max)));
    }
    owner_mode(path, m.mode(), m.uid(), sys::euid(), false)?;
    String::from_utf8(b).map_err(|_| format!("{}: not UTF-8; fix: save it as UTF-8", path.display()))
}

/// The config file (`dir` false) may be owned by the euid or root, its directory only by the euid; neither may be
/// group- or world-writable. Pure, so a test can give it another owner.
fn owner_mode(path: &Path, mode: u32, uid: u32, euid: u32, dir: bool) -> Result<(), String> {
    let p = path.display();
    if uid != euid && (dir || uid != 0) {
        Err(format!("{p} is owned by uid {uid}, not by you (uid {euid}); fix: use a {} you own", if dir { "directory" } else { "file" }))
    } else if mode & 0o022 != 0 {
        Err(format!("{p} is group- or world-writable; fix: chmod go-w {p}"))
    } else {
        Ok(())
    }
}

/// The TOML subset: (line, key, value) per assignment; Err("LINE: PROBLEM").
pub fn parse(text: &str) -> Result<Vec<(usize, String, Value)>, String> {
    let mut out: Vec<(usize, String, Value)> = Vec::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    for (i, line) in text.split('\n').enumerate() {
        let err = |p: String| format!("{}: {p}", i + 1);
        if line.contains(|c: char| c.is_control() && c != '\t') {
            return Err(err("a raw control character; fix: remove it, or write it as an escape inside a string".into()));
        }
        let l = line.trim_start_matches(WS);
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, v) = l.split_once('=').ok_or_else(|| err("expected key = value; fix: write key = value (no tables, no multi-line values)".into()))?;
        let k = k.trim_end_matches(WS);
        if !k.starts_with(|c: char| c.is_ascii_lowercase()) || !k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(err(format!("invalid key {k:?}; fix: keys use a-z, 0-9 and _")));
        }
        if let Some(first) = out.iter().find(|e| e.1 == k) {
            return Err(err(format!("duplicate key {k} (first on line {}); fix: keep one of them", first.0)));
        }
        let end = |r: &str| r.trim_start_matches(WS).is_empty() || r.trim_start_matches(WS).starts_with('#');
        let fix = "invalid value; fix: use \"text\", an integer from 0 to 2147483647, true, false or [\"a\", \"b\"] on one line";
        let (v, _) = value(v.trim_start_matches(WS)).filter(|(_, r)| end(r)).ok_or_else(|| err(fix.into()))?;
        out.push((i + 1, k.to_string(), v));
    }
    Ok(out)
}

/// A value and the text after it: a scalar, or a one-line array of strings with one optional trailing comma.
fn value(s: &str) -> Option<(Value, &str)> {
    let Some(mut s) = s.strip_prefix('[') else { return scalar(s) };
    let mut items = Vec::new();
    loop {
        s = s.trim_start_matches(WS);
        if let Some(rest) = s.strip_prefix(']') {
            return Some((Value::Array(items), rest));
        }
        let (v, rest) = scalar(s).filter(|(v, _)| v.is_string())?;
        items.push(v);
        let rest = rest.trim_start_matches(WS);
        s = rest.strip_prefix(',').or_else(|| rest.starts_with(']').then_some(rest))?;
    }
}

/// A JSON string (its escapes keep their JSON meaning), `true`, `false` or an integer from 0 to 2^31-1 (JSON's grammar
/// without a sign, fraction or exponent), and the text after it. A `#` ends a bare value: `7#c` is 7 and a comment.
fn scalar(s: &str) -> Option<(Value, &str)> {
    let bare = s.get(..s.find('#').filter(|_| !s.starts_with('"')).unwrap_or(s.len()))?;
    let mut it = serde_json::Deserializer::from_str(bare).into_iter::<Value>();
    let v = it.next()?.ok().filter(|v| v.is_string() || v.is_boolean() || (!s.starts_with('-') && v.as_u64().is_some_and(|n| n <= 2_147_483_647)))?;
    Some((v, s.get(it.byte_offset()..)?))
}

/// `http://HOST[:PORT]/PATH`: http only (`https is not supported in henki v1`), no userinfo, no query, one trailing `/` stripped.
pub fn parse_url(s: &str) -> Result<Url, String> {
    if s.starts_with("https://") {
        return Err("https is not supported in henki v1".into());
    }
    let bad = |why: &str| format!("{s} {why}");
    let rest = s.strip_prefix("http://").ok_or_else(|| bad("is not an http:// URL"))?;
    let (auth, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if auth.contains('@') || rest.contains(['?', '#']) {
        return Err(bad("holds a user name, password, query or fragment"));
    }
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => (h, p.parse::<u16>().ok().filter(|&n| n > 0 && p.bytes().all(|b| b.is_ascii_digit()))),
        _ => (auth, Some(80)),
    };
    let v6 = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    let named = v6.map_or(!host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'), |h| h.parse::<Ipv6Addr>().is_ok());
    let path = path.strip_suffix('/').unwrap_or(path);
    match port {
        Some(port) if named && path.bytes().all(|b| b.is_ascii_graphic()) => Ok(Url { host: v6.unwrap_or(host).into(), port, path: path.into() }),
        _ => Err(bad("is not http://HOST[:PORT]/PATH")),
    }
}

/// The nearest ancestor of `cwd` holding `.git` (file or dir), checking `home` and stopping after it, or at `/`; else `cwd`.
pub fn project_root(cwd: &Path, home: &Path) -> PathBuf {
    let upto = cwd.ancestors().position(|d| d == home).map_or(usize::MAX, |i| i + 1);
    let git = |d: &&Path| d.join(".git").is_dir() || d.join(".git").is_file();
    cwd.ancestors().take(upto).find(git).unwrap_or(cwd).to_path_buf()
}

/// A leading `~/` becomes `home`; relative paths are left alone.
pub fn expand(path: &str, home: &Path) -> PathBuf {
    path.strip_prefix("~/").map_or_else(|| PathBuf::from(path), |r| home.join(r))
}

#[cfg(test)]
impl Config {
    /// A valid `Config` rooted in `dir` (home, cwd, root and config dir), model `m`, base URL `http://127.0.0.1:9/v1`,
    /// `/bin/bash`, defaults elsewhere, no config file and nothing protected. Tests build from it and change fields, so a
    /// new field never breaks them.
    pub fn test(dir: &Path) -> Config {
        Config {
            model: "m".into(),
            url: Url { host: "127.0.0.1".into(), port: 9, path: "/v1".into() },
            url_from: "config",
            api_key: String::new(),
            auto: false,
            approve_from: "default".into(),
            tools: vec!["shell", "read", "write", "edit"],
            pass_env: Vec::new(),
            max_steps: 50,
            max_seconds: 0,
            tool_timeout_s: 120,
            output_cap: 16_384,
            window: 32_768,
            max_tokens: 4_096,
            file: None,
            dir: dir.to_path_buf(),
            explicit: false,
            hash: None,
            home: dir.to_path_buf(),
            cwd: dir.to_path_buf(),
            env: Env::default(),
            root: dir.to_path_buf(),
            user_path: "/usr/local/bin:/usr/bin:/bin".into(),
            bash: PathBuf::from("/bin/bash"),
            agents: None,
            agents_hash: None,
            protected: Vec::new(),
            notices: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
