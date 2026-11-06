//! The configuration: one TOML-subset file, two environment variables and the flags, checked so that anything wrong
//! stops henki with the file, the line and the fix.
use crate::types::Url;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A snapshot of the process environment, taken once in `main`; tests build their own with `Env(vec![...])`.
#[derive(Clone, Debug, Default)]
pub struct Env(pub Vec<(std::ffi::OsString, std::ffi::OsString)>);

impl Env {
    pub fn os() -> Env {
        todo!()
    }

    pub fn get(&self, name: &str) -> Option<&std::ffi::OsStr> {
        todo!()
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

/// Defaults, then one file, then HENKI_MODEL and HENKI_BASE_URL, then the flags. Err: "FILE:LINE: PROBLEM; fix: ..."
/// (or "PROBLEM; fix: ..." when no file is involved); `main` turns it into exit 2 `config`.
pub fn load(f: &Flags, env: &Env, cwd: &Path) -> Result<Config, String> {
    todo!()
}

/// The TOML subset: (line, key, value) per assignment; Err("LINE: PROBLEM").
pub fn parse(text: &str) -> Result<Vec<(usize, String, Value)>, String> {
    todo!()
}

/// `http://HOST[:PORT]/PATH`: http only (`https is not supported in henki v1`), no userinfo, no query, one trailing `/` stripped.
pub fn parse_url(s: &str) -> Result<Url, String> {
    todo!()
}

/// The nearest ancestor of `cwd` holding `.git` (file or dir), checking `home` and stopping after it, or at `/`; else `cwd`.
pub fn project_root(cwd: &Path, home: &Path) -> PathBuf {
    todo!()
}

/// A leading `~/` becomes `home`; relative paths are left alone.
pub fn expand(path: &str, home: &Path) -> PathBuf {
    todo!()
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
