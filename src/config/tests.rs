#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use super::*;
use serde_json::json;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const BASE: &str = "model = \"m\"\nbase_url = \"http://127.0.0.1:8080/v1\"\n";

/// A fresh tree under the temp dir, removed when dropped: `home/.config/henki` (0700), a git `work` dir as the cwd, and
/// `bin/bash` (an executable file) for PATH.
struct T {
    dir: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
    bin: PathBuf,
}

impl T {
    fn new() -> T {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("henki-config-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Its real path, so test paths equal the realpaths `load` derives (on macOS the temp dir sits behind a link).
        let dir = fs::canonicalize(&dir).unwrap();
        let t = T { home: dir.join("home"), cwd: dir.join("work"), bin: dir.join("bin"), dir };
        fs::create_dir_all(t.cfg_dir()).unwrap();
        chmod(&t.cfg_dir(), 0o700);
        fs::create_dir_all(t.cwd.join(".git")).unwrap();
        fs::create_dir_all(&t.bin).unwrap();
        put(&t.bin.join("bash"), "#!/bin/sh\n", 0o755);
        t
    }
    fn cfg_dir(&self) -> PathBuf {
        self.home.join(".config/henki")
    }
    fn cfg(&self) -> PathBuf {
        self.cfg_dir().join("config.toml")
    }
    fn env(&self, extra: &[(&str, &str)]) -> Env {
        let base = [("HOME", self.home.to_str().unwrap()), ("PATH", self.bin.to_str().unwrap())];
        Env(base.iter().chain(extra).map(|(k, v)| (k.into(), v.into())).collect())
    }
    /// HOME and this PATH only.
    fn env_path(&self, path: &str) -> Env {
        Env(vec![("HOME".into(), self.home.clone().into()), ("PATH".into(), path.into())])
    }
    /// The default config file holding `text`, loaded with no flags and the base environment plus `extra`.
    fn load_env(&self, text: &str, extra: &[(&str, &str)]) -> Result<Config, String> {
        put(&self.cfg(), text, 0o600);
        load(&flags(), &self.env(extra), &self.cwd)
    }
    fn load(&self, text: &str) -> Result<Config, String> {
        self.load_env(text, &[])
    }
    fn err(&self, text: &str) -> String {
        self.load(text).err().expect("a load error")
    }
    /// `text` as `DIR/cfg/config.toml` (0700 dir, with the `ext` dir an explicit config needs), loaded with `--config`.
    fn explicit(&self, text: &str) -> Result<Config, String> {
        let d = self.dir.join("cfg");
        fs::create_dir_all(d.join("ext")).unwrap();
        chmod(&d, 0o700);
        put(&d.join("config.toml"), text, 0o600);
        load(&Flags { config: Some(d.join("config.toml")), ..flags() }, &self.env(&[]), &self.cwd)
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn flags() -> Flags {
    Flags::default()
}

/// The extension `name` in the ext path `path`: a `doc.md` and an executable `bin/CMD` per command.
fn add_ext(path: &Path, name: &str, cmds: &[&str]) {
    let dir = path.join(name);
    fs::create_dir_all(dir.join(if cmds.is_empty() { "" } else { "bin" })).unwrap();
    put(
        &dir.join("doc.md"),
        &format!(
            "# {name}: a test tool

How to use it.
"
        ),
        0o644,
    );
    for c in cmds {
        put(
            &dir.join("bin").join(c),
            "#!/bin/sh
",
            0o755,
        );
    }
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

fn put(p: &Path, text: &str, mode: u32) {
    let _ = fs::remove_file(p);
    fs::write(p, text).unwrap();
    chmod(p, mode);
}

fn mkfifo(p: &Path) {
    assert!(std::process::Command::new("mkfifo").arg(p).status().unwrap().success());
}

fn has(err: &str, part: &str) {
    assert!(err.contains(part), "{err:?} lacks {part:?}");
}

fn one(text: &str) -> Value {
    let mut p = parse(text).unwrap();
    assert_eq!(p.len(), 1, "{text:?}");
    p.remove(0).2
}

fn refused(text: &str) {
    assert!(parse(text).is_err(), "{text:?} parsed");
}

#[test]
fn toml_subset_strings() {
    let p = parse("model = \"qwen\"\nempty = \"\"\n  hash = \"a # b = c\"\nutf=\"日本 é\"\n").unwrap();
    let want = [(1, "model", json!("qwen")), (2, "empty", json!("")), (3, "hash", json!("a # b = c")), (4, "utf", json!("日本 é"))];
    assert_eq!(p, want.map(|(n, k, v)| (n, k.to_string(), v)));
}

#[test]
fn toml_subset_escapes() {
    assert_eq!(one(r#"s = "q\"b\\s\nn\tt""#), json!("q\"b\\s\nn\tt"));
    // JSON escapes keep their JSON meaning.
    assert_eq!(one(r#"s = "\u00e9\/\b\f\r""#), json!("é/\u{8}\u{c}\r"));
    for bad in [r#"s = "\e""#, r#"s = "\U0001F600""#, r#"s = "\x41""#, r#"s = "\q""#] {
        refused(bad);
    }
}

#[test]
fn toml_subset_integers() {
    assert_eq!(one("a = 0"), json!(0));
    assert_eq!(one("a = 42"), json!(42));
    assert_eq!(one("a = 2147483647"), json!(2_147_483_647));
    for bad in ["a = 2147483648", "a = 99999999999999999999999", "a = -1", "a = -0", "a = 01", "a = 1.0", "a = 1e3", "a = 0x10", "a = 1_000", "a = +1"] {
        refused(bad);
    }
}

#[test]
fn toml_subset_bools() {
    assert_eq!(one("a = true"), json!(true));
    assert_eq!(one("a = false"), json!(false));
    for bad in ["a = True", "a = yes", "a = truex", "a = tru", "a = null"] {
        refused(bad);
    }
}

#[test]
fn toml_subset_array_trailing_comma() {
    assert_eq!(one(r#"t = ["a", "b",]"#), json!(["a", "b"]));
    assert_eq!(one(r#"t = [ "a" , "b" , ]"#), json!(["a", "b"]));
    assert_eq!(one(r#"t = ["a"]"#), json!(["a"]));
    assert_eq!(one("t = []"), json!([]));
    for bad in ["t = [,]", r#"t = ["a",,]"#, r#"t = ["a" "b"]"#, "t = [1, 2]", r#"t = ["a", true]"#, r#"t = [["a"]]"#, r#"t = ["a""#, r#"t = ["a"]]"#] {
        refused(bad);
    }
}

#[test]
fn toml_subset_comment_after_value() {
    let p = parse("# head\n\n  \t\nn = 5 # five\ns = \"x\"# \"quoted\"\nb = true#c\nl = [\"a\"] # list\ni = 7#c\n").unwrap();
    let want = [(4, "n", json!(5)), (5, "s", json!("x")), (6, "b", json!(true)), (7, "l", json!(["a"])), (8, "i", json!(7))];
    assert_eq!(p, want.map(|(n, k, v)| (n, k.to_string(), v)));
    refused("n = 5 five");
    refused("s = \"x\" y");
}

#[test]
fn toml_subset_bom_crlf() {
    let p = parse("\u{feff}a = 1\r\nb = \"x\"\r\n\r\n").unwrap();
    assert_eq!(p, [(1, "a".to_string(), json!(1)), (2, "b".to_string(), json!("x"))]);
    refused("a = 1\r\r\n");
    refused("a = \"x\ry\"\n");
    refused("b = 1\n\u{feff}a = 1\n");
    // A \r is stripped only before a \n.
    refused("a = 1\r");
}

#[test]
fn toml_subset_refuses() {
    let cases = [
        "[table]",
        "[t.u]\na = 1",
        "a = 'single'",
        "a = 1.5",
        "a = 1979-05-27",
        "a = 07:32:00",
        "a = \"\"\"multi\"\"\"",
        "a = [\n\"x\"]",
        "a = \"x\ty\"",
        "a = \"x\u{7f}\"",
        "a = \"\u{85}\"",
        "a = 1_000",
        "a = +1",
        "a = {b = 1}",
        "a.b = 1",
        "A = 1",
        "1a = 1",
        "= 1",
        "a =",
        "a",
    ];
    for c in cases {
        refused(c);
    }
    assert!(parse("a = 1\nb = 'x'\n").unwrap_err().starts_with("2: "));
}

#[test]
fn config_syntax() {
    let t = T::new();
    let e = t.err(&format!("{BASE}max_steps 5\n"));
    assert!(e.starts_with(&format!("{}:3: expected key = value; fix: ", t.cfg().display())), "{e}");
    has(&t.err(&format!("{BASE}[section]\n")), ":3: expected key = value");
    has(&t.err(&format!("{BASE}api_key = 'x'\n")), ":3: invalid value; fix: ");
}

#[test]
fn config_unknown_key() {
    let t = T::new();
    for k in ["foo = 1", "project_agents = true", "api_key_cmd = \"x\"", "shell = \"zsh\""] {
        let e = t.err(&format!("{BASE}{k}\n"));
        has(&e, &format!("config.toml:3: unknown key {}; fix: remove it", k.split(' ').next().unwrap()));
    }
}

#[test]
fn config_duplicate_key() {
    let t = T::new();
    has(&t.err(&format!("{BASE}max_steps = 3\nmodel = \"n\"\n")), "config.toml:4: duplicate key model (first on line 1)");
}

#[test]
fn config_wrong_type() {
    let t = T::new();
    let cases = [
        ("max_steps = \"5\"", "max_steps must be an integer"),
        ("max_tokens = true", "max_tokens must be an integer"),
        ("api_key = 5", "api_key must be a string"),
        ("approve = true", "approve must be a string"),
        ("tools = \"shell\"", "tools must be a list"),
        ("pass_env = \"LANG\"", "pass_env must be a list"),
        ("ext = \"/x\"", "ext must be a list"),
        ("dialect = [\"openai\"]", "dialect must be a string"),
    ];
    for (line, msg) in cases {
        has(&t.err(&format!("{BASE}{line}\n")), &format!(":3: {msg}"));
    }
    has(&t.err("model = 5\nbase_url = \"http://127.0.0.1/v1\"\n"), ":1: model must be a string");
    has(&t.err(&format!("{BASE}approve = \"always\"\n")), ":3: approve is \"ask\" or \"auto\"");
}

/// Each bad value of `key` fails at its line with the range, each good one loads.
fn ranges(key: &str, bad: &[u64], good: &[u64]) {
    let t = T::new();
    for n in bad {
        has(&t.err(&format!("{BASE}{key} = {n}\n")), &format!(":3: {key} = {n} is out of range; fix: use "));
    }
    for &n in good {
        let c = t.load(&format!("{BASE}{key} = {n}\n")).unwrap();
        let got = match key {
            "max_steps" => u64::from(c.max_steps),
            "max_seconds" => c.max_seconds,
            "tool_timeout_s" => c.tool_timeout_s,
            "output_cap" => c.output_cap as u64,
            "context_window" if n == 0 => return,
            "context_window" => c.window,
            _ => c.max_tokens,
        };
        assert_eq!(got, n, "{key}");
    }
}

#[test]
fn config_range_max_steps() {
    ranges("max_steps", &[0, 10_001], &[1, 12, 10_000]);
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let e = load(&Flags { max_steps: Some(0), ..flags() }, &t.env(&[]), &t.cwd).err().unwrap();
    assert_eq!(e, "--max-steps: max_steps = 0 is out of range; fix: use 1 to 10,000");
}

#[test]
fn config_range_max_seconds() {
    ranges("max_seconds", &[604_801], &[0, 1, 604_800]);
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let e = load(&Flags { max_seconds: Some(604_801), ..flags() }, &t.env(&[]), &t.cwd).err().unwrap();
    has(&e, "--max-seconds: max_seconds = 604801 is out of range; fix: use 0 to 604,800");
}

#[test]
fn config_range_tool_timeout_s() {
    ranges("tool_timeout_s", &[0, 601], &[1, 600]);
}

#[test]
fn config_range_output_cap() {
    ranges("output_cap", &[0, 1_023, 262_145], &[1_024, 262_144]);
}

#[test]
fn config_range_context_window() {
    ranges("context_window", &[1, 1_023], &[0, 1_024, 2_147_483_647]);
    let t = T::new();
    has(&t.err(&format!("{BASE}context_window = 5\n")), "use 1,024 to 2,147,483,647, or 0 when unknown");
    has(&t.err(&format!("{BASE}context_window = 2147483648\n")), ":3: invalid value");
}

#[test]
fn config_range_max_tokens() {
    ranges("max_tokens", &[0, 1_000_001], &[1, 1_000_000]);
}

#[test]
fn config_https_refused() {
    let t = T::new();
    let e = t.err("model = \"m\"\nbase_url = \"https://127.0.0.1:8080/v1\"\n");
    has(&e, "config.toml:2: https is not supported in henki v1; fix: ");
    assert_eq!(parse_url("https://localhost/v1").unwrap_err(), "https is not supported in henki v1");
}

#[test]
fn config_base_url_userinfo() {
    let t = T::new();
    has(&t.err("model = \"m\"\nbase_url = \"http://u:p@127.0.0.1:8080/v1\"\n"), ":2: http://u:p@127.0.0.1:8080/v1 holds a user name, password, query or fragment");
    assert!(parse_url("http://u@localhost/").is_err());
}

#[test]
fn config_base_url_query() {
    let t = T::new();
    has(&t.err("model = \"m\"\nbase_url = \"http://127.0.0.1/v1?key=1\"\n"), ":2: http://127.0.0.1/v1?key=1 holds a user name, password, query or fragment");
    assert!(parse_url("http://127.0.0.1/v1#x").is_err());
}

#[test]
fn config_base_url_slash() {
    let url = |host: &str, port, path: &str| Url { host: host.into(), port, path: path.into() };
    assert_eq!(parse_url("http://127.0.0.1:8080/v1/").unwrap(), url("127.0.0.1", 8080, "/v1"));
    assert_eq!(parse_url("http://127.0.0.1:8080/v1//").unwrap(), url("127.0.0.1", 8080, "/v1/"));
    assert_eq!(parse_url("http://localhost/v1").unwrap(), url("localhost", 80, "/v1"));
    assert_eq!(parse_url("http://[::1]:9931/").unwrap(), url("::1", 9931, ""));
    assert_eq!(parse_url("http://[::1]").unwrap(), url("::1", 80, ""));
    assert_eq!(parse_url("http://model.example:11434").unwrap(), url("model.example", 11434, ""));
    let bad = ["", "http://", "http:///v1", "http://:80/", "http://h:0/", "http://h:65536/", "http://h:/", "http://h:x/", "http://[::1/", "http://[zz]/"];
    for b in bad.iter().chain(&["http://::1/", "http://h/a b", "http://h/\u{e9}", "ftp://h/", "HTTP://h/", "http://h_1/", "http://[::1]x/"]) {
        assert!(parse_url(b).is_err(), "{b:?} parsed");
    }
    let t = T::new();
    let c = t.load("model = \"m\"\nbase_url = \"http://127.0.0.1:8080/v1/\"\n").unwrap();
    assert_eq!((c.url, c.url_from), (url("127.0.0.1", 8080, "/v1"), "config"));
}

#[test]
fn config_key_charset() {
    let t = T::new();
    for key in ["a b", "k\u{e9}y", " "] {
        has(&t.err(&format!("{BASE}api_key = \"{key}\"\n")), ":3: api_key holds a space or a byte outside printable ASCII");
    }
}

#[test]
fn config_key_non_loopback() {
    let t = T::new();
    for host in ["10.1.2.3", "model.example", "127.1", "localhost.", "0.0.0.0"] {
        let e = t.err(&format!("model = \"m\"\nbase_url = \"http://{host}:8080/v1\"\napi_key = \"k\"\n"));
        has(&e, &format!("config.toml:3: api_key is sent only to a loopback host, not to {host}"));
    }
    for host in ["localhost", "127.0.0.1", "127.9.8.7", "[::1]"] {
        let c = t.load(&format!("model = \"m\"\nbase_url = \"http://{host}:8080/v1\"\napi_key = \"sk-1\"\n")).unwrap();
        assert_eq!(c.api_key, "sk-1");
    }
    // Without a key a remote host loads: nothing secret is sent.
    assert_eq!(t.load("model = \"m\"\nbase_url = \"http://10.1.2.3/v1\"\napi_key = \"\"\n").unwrap().url.host, "10.1.2.3");
}

#[test]
fn config_henki_base_url_non_loopback() {
    let t = T::new();
    for u in ["http://192.168.1.5:8080/v1", "http://localhost.:8080/v1", "http://example.com/v1"] {
        let e = t.load_env(BASE, &[("HENKI_BASE_URL", u)]).err().unwrap();
        assert!(e.starts_with("HENKI_BASE_URL: ") && e.contains("is not a loopback host; fix: "), "{e}");
    }
    has(&t.load_env(BASE, &[("HENKI_BASE_URL", "https://127.0.0.1/v1")]).err().unwrap(), "HENKI_BASE_URL: https is not supported in henki v1");
    let c = t.load_env(BASE, &[("HENKI_BASE_URL", "http://[::1]:9931/v1/")]).unwrap();
    assert_eq!((c.url.host.as_str(), c.url.port, c.url.path.as_str(), c.url_from), ("::1", 9931, "/v1", "HENKI_BASE_URL"));
}

#[test]
fn config_tools_unknown() {
    has(&T::new().err(&format!("{BASE}tools = [\"shell\", \"bash\"]\n")), ":3: tool bash is unknown; fix: ");
}

#[test]
fn config_tools_repeat() {
    has(&T::new().err(&format!("{BASE}tools = [\"read\", \"edit\", \"read\"]\n")), ":3: tool read is listed twice");
}

#[test]
fn config_tools_empty() {
    has(&T::new().err(&format!("{BASE}tools = []\n")), ":3: tools is empty");
}

#[test]
fn config_tools_canonical_order() {
    let t = T::new();
    assert_eq!(t.load(BASE).unwrap().tools, ["shell", "read", "write", "edit"]);
    assert_eq!(t.load(&format!("{BASE}tools = [\"edit\", \"shell\", \"read\",]\n")).unwrap().tools, ["shell", "read", "edit"]);
}

#[test]
fn config_pass_env_name() {
    let t = T::new();
    for name in ["1A", "A-B", "", "A B"] {
        has(&t.err(&format!("{BASE}pass_env = [\"LANG_X\", \"{name}\"]\n")), &format!(":3: pass_env name {name:?} is not a variable name"));
    }
    for name in ["PATH", "HENKI_X"] {
        has(&t.err(&format!("{BASE}pass_env = [\"{name}\"]\n")), &format!(":3: pass_env cannot hold {name}; fix: "));
    }
    assert_eq!(t.load(&format!("{BASE}pass_env = [\"NODE_EXTRA_CA_CERTS\", \"_x1\"]\n")).unwrap().pass_env, ["NODE_EXTRA_CA_CERTS", "_x1"]);
}

#[test]
fn config_pass_env_credential() {
    let t = T::new();
    for name in ["GH_TOKEN", "api_key", "MY_SECRET", "PGPASSWORD", "AWS_CREDENTIALS", "OpenAI_Key"] {
        let text = format!("{BASE}pass_env = [\"{name}\"]\n");
        has(&t.err(&text), &format!(":3: pass_env {name} looks like a credential; fix: pass it only from a config given with --config"));
        assert_eq!(t.explicit(&text).unwrap().pass_env, [name]);
    }
}

#[test]
fn config_explicit_missing() {
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let missing = t.dir.join("missing.toml");
    let env = t.env(&[("HENKI_MODEL", "m"), ("HENKI_BASE_URL", "http://127.0.0.1:1/v1")]);
    let e = load(&Flags { config: Some(missing.clone()), ..flags() }, &env, &t.cwd).err().unwrap();
    assert!(e.starts_with(&format!("{}: ", missing.display())) && e.contains("; fix: "), "{e}");
    // A relative --config is taken from the cwd.
    let e = load(&Flags { config: Some("nope.toml".into()), ..flags() }, &env, &t.cwd).err().unwrap();
    has(&e, &t.cwd.join("nope.toml").display().to_string());
}

#[test]
fn config_missing_message() {
    let t = T::new();
    let want = "no config; copy one: mkdir -p ~/.config/henki && cp $(brew --prefix)/share/henki/config.mac.toml ~/.config/henki/config.toml";
    for extra in [&[][..], &[("HENKI_MODEL", "m")], &[("HENKI_BASE_URL", "http://127.0.0.1:1/v1")], &[("HENKI_MODEL", ""), ("HENKI_BASE_URL", "http://127.0.0.1:1/v1")]] {
        assert_eq!(load(&flags(), &t.env(extra), &t.cwd).err().unwrap(), want);
    }
}

#[test]
fn config_env_only_start() {
    let t = T::new();
    let c = load(&flags(), &t.env(&[("HENKI_MODEL", "qwen"), ("HENKI_BASE_URL", "http://127.0.0.1:8080/v1")]), &t.cwd).unwrap();
    assert_eq!((c.model.as_str(), c.url.port, c.url_from), ("qwen", 8080, "HENKI_BASE_URL"));
    assert_eq!((c.file, c.hash, c.explicit, c.auto), (None, None, false, false));
    assert_eq!(c.dir, t.cfg_dir());
    assert_eq!(c.protected, [t.cfg_dir(), t.home.join(".local/state/henki"), t.cfg_dir().join("ext")]);
}

#[test]
fn config_precedence() {
    let t = T::new();
    let text = format!("{BASE}max_steps = 7\nmax_seconds = 30\napprove = \"auto\"\n");
    let c = t.load(&text).unwrap();
    assert_eq!((c.model.as_str(), c.max_steps, c.max_seconds, c.auto), ("m", 7, 30, true));
    let env = t.env(&[("HENKI_MODEL", "from-env")]);
    let c = load(&flags(), &env, &t.cwd).unwrap();
    assert_eq!(c.model, "from-env");
    let f = Flags { model: Some("from-flag".into()), max_steps: Some(9), max_seconds: Some(0), approve: Some(false), config: None };
    let c = load(&f, &env, &t.cwd).unwrap();
    assert_eq!((c.model.as_str(), c.max_steps, c.max_seconds, c.auto, c.approve_from.as_str()), ("from-flag", 9, 0, false, "--approve"));
    assert_eq!((c.cwd, c.root, c.home), (t.cwd.clone(), t.cwd.clone(), t.home.clone()));
    assert_eq!(c.env.get("HENKI_MODEL"), Some(OsStr::new("from-env")));
    assert_eq!(c.hash, Some(sys::fnv(text.as_bytes())));
}

#[test]
fn config_ignores_xdg_config_home() {
    let t = T::new();
    let xdg = t.dir.join("xdg");
    fs::create_dir_all(xdg.join("henki")).unwrap();
    put(&xdg.join("henki/config.toml"), "model = \"xdg\"\nbase_url = \"http://127.0.0.1:1/v1\"\n", 0o600);
    let extra = [("XDG_CONFIG_HOME", xdg.to_str().unwrap())];
    assert_eq!(t.load_env(BASE, &extra).unwrap().model, "m");
    fs::remove_file(t.cfg()).unwrap();
    assert!(load(&flags(), &t.env(&extra), &t.cwd).err().unwrap().starts_with("no config; "));
}

#[test]
fn config_unknown_henki_env_notice() {
    let t = T::new();
    let silent = [("HENKI_DEPTH", "1"), ("HENKI_SESSION", "x"), ("HENKI_USER_PATH", "/bin"), ("HENKI_VERSION", "0.1.0")];
    let c = t.load_env(&format!("{BASE}context_window = 8192\n"), &[&silent[..], &[("HENKI_FOO", "1"), ("HENKI_CONFIG", "/x")]].concat()).unwrap();
    assert_eq!(c.notices, ["ignoring HENKI_FOO: henki does not read it", "ignoring HENKI_CONFIG: henki does not read it"]);
    assert!(t.load_env(&format!("{BASE}context_window = 8192\n"), &silent).unwrap().notices.is_empty());
}

#[test]
fn config_approve_not_from_env() {
    let t = T::new();
    let c = t.load_env(BASE, &[("HENKI_APPROVE", "auto"), ("HENKI_AUTO", "1")]).unwrap();
    assert_eq!((c.auto, c.approve_from.as_str()), (false, "default"));
    has(&c.notices.join("\n"), "ignoring HENKI_APPROVE");
    let c = t.load(&format!("{BASE}approve = \"auto\"\n")).unwrap();
    assert_eq!((c.auto, c.approve_from), (true, format!("{}:3", t.cfg().display())));
    let c = t.load(&format!("{BASE}approve = \"ask\"\n")).unwrap();
    assert!(!c.auto);
}

#[test]
fn config_file_mode() {
    let t = T::new();
    for mode in [0o620, 0o602, 0o666] {
        put(&t.cfg(), BASE, mode);
        let e = load(&flags(), &t.env(&[]), &t.cwd).err().unwrap();
        assert_eq!(e, format!("{0} is group- or world-writable; fix: chmod go-w {0}", t.cfg().display()));
    }
    put(&t.cfg(), BASE, 0o644);
    assert!(load(&flags(), &t.env(&[]), &t.cwd).is_ok());
}

#[test]
fn config_file_owner() {
    let p = Path::new("/c/config.toml");
    has(&owner_mode(p, 0o600, 4242, 1000, false).unwrap_err(), "/c/config.toml is owned by uid 4242, not by you (uid 1000); fix: use a file you own");
    assert!(owner_mode(p, 0o644, 0, 1000, false).is_ok(), "a root-owned file is fine");
    assert!(owner_mode(p, 0o600, 1000, 1000, false).is_ok());
    has(&owner_mode(p, 0o664, 1000, 1000, false).unwrap_err(), "group- or world-writable");
    let d = Path::new("/c");
    has(&owner_mode(d, 0o700, 0, 1000, true).unwrap_err(), "owned by uid 0, not by you (uid 1000); fix: use a directory you own");
    assert!(owner_mode(d, 0o755, 1000, 1000, true).is_ok());
    assert!(owner_mode(d, 0o755, 0, 0, true).is_ok());
}

#[test]
fn config_dir_mode() {
    let t = T::new();
    for mode in [0o770, 0o707, 0o777] {
        chmod(&t.cfg_dir(), mode);
        let e = t.err(BASE);
        assert_eq!(e, format!("{0} is group- or world-writable; fix: chmod go-w {0}", t.cfg_dir().display()));
    }
    chmod(&t.cfg_dir(), 0o755);
    assert!(t.load(BASE).is_ok());
}

#[test]
fn config_inside_project() {
    let t = T::new();
    put(&t.cwd.join("henki.toml"), BASE, 0o600);
    chmod(&t.cwd, 0o700);
    let e = load(&Flags { config: Some(t.cwd.join("henki.toml")), ..flags() }, &t.env(&[]), &t.cwd).err().unwrap();
    has(&e, &format!("henki.toml is inside the project {} or named through it; fix: ", t.cwd.display()));
    // The default file, linked from inside the project.
    symlink(t.cwd.join("henki.toml"), t.cfg()).unwrap();
    has(&load(&flags(), &t.env(&[]), &t.cwd).err().unwrap(), "is inside the project");
    // A project rooted at $HOME may hold the config.
    fs::remove_file(t.cfg()).unwrap();
    put(&t.cfg(), BASE, 0o600);
    fs::create_dir_all(t.home.join(".git")).unwrap();
    fs::create_dir_all(t.home.join("src")).unwrap();
    let c = load(&flags(), &t.env(&[]), &t.home.join("src")).unwrap();
    assert_eq!(c.root, t.home);
}

#[test]
fn config_no_bash() {
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let e = load(&flags(), &Env(vec![("HOME".into(), t.home.clone().into()), ("PATH".into(), "/nonexistent".into())]), &t.cwd).err().unwrap();
    assert_eq!(e, "bash is not on PATH; fix: install bash, or add its directory to PATH");
    let e = load(&flags(), &Env(vec![("HOME".into(), t.home.clone().into())]), &t.cwd).err().unwrap();
    has(&e, "bash is not on PATH");
    chmod(&t.bin.join("bash"), 0o644);
    has(&t.err(BASE), "bash is not on PATH");
}

#[test]
fn config_bash_absolute_only() {
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let rel = t.bin.strip_prefix("/").unwrap().to_str().unwrap().to_string();
    let env = |path: &str| Env(vec![("HOME".into(), t.home.clone().into()), ("PATH".into(), path.into())]);
    // The same directory spelled relative to `/` is never searched, whatever the process cwd is.
    has(&load(&flags(), &env(&format!("{rel}::.")), &t.cwd).err().unwrap(), "bash is not on PATH");
    let c = load(&flags(), &env(&format!("{rel}:/nonexistent::{}:.", t.bin.display())), &t.cwd).unwrap();
    assert_eq!(c.user_path, format!("/nonexistent:{}", t.bin.display()));
    assert_eq!(c.bash, t.bin.join("bash"));
}

#[test]
fn config_ext_missing() {
    let t = T::new();
    has(&t.err(&format!("{BASE}ext = [\"/nonexistent/ext\"]\n")), "config.toml:3: ext /nonexistent/ext: ");
    has(&t.err(&format!("{BASE}ext = [\"rel/ext\"]\n")), "config.toml:3: ext path rel/ext is relative; fix: ");
    fs::create_dir_all(t.home.join("exts")).unwrap();
    assert!(t.load(&format!("{BASE}ext = [\"~/exts\"]\n")).is_ok());
    put(&t.home.join("file"), "", 0o600);
    has(&t.err(&format!("{BASE}ext = [\"~/exts\", \"~/file\"]\n")), &format!("config.toml:3: ext {}: ", t.home.join("file").display()));
}

#[test]
fn config_ext_default_absent_ok() {
    let t = T::new();
    assert!(!t.cfg_dir().join("ext").exists());
    assert!(t.load(BASE).is_ok());
}

#[test]
fn config_ext_default_missing_explicit() {
    let t = T::new();
    assert!(t.explicit(BASE).is_ok());
    let ext = t.dir.join("cfg/ext");
    fs::remove_dir(&ext).unwrap();
    let file = t.dir.join("cfg/config.toml");
    let e = load(&Flags { config: Some(file.clone()), ..flags() }, &t.env(&[]), &t.cwd).err().unwrap();
    assert!(e.starts_with(&format!("{}: ext {}: ", file.display(), ext.display())) && e.contains("; fix: "), "{e}");
}

#[test]
fn config_ext_discovered() {
    let t = T::new();
    let ext = t.cfg_dir().join("ext");
    fs::create_dir_all(ext.join(".git")).unwrap();
    put(&ext.join(".keep"), "", 0o600);
    assert!(t.load(BASE).unwrap().exts.is_empty(), "dot entries are ignored");
    add_ext(&ext, "itse", &[]);
    add_ext(&ext, "halti", &["halti"]);
    for text in ["", "ext = [\"~/.config/henki/ext\"]\n"] {
        let c = t.load(&format!("{BASE}context_window = 8192\n{text}")).unwrap();
        let found: Vec<_> = c.exts.iter().map(|e| (e.name.as_str(), e.bin.clone())).collect();
        assert_eq!(found, [("halti", Some(ext.join("halti/bin"))), ("itse", None)]);
        assert!(c.notices.is_empty(), "{:?}", c.notices);
    }
    // What discovery refuses names the config line that chose the path, or the file for the default path.
    put(&ext.join("notes.txt"), "", 0o600);
    let e = t.err(BASE);
    assert!(e.starts_with(&format!("{}: ext ", t.cfg().display())) && e.contains("; fix: "), "{e}");
    let e = t.err(&format!("{BASE}context_window = 8192\next = [\"~/.config/henki/ext\"]\n"));
    assert!(e.starts_with(&format!("{}:4: ext ", t.cfg().display())), "{e}");
}

#[test]
fn config_ext_relative_refused() {
    let t = T::new();
    fs::create_dir_all(t.home.join("exts")).unwrap();
    // Before any path is read: the relative one is named even after a missing absolute one.
    for rel in ["rel/ext", "./ext", "~", "~user/ext", ""] {
        for list in [format!("\"~/exts\", \"{rel}\""), format!("\"/nonexistent\", \"{rel}\"")] {
            let e = t.err(&format!("{BASE}ext = [{list}]\n"));
            assert_eq!(e, format!("{}:3: ext path {rel} is relative; fix: use an absolute path or ~/", t.cfg().display()));
        }
    }
}

#[test]
fn config_user_path_drops_ext_bins() {
    let t = T::new();
    let ext = t.cfg_dir().join("ext");
    add_ext(&ext, "tool", &["tool-x"]);
    put(&t.cfg(), BASE, 0o600);
    let path = format!("{}:/nonexistent:rel:{}::.", ext.join("tool/bin").display(), t.bin.display());
    let c = load(&flags(), &t.env_path(&path), &t.cwd).unwrap();
    assert_eq!(c.user_path, format!("/nonexistent:{}", t.bin.display()));
    assert_eq!(c.exts[0].cmds, ["tool-x"]);
}

#[test]
fn config_bash_not_from_ext() {
    let t = T::new();
    let ext = t.cfg_dir().join("ext");
    add_ext(&ext, "sh", &["bash"]);
    put(&t.cfg(), &format!("{BASE}shadow = [\"sh/bash\"]\n"), 0o600);
    // The extension's bin/ comes first on the inherited PATH; henki's shell is still the user's.
    let env = t.env_path(&format!("{}:{}", ext.join("sh/bin").display(), t.bin.display()));
    let c = load(&flags(), &env, &t.cwd).unwrap();
    assert_eq!(c.bash, t.bin.join("bash"));
    assert_eq!(c.exts[0].shadows, [t.bin.join("bash")]);
    // With bash only in the extension there is none.
    fs::remove_file(t.bin.join("bash")).unwrap();
    assert_eq!(load(&flags(), &env, &t.cwd).err().unwrap(), "bash is not on PATH; fix: install bash, or add its directory to PATH");
}

#[test]
fn config_shadow_list() {
    let t = T::new();
    add_ext(&t.cfg_dir().join("ext"), "halti", &["tool"]);
    put(&t.bin.join("tool"), "#!/bin/sh\n", 0o755);
    let e = t.err(BASE);
    has(&e, "bin/tool shadows ");
    has(&e, "shadow = [\"halti/tool\"]");
    // The entries go to discovery as written; one naming no command of an extension is ignored.
    let c = t.load(&format!("{BASE}shadow = [\"halti/tool\", \"other/x\",]\n")).unwrap();
    assert_eq!(c.shadow, ["halti/tool", "other/x"]);
    assert_eq!(c.exts[0].shadows, [t.bin.join("tool")]);
    assert!(t.load(&format!("{BASE}shadow = [\"other/x\"]\n")).is_err());
}

#[test]
fn config_shadow_not_list() {
    let t = T::new();
    has(&t.err(&format!("{BASE}shadow = \"halti/halti\"\n")), ":3: shadow must be a list; fix: write shadow = [\"...\"]");
    has(&t.err(&format!("{BASE}shadow = true\n")), ":3: shadow must be a list");
    assert!(t.load(&format!("{BASE}shadow = []\n")).unwrap().shadow.is_empty());
}

#[test]
fn config_dialect_anthropic_accepted() {
    let t = T::new();
    assert_eq!(t.load(BASE).unwrap().dialect, "openai");
    assert_eq!(t.load(&format!("{BASE}dialect = \"openai\"\n")).unwrap().dialect, "openai");
    assert_eq!(t.load(&format!("{BASE}dialect = \"anthropic\"\n")).unwrap().dialect, "anthropic");
}

#[test]
fn config_dialect_unknown() {
    let t = T::new();
    for d in ["gemini", "", "Anthropic", "openai "] {
        let e = t.err(&format!("{BASE}dialect = \"{d}\"\n"));
        assert_eq!(e, format!("{}:3: unknown dialect {d}; fix: use dialect = \"openai\" or \"anthropic\"", t.cfg().display()));
    }
}

#[test]
fn config_project_agents_still_unknown() {
    let t = T::new();
    let e = t.err(&format!("{BASE}project_agents = true\n"));
    assert_eq!(e, format!("{}:3: unknown key project_agents; fix: remove it (this build knows {KEYS})", t.cfg().display()));
    assert!(KEYS.contains("ext, shadow, pass_env") && !KEYS.contains("project_agents"));
}

#[test]
fn config_state_path_xdg() {
    let t = T::new();
    assert_eq!(t.load(BASE).unwrap().state, t.home.join(".local/state/henki"));
    let xdg = t.dir.join("xdg-state");
    assert_eq!(t.load_env(BASE, &[("XDG_STATE_HOME", xdg.to_str().unwrap())]).unwrap().state, xdg.join("henki"));
    assert!(!xdg.exists() && !t.home.join(".local").exists(), "load creates nothing");
}

#[test]
fn config_state_path_relative_xdg_ignored() {
    let t = T::new();
    for rel in ["state", "./state", "~/state", ""] {
        assert_eq!(t.load_env(BASE, &[("XDG_STATE_HOME", rel)]).unwrap().state, t.home.join(".local/state/henki"), "{rel:?}");
    }
}

#[test]
fn config_state_protected() {
    let t = T::new();
    let xdg = t.dir.join("xdg");
    assert!(t.load_env(BASE, &[("XDG_STATE_HOME", xdg.to_str().unwrap())]).unwrap().protected.contains(&xdg.join("henki")), "absent, by its path");
    // Behind a link, by both names.
    let real = t.dir.join("real-state");
    fs::create_dir_all(real.join("henki")).unwrap();
    fs::create_dir_all(t.home.join(".local")).unwrap();
    symlink(&real, t.home.join(".local/state")).unwrap();
    let c = t.load(BASE).unwrap();
    for p in [t.home.join(".local/state/henki"), real.join("henki")] {
        assert!(c.protected.contains(&p), "{p:?} not in {:?}", c.protected);
    }
}

#[test]
fn config_ext_paths_protected() {
    let t = T::new();
    assert!(t.load(BASE).unwrap().protected.contains(&t.cfg_dir().join("ext")), "the default path, absent");
    let (plain, real, link) = (t.home.join("exts"), t.dir.join("real-exts"), t.home.join("linked-exts"));
    fs::create_dir_all(&plain).unwrap();
    fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();
    let c = t.load(&format!("{BASE}ext = [\"~/exts\", \"~/linked-exts\"]\n")).unwrap();
    assert_eq!(c.protected[c.protected.len() - 3..], [plain, link, real]);
    assert!(!c.protected.contains(&t.cfg_dir().join("ext")), "only the paths in use");
}

#[test]
fn config_ext_symlink_target_protected() {
    let t = T::new();
    let ext = t.cfg_dir().join("ext");
    fs::create_dir_all(&ext).unwrap();
    // The extension dir is a link to a checkout, and one of its commands a link to a file elsewhere.
    let src = t.dir.join("src");
    add_ext(&src, "halti-checkout", &["halti"]);
    put(&t.dir.join("tool-real"), "#!/bin/sh\n", 0o755);
    symlink(t.dir.join("tool-real"), src.join("halti-checkout/bin/tool")).unwrap();
    symlink(src.join("halti-checkout"), ext.join("halti")).unwrap();
    let c = t.load(BASE).unwrap();
    assert_eq!(c.exts[0].name, "halti");
    for p in [src.join("halti-checkout"), t.dir.join("tool-real")] {
        assert!(c.exts[0].targets.contains(&p) && c.protected.contains(&p), "{p:?} not in {:?}", c.protected);
    }
}

#[test]
fn config_allow_loaded_in_ask() {
    let t = T::new();
    let allow = t.cfg_dir().join("allow");
    let c = t.load(BASE).unwrap();
    assert_eq!((c.allow.file, c.allow.rules.len(), c.allow.hash), (allow.clone(), 0, None), "absent: no rules");
    put(&allow, &format!("# mine\n{}\tgit status\n*\tls -la\n", t.cwd.display()), 0o600);
    let c = t.load(BASE).unwrap();
    let rules: Vec<_> = c.allow.rules.iter().map(|r| (r.root.as_str(), r.cmd.as_str())).collect();
    assert_eq!(rules, [(t.cwd.to_str().unwrap(), "git status"), ("*", "ls -la")]);
    let direct = allow::load(&allow, true).unwrap();
    assert_eq!((c.allow.file, c.allow.rules, c.allow.hash), (direct.file, direct.rules, direct.hash));
}

#[test]
fn config_allow_not_read_in_auto() {
    let t = T::new();
    let allow = t.cfg_dir().join("allow");
    put(&allow, "not a rule\n", 0o666);
    let text = format!("{BASE}context_window = 8192\n");
    let auto = [t.load(&format!("{text}approve = \"auto\"\n")).unwrap(), load(&Flags { approve: Some(true), ..flags() }, &t.env(&[]), &t.cwd).unwrap()];
    for c in auto {
        assert_eq!((c.allow.file, c.allow.rules.len(), c.allow.hash), (allow.clone(), 0, None));
        assert!(c.notices.is_empty(), "{:?}", c.notices);
    }
    assert!(t.load(&text).is_err(), "ask mode reads it");
}

#[test]
fn config_allow_fail_closed() {
    let t = T::new();
    let allow = t.cfg_dir().join("allow");
    put(&t.dir.join("elsewhere"), "*\tls\n", 0o600);
    for what in ["a line without a tab", "world-writable", "a link"] {
        let _ = fs::remove_file(&allow);
        match what {
            "a line without a tab" => put(&allow, "git status\n", 0o600),
            "world-writable" => put(&allow, "*\tls\n", 0o666),
            _ => symlink(t.dir.join("elsewhere"), &allow).unwrap(),
        }
        let want = allow::load(&allow, true).err().unwrap_or_else(|| panic!("allow::load takes {what}"));
        assert_eq!(t.err(BASE), want, "{what}");
    }
}

#[test]
fn config_allow_inside_project() {
    let t = T::new();
    let allow = t.cfg_dir().join("allow");
    // A link from the config dir into the project.
    put(&t.cwd.join("rules"), "*\tls\n", 0o600);
    symlink(t.cwd.join("rules"), &allow).unwrap();
    assert_eq!(t.err(BASE), format!("{} is inside the project {} or named through it; fix: use a config dir outside the project", allow.display(), t.cwd.display()));
    // No config file, and the default config dir in a repository at ~/.config.
    fs::remove_file(&allow).unwrap();
    fs::remove_file(t.cfg()).unwrap();
    let root = t.home.join(".config");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    let env = t.env(&[("HENKI_MODEL", "m"), ("HENKI_BASE_URL", "http://127.0.0.1:1/v1")]);
    assert!(load(&flags(), &env, &root.join("sub")).is_ok(), "absent, it holds no rules");
    put(&allow, "*\tls\n", 0o600);
    let e = load(&flags(), &env, &root.join("sub")).err().unwrap();
    assert_eq!(e, format!("{} is inside the project {} or named through it; fix: use a config dir outside the project", allow.display(), root.display()));
    assert!(load(&Flags { approve: Some(true), ..flags() }, &env, &root.join("sub")).is_ok(), "auto never reads it");
}

#[test]
fn config_allow_notice() {
    let t = T::new();
    let allow = t.cfg_dir().join("allow");
    let (here, other) = (t.cwd.display(), t.dir.join("other"));
    let text = format!("{BASE}context_window = 8192\n");
    put(&allow, &format!("{here}\tgit status\n{}\tmake\n*\tls\n# {here}\tx\n{here}\tcargo test\n", other.display()), 0o600);
    assert_eq!(t.load(&text).unwrap().notices, ["allow: 3 rules apply to this root (~/.config/henki/allow; /allow lists them)"]);
    // Rules for other roots only, or auto mode: no notice.
    put(&allow, &format!("{}\tmake\n", other.display()), 0o600);
    assert!(t.load(&text).unwrap().notices.is_empty());
    put(&allow, "*\tls\n", 0o600);
    assert_eq!(t.load(&text).unwrap().notices, ["allow: 1 rules apply to this root (~/.config/henki/allow; /allow lists them)"]);
    assert!(t.load(&format!("{text}approve = \"auto\"\n")).unwrap().notices.is_empty());
}

#[test]
fn config_trust_paths() {
    let t = T::new();
    let (d, ext) = (t.cfg_dir(), t.cfg_dir().join("ext"));
    add_ext(&ext, "halti", &["halti"]);
    let c = t.load(BASE).unwrap();
    assert!(c.exts[0].paths.contains(&ext.join("halti/doc.md")) && c.exts[0].paths.contains(&ext.join("halti/bin/halti")));
    let want: Vec<_> = [t.cfg(), d.join("allow"), d.join("AGENTS.md"), t.bin.join("bash"), ext.clone()].into_iter().chain(c.exts[0].paths.clone()).collect();
    assert_eq!(c.trust, want);
    assert!(!d.join("allow").exists() && !d.join("AGENTS.md").exists(), "absent files are watched too");
    // Without a config file its default path is watched; with --config, the file named and the ext paths listed.
    fs::remove_dir_all(&ext).unwrap();
    fs::remove_file(t.cfg()).unwrap();
    let c = load(&flags(), &t.env(&[("HENKI_MODEL", "m"), ("HENKI_BASE_URL", "http://127.0.0.1:1/v1")]), &t.cwd).unwrap();
    assert_eq!(c.trust, [t.cfg(), d.join("allow"), d.join("AGENTS.md"), t.bin.join("bash"), ext.clone()]);
    fs::create_dir_all(t.home.join("exts")).unwrap();
    let c = t.explicit(&format!("{BASE}ext = [\"~/exts\"]\n")).unwrap();
    let cfg = t.dir.join("cfg");
    assert_eq!(c.trust, [cfg.join("config.toml"), cfg.join("allow"), cfg.join("AGENTS.md"), t.bin.join("bash"), t.home.join("exts")]);
}

#[test]
fn profiles_parse() {
    let mac = include_str!("../../profiles/config.mac.toml");
    let boxed = include_str!("../../profiles/config.box.toml");
    assert!(!parse(mac).unwrap().is_empty());
    assert!(!parse(boxed).unwrap().is_empty());
    let t = T::new();
    let c = t.load(mac).unwrap();
    assert_eq!((c.dialect, c.max_steps, c.auto, c.window, c.url.port), ("openai", 12, false, 32_768, 8080));
    assert!(c.notices.is_empty() && c.exts.is_empty());
    // With the halti extension installed, its commands shadow the real ones that line names.
    for cmd in ["halti", "pbpaste", "security"] {
        put(&t.bin.join(cmd), "#!/bin/sh\n", 0o755);
    }
    add_ext(&t.cfg_dir().join("ext"), "halti", &["halti", "pbpaste", "security"]);
    assert!(t.load(&mac.replace("shadow", "# shadow")).is_err());
    assert_eq!(t.load(mac).unwrap().exts[0].shadows.len(), 3);
    // The box file, with its mount moved under the test dir and a doc-only extension in it.
    let mnt = t.dir.join("cfg");
    add_ext(&mnt.join("ext"), "itse", &[]);
    let c = t.explicit(&boxed.replace("/mnt/henki", mnt.to_str().unwrap())).unwrap();
    assert_eq!((c.dialect, c.max_steps, c.auto, c.url.host.as_str(), c.pass_env.len(), c.output_cap), ("anthropic", 100, true, "127.0.0.2", 8, 32_768));
    assert_eq!((c.exts.len(), c.exts[0].name.as_str(), c.exts[0].bin.is_none()), (1, "itse", true));
}

#[test]
fn config_agents_loaded_trimmed() {
    let t = T::new();
    assert_eq!((t.load(BASE).unwrap().agents, t.load(BASE).unwrap().agents_hash), (None, None));
    let text = "Use the doc.\n\n- one\n\n\n";
    put(&t.cfg_dir().join("AGENTS.md"), text, 0o600);
    let c = t.load(BASE).unwrap();
    assert_eq!(c.agents.as_deref(), Some("Use the doc.\n\n- one"));
    assert_eq!(c.agents_hash, Some(sys::fnv(text.as_bytes())));
    put(&t.cfg_dir().join("AGENTS.md"), "\n\n", 0o600);
    let c = t.load(BASE).unwrap();
    assert_eq!((c.agents, c.agents_hash.is_some()), (None, true));
}

#[test]
fn config_agents_too_big() {
    let t = T::new();
    let p = t.cfg_dir().join("AGENTS.md");
    put(&p, &"a".repeat(4096), 0o600);
    assert_eq!(t.load(BASE).unwrap().agents.unwrap().len(), 4096);
    put(&p, &"a".repeat(4097), 0o600);
    assert_eq!(t.err(BASE), format!("{}: over 4,096 bytes; fix: shorten it", p.display()));
}

#[test]
fn config_agents_escaped_refused() {
    let t = T::new();
    let p = t.cfg_dir().join("AGENTS.md");
    let tag = format!(":1: holds U+{:X}", 0xe0001);
    for (text, at) in [("ok\nbad \u{202e} here\n", ":2: holds U+202E"), ("a\r\n", ":1: holds U+000D"), ("\u{feff}x", ":1: holds U+FEFF"), ("\u{e0001}", &tag)] {
        put(&p, text, 0o600);
        has(&t.err(BASE), &format!("AGENTS.md{at}, which a terminal does not show; fix: remove it"));
    }
}

#[test]
fn config_window_default_notice() {
    let t = T::new();
    let c = t.load(BASE).unwrap();
    assert_eq!((c.window, c.notices), (32_768, vec!["context_window is unknown; using 32,768 tokens".to_string()]));
    let c = t.load(&format!("{BASE}context_window = 8192\n")).unwrap();
    assert_eq!((c.window, c.notices.len()), (8_192, 0));
}

#[test]
fn config_fifo_refused() {
    let t = T::new();
    let fifo = t.dir.join("fifo.toml");
    mkfifo(&fifo);
    let (tx, rx) = std::sync::mpsc::channel();
    let (env, cwd, f) = (t.env(&[]), t.cwd.clone(), fifo.clone());
    std::thread::spawn(move || tx.send(load(&Flags { config: Some(f), ..flags() }, &env, &cwd).err()));
    let e = rx.recv_timeout(Duration::from_secs(5)).expect("load blocked on a FIFO").unwrap();
    has(&e, &format!("not a regular file: {} (fifo)", fifo.display()));
    mkfifo(&t.cfg_dir().join("AGENTS.md"));
    has(&t.err(BASE), "AGENTS.md (fifo)");
}

#[test]
fn config_home_unset() {
    let t = T::new();
    let path = ("PATH".into(), t.bin.clone().into());
    for home in [None, Some(""), Some("relative/home")] {
        let env = Env(home.map(|h| ("HOME".into(), h.into())).into_iter().chain([path.clone()]).collect());
        assert_eq!(load(&flags(), &env, &t.cwd).err().unwrap(), "HOME is not set; fix: set HOME to your home directory");
    }
}

#[test]
fn config_protected_realpaths() {
    let t = T::new();
    let real = t.dir.join("dotfiles");
    fs::create_dir_all(&real).unwrap();
    chmod(&real, 0o700);
    fs::remove_dir(t.cfg_dir()).unwrap();
    symlink(&real, t.cfg_dir()).unwrap();
    let c = t.load(BASE).unwrap();
    let real = fs::canonicalize(&real).unwrap();
    assert_eq!(c.protected, [t.cfg(), real.join("config.toml"), t.cfg_dir(), real, t.home.join(".local/state/henki"), t.cfg_dir().join("ext")]);
    assert_eq!((c.file, c.dir), (Some(t.cfg()), t.cfg_dir()));
}

#[test]
fn project_root_cases() {
    let t = T::new();
    // A .git directory, from deep below it.
    let deep = t.cwd.join("a/b");
    fs::create_dir_all(&deep).unwrap();
    assert_eq!(project_root(&deep, &t.home), t.cwd);
    // A .git file (a worktree or submodule).
    let wt = t.dir.join("wt");
    fs::create_dir_all(wt.join("src")).unwrap();
    put(&wt.join(".git"), "gitdir: elsewhere\n", 0o600);
    assert_eq!(project_root(&wt.join("src"), &t.home), wt);
    // $HOME itself is checked, and the walk stops after it.
    let sub = t.home.join("p/q");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(project_root(&sub, &t.home), sub);
    fs::create_dir_all(t.dir.join(".git")).unwrap();
    assert_eq!(project_root(&sub, &t.home), sub, "a repository above $HOME is not the project");
    fs::create_dir_all(t.home.join(".git")).unwrap();
    assert_eq!(project_root(&sub, &t.home), t.home);
    // Without one up to `/`, the cwd (unless the temp dir itself sits in a repository).
    let none = std::env::temp_dir().join(format!("henki-config-none-{}", std::process::id()));
    fs::create_dir_all(&none).unwrap();
    let above = none.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf);
    assert_eq!(project_root(&none, Path::new("/nonexistent")), above.unwrap_or_else(|| none.clone()));
    fs::remove_dir(&none).unwrap();
}

#[test]
fn expand_tilde() {
    let home = Path::new("/home/u");
    assert_eq!(expand("~/x/y", home), Path::new("/home/u/x/y"));
    assert_eq!(expand("~/", home), Path::new("/home/u/"));
    for same in ["~", "~user/x", "/abs/~/x", "rel/~/x", "rel"] {
        assert_eq!(expand(same, home), Path::new(same));
    }
}

#[test]
fn config_never_panics() {
    const PIECES: [&str; 24] = ["a", "key", " = ", "=", "\"", "\\", "[", "]", ",", "#", " ", "\t", "\n", "\r", "0", "9", "true", "-", "_", "'", "\u{feff}", "é", "\\u", "{"];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for i in 0..10_000 {
        let len = (next() % 24) as usize;
        let s: String = if i % 2 == 0 {
            (0..len).map(|_| PIECES[(next() % PIECES.len() as u64) as usize]).collect()
        } else {
            let bytes: Vec<u8> = (0..len * 3).map(|_| next() as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let _ = parse(&s);
        let _ = parse_url(&s);
        let _ = parse_url(&format!("http://{s}"));
    }
}

#[test]
fn config_empty_model_base_url() {
    let t = T::new();
    has(&t.err("model = \"\"\nbase_url = \"http://127.0.0.1/v1\"\n"), "config.toml:1: model is empty; fix: ");
    has(&t.err("model = \"m\"\nbase_url = \"\"\n"), "config.toml:2: base_url is empty; fix: ");
    assert_eq!(t.err("base_url = \"http://127.0.0.1/v1\"\n"), format!("no model; fix: set model in {}", t.cfg().display()));
    assert_eq!(t.err("model = \"m\"\n"), format!("no base_url; fix: set base_url in {}", t.cfg().display()));
    put(&t.cfg(), BASE, 0o600);
    let e = load(&Flags { model: Some(String::new()), ..flags() }, &t.env(&[]), &t.cwd).err().unwrap();
    assert_eq!(e, "--model: model is empty; fix: set model");
}

#[test]
fn config_too_big() {
    let t = T::new();
    let pad = format!("{BASE}{}", "# padding\n".repeat(6_600));
    assert!(pad.len() > 64 << 10);
    assert_eq!(t.err(&pad), format!("{}: over 65,536 bytes; fix: shorten it", t.cfg().display()));
    assert!(t.load(&pad[..64 << 10]).is_ok());
}

#[test]
fn config_agents_mode() {
    let t = T::new();
    let p = t.cfg_dir().join("AGENTS.md");
    for mode in [0o620, 0o602, 0o666] {
        put(&p, "be brief\n", mode);
        assert_eq!(t.err(BASE), format!("{0} is group- or world-writable; fix: chmod go-w {0}", p.display()));
    }
    // A link is judged by the file it reaches.
    let shared = t.dir.join("shared.md");
    put(&shared, "be brief\n", 0o666);
    fs::remove_file(&p).unwrap();
    symlink(&shared, &p).unwrap();
    has(&t.err(BASE), "AGENTS.md is group- or world-writable");
    put(&p, "be brief\n", 0o644);
    assert_eq!(t.load(BASE).unwrap().agents.as_deref(), Some("be brief"));
}

#[test]
fn config_home_symlinked() {
    // HOME names the home directory through a link; the cwd is its real path, as getcwd reports it.
    let t = T::new();
    put(&t.cfg(), BASE, 0o600);
    let link = t.dir.join("homelink");
    symlink(&t.home, &link).unwrap();
    let env = Env(vec![("HOME".into(), link.clone().into()), ("PATH".into(), t.bin.clone().into())]);
    let c = load(&flags(), &env, &t.home).unwrap();
    assert_eq!((c.root, c.home, c.file), (t.home.clone(), link.clone(), Some(link.join(".config/henki/config.toml"))));
    // A repository at home is the project of a directory below it, and may hold the config.
    let notes = t.home.join("notes");
    fs::create_dir_all(t.home.join(".git")).unwrap();
    fs::create_dir_all(&notes).unwrap();
    assert_eq!(load(&flags(), &env, &notes).unwrap().root, t.home);
    // The walk stops at home: a repository above it is not the project.
    fs::remove_dir(t.home.join(".git")).unwrap();
    fs::create_dir_all(t.dir.join(".git")).unwrap();
    assert_eq!(load(&flags(), &env, &notes).unwrap().root, notes);
}

#[test]
fn config_explicit_relative_dotdot() {
    let t = T::new();
    t.explicit(BASE).unwrap();
    let file = t.dir.join("cfg/config.toml");
    let sub = t.cwd.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let named = |p: &Path, cwd: &Path| load(&Flags { config: Some(p.into()), ..flags() }, &t.env(&[]), cwd);
    // Leading `..` climb the cwd itself, so the file named is the one outside the project.
    assert_eq!(named(Path::new("../../cfg/config.toml"), &sub).unwrap().file, Some(file.clone()));
    assert_eq!(named(Path::new("../cfg/./config.toml"), &t.bin).unwrap().file, Some(file.clone()));
    // A `..` after a name in the project stays refused: the project may turn that name into a link to anywhere.
    has(&named(Path::new("sub/../../cfg/config.toml"), &t.cwd).err().unwrap(), "or named through it; fix: ");
    // So does a link in the project to the config dir outside it.
    symlink(t.dir.join("cfg"), t.cwd.join("cfg")).unwrap();
    let e = named(Path::new("cfg/config.toml"), &t.cwd).err().unwrap();
    has(&e, &format!("work/cfg/config.toml is inside the project {} or named through it", t.cwd.display()));
    assert!(named(&file, &sub).is_ok());
}

#[test]
fn config_henki_base_url_overrides_file() {
    let t = T::new();
    let text = "model = \"m\"\nbase_url = \"http://10.1.2.3:8080/v1\"\napi_key = \"sk-1\"\n";
    has(&t.err(text), "config.toml:3: api_key is sent only to a loopback host, not to 10.1.2.3");
    // The key goes where the requests go: HENKI_BASE_URL replaces the remote base_url with a loopback one.
    let c = t.load_env(text, &[("HENKI_BASE_URL", "http://127.0.0.1:9931/v1")]).unwrap();
    assert_eq!((c.url.host.as_str(), c.url.port, c.url_from, c.api_key.as_str()), ("127.0.0.1", 9931, "HENKI_BASE_URL", "sk-1"));
    // An empty HENKI_BASE_URL is unset.
    assert_eq!(t.load_env(BASE, &[("HENKI_BASE_URL", "")]).unwrap().url_from, "config");
}

#[test]
fn config_ext_unreadable() {
    if sys::euid() == 0 {
        eprintln!("config_ext_unreadable skipped: root reads any directory");
        return;
    }
    let t = T::new();
    let exts = t.home.join("exts");
    fs::create_dir_all(&exts).unwrap();
    chmod(&exts, 0o000);
    let listed = t.err(&format!("{BASE}ext = [\"~/exts\"]\n"));
    chmod(&exts, 0o700);
    assert!(listed.starts_with(&format!("{}:3: ext {}: ", t.cfg().display(), exts.display())), "{listed}");
    has(&listed, "; fix: ");
    // The default ext path may be absent, but an unreadable one fails closed too.
    let ext = t.cfg_dir().join("ext");
    fs::create_dir_all(&ext).unwrap();
    chmod(&ext, 0o000);
    let default = t.err(BASE);
    chmod(&ext, 0o700);
    assert!(default.starts_with(&format!("{}: ext {}: ", t.cfg().display(), ext.display())), "{default}");
}

#[test]
fn parse_url_port_grammar() {
    for bad in ["http://127.0.0.1:+80/v1", "http://[::1]:+9931/", "http://localhost:+1"] {
        assert!(parse_url(bad).is_err(), "{bad:?} parsed");
    }
    // A port is digits; leading zeros are digits too.
    assert_eq!(parse_url("http://127.0.0.1:0080/v1").unwrap().port, 80);
}
