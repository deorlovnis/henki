#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The command line: `--version`, `--help`, refused arguments, the argument errors of `run`, the REPL's refusal
//! without a terminal, and `henki mods` (`henki allow`), which becomes the module manager without reading a config.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: henki [--config FILE] [--approve ask|auto] [--model NAME] [--resume[=ID]] [--nested]
       henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki mods [ARGS] | henki allow [rm N]
       henki --version | --help
";

/// henki with an empty environment and a HOME that does not exist, so nothing outside the test is read.
fn henki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_henki")).args(args).env_clear().env("HOME", "/nonexistent/henki-cli-home").env("PATH", "/usr/bin:/bin").output().expect("run henki")
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("utf-8 output")
}

/// A throwaway HOME (removed on drop) with a 0700 `~/.config/henki/`.
struct Home(PathBuf);

impl Home {
    fn new() -> Home {
        static N: AtomicUsize = AtomicUsize::new(0);
        let h = PathBuf::from(format!("/tmp/henki-cli-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(h.join(".config/henki")).unwrap();
        fs::set_permissions(h.join(".config/henki"), fs::Permissions::from_mode(0o700)).unwrap();
        Home(h)
    }

    fn allow(&self) -> PathBuf {
        self.0.join(".config/henki/allow")
    }

    /// henki in this HOME (also its cwd), stdin at /dev/null, with `env` added.
    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_henki"));
        c.args(args).current_dir(&self.0).env_clear().env("HOME", &self.0).env("PATH", "/usr/bin:/bin").env("XDG_STATE_HOME", self.0.join("state"));
        c.envs(env.iter().copied()).stdin(Stdio::null()).output().expect("run henki")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str, mode: u32) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn version() {
    let out = henki(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    let commit = option_env!("HENKI_COMMIT").unwrap_or("unknown");
    assert_eq!(text(&out.stdout), format!("henki {} ({commit}, {}-{})\n", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS));
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn help() {
    let out = henki(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).ends_with(USAGE), "{}", text(&out.stdout));
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn unknown_argument() {
    let cases = [(&["--bogus"][..], "--bogus"), (&["--project-agents"][..], "--project-agents"), (&["--version", "extra"][..], "extra"), (&["repl"][..], "repl")];
    for (args, says) in cases {
        let out = henki(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&out.stdout), "", "{args:?}");
        let err = text(&out.stderr);
        assert!(err.starts_with("henki: ") && err.contains(says) && err.ends_with(USAGE), "{args:?}: {err}");
    }
}

/// `run` argument errors: exit 2 `config`; a `--json` result with `session: null` only when `--json` came first.
#[test]
fn run_flags() {
    let json_first: [(&[&str], &str); 7] = [
        (&["run", "--json", "--max-steps", "abc", "task"], "abc"),
        (&["run", "--json", "--max-seconds"], "--max-seconds"),
        (&["run", "--json", "--approve", "maybe", "task"], "maybe"),
        (&["run", "--json", "--bogus", "task"], "--bogus"),
        (&["run", "--json", "--project-agents", "task"], "--project-agents"),
        (&["run", "--json", "one", "two"], "two"),
        (&["run", "--json"], ""),
    ];
    for (args, says) in json_first {
        let out = henki(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out.stderr));
        let lines: Vec<serde_json::Value> = text(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 1, "{args:?}: one result line");
        let r = &lines[0];
        assert_eq!(
            (r["t"].as_str(), r["status"].as_str(), r["exit"].as_i64(), r["error"]["kind"].as_str()),
            (Some("result"), Some("error"), Some(2), Some("config")),
            "{args:?}: {r}"
        );
        assert!(r["session"].is_null() && r["error"]["message"].as_str().unwrap().contains(says), "{args:?}: {r}");
        assert_eq!(text(&out.stderr), "", "{args:?}: --json writes no stderr");
    }
    for args in [&["run", "--max-steps", "abc", "--json", "task"][..], &["run", "--bogus", "--json", "task"][..]] {
        let out = henki(args);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "{args:?}: no result before --json is known");
        assert!(text(&out.stderr).starts_with("henki: "), "{args:?}");
    }
}

#[test]
fn repl_needs_terminal() {
    let h = Home::new();
    let out = h.run(&[], &[("HENKI_BASE_URL", "http://127.0.0.1:9/v1"), ("HENKI_MODEL", "m")]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "henki: the REPL needs a terminal; use henki run\n");
    let sessions = h.0.join("state/henki/sessions");
    let files: Vec<_> = fs::read_dir(&sessions).into_iter().flatten().flatten().flat_map(|d| fs::read_dir(d.path()).into_iter().flatten().flatten()).collect();
    assert!(files.is_empty(), "a session file: {files:?}");
}

/// A stand-in manager in DIR: it prints its arguments, HENKI_BIN and its signal mask and ignored set (Linux), then
/// exits 7.
fn fake_manager(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let script = "#!/bin/sh\nfor a in \"$@\"; do echo \"arg $a\"; done\necho \"bin $HENKI_BIN\"\n\
        [ -r /proc/self/status ] && grep -E '^Sig(Blk|Ign)' /proc/self/status\nexit 7\n";
    write(&dir.join("henki-mods"), script, 0o755);
}

/// `henki mods ARGS` and `henki allow ARGS` become the manager with ARGS untouched and HENKI_BIN set, without reading
/// a config or checking HENKI_DEPTH, its exit code henki's; the signals henki blocks and ignores are back to their
/// defaults.
#[test]
fn mods_execs_manager() {
    let h = Home::new();
    fake_manager(&h.0.join("bin"));
    write(&h.0.join(".config/henki/config.toml"), "not toml\n", 0o600);
    let path = format!("{}:/usr/bin:/bin", h.0.join("bin").display());
    let env = [("PATH", path.as_str()), ("HENKI_DEPTH", "3")];
    let bin = env!("CARGO_BIN_EXE_henki");
    let cases: [(&[&str], &str); 3] = [
        (&["mods", "add", "./x", "--yes", "--config", "a b"], "arg add\narg ./x\narg --yes\narg --config\narg a b\n"),
        (&["allow", "rm", "1"], "arg allow\narg rm\narg 1\n"),
        (&["mods"], ""),
    ];
    for (args, want) in cases {
        let out = h.run(args, &env);
        assert_eq!((out.status.code(), text(&out.stderr)), (Some(7), ""), "{args:?}");
        assert!(text(&out.stdout).starts_with(&format!("{want}bin {bin}\n")), "{args:?}: {}", text(&out.stdout));
    }
    if cfg!(target_os = "linux") {
        // Started with INT and QUIT ignored, as a background job of a script is.
        let mut c = Command::new("sh");
        c.args(["-c", "trap '' INT QUIT; exec \"$0\" mods", bin]).current_dir(&h.0).env_clear().env("HOME", &h.0).env("PATH", &path);
        let out = c.stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
        // HUP, INT, QUIT and TERM: bits 0, 1, 2 and 14.
        let mask = |name: &str| text(&out.stdout).lines().find_map(|l| l.strip_prefix(name)).map(|m| u64::from_str_radix(m.trim(), 16).unwrap() & 0x4007);
        assert_eq!((mask("SigBlk:"), mask("SigIgn:")), (Some(0), Some(0)), "{}", text(&out.stdout));
    }
}

/// No manager next to henki or on PATH: exit 2 with the fix, nothing read.
#[test]
fn mods_manager_missing() {
    let h = Home::new();
    for args in [&["mods"][..], &["allow", "rm", "1"][..]] {
        let out = h.run(args, &[]);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "{args:?}");
        assert_eq!(text(&out.stderr), "henki: henki mods needs henki-mods next to henki or on PATH: not found; fix: reinstall henki\n");
    }
}

/// The manager next to henki's realpath (the release tarball's bin/) wins over PATH, also when henki is run through a
/// link elsewhere (Homebrew links bin/henki into its prefix).
#[test]
fn mods_manager_beside_realpath() {
    let h = Home::new();
    let tarball = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("henki-cli-bin-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tarball);
    fake_manager(&tarball);
    fs::copy(env!("CARGO_BIN_EXE_henki"), tarball.join("henki")).unwrap();
    std::os::unix::fs::symlink(tarball.join("henki"), h.0.join("henki")).unwrap();
    write(&h.0.join("henki-mods"), "#!/bin/sh\necho wrong one\n", 0o755);
    let mut c = Command::new(h.0.join("henki"));
    c.arg("mods").current_dir(&h.0).env_clear().env("HOME", &h.0).env("PATH", format!("{}:/usr/bin:/bin", h.0.display()));
    let out = c.stdin(Stdio::null()).output().unwrap();
    let _ = fs::remove_dir_all(&tarball);
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with(&format!("bin {}\n", tarball.join("henki").display())), "{}", text(&out.stdout));
}

/// `henki allow` reaches the shipped manager's `allow`: the rules of the allow file in the config dir, or beside a
/// relative `--config`, and `rm N` (the manager's own tests hold the rest).
#[test]
fn allow_through_manager() {
    let h = Home::new();
    let path = format!("{}/manager:/usr/bin:/bin", env!("CARGO_MANIFEST_DIR"));
    let env = [("PATH", path.as_str())];
    write(&h.allow(), &format!("# mine\n/src/app\tgit status\n{}\tls -la\n", h.0.join("proj").display()), 0o600);
    let out = h.run(&["allow"], &env);
    assert_eq!(
        (out.status.code(), text(&out.stdout)),
        (Some(0), format!("{}\n1  /src/app  git status\n2  ~/proj  ls -la\n", h.allow().display()).as_str()),
        "{}",
        text(&out.stderr)
    );
    let out = h.run(&["allow", "rm", "1"], &env);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), "removed rule 1: /src/app  git status\n"), "{}", text(&out.stderr));
    assert_eq!(fs::read_to_string(h.allow()).unwrap(), format!("# mine\n{}\tls -la\n", h.0.join("proj").display()));
    fs::create_dir_all(h.0.join("alt")).unwrap();
    write(&h.0.join("alt/allow"), "/src/app\techo alt\n", 0o600);
    let out = h.run(&["allow", "--config", "alt/henki.toml"], &env);
    let here = fs::canonicalize(&h.0).unwrap();
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), format!("{}\n1  /src/app  echo alt\n", here.join("alt/allow").display()).as_str()), "{}", text(&out.stderr));
}

#[test]
#[ignore = "release gate"]
fn version_le_10ms() {
    let mut times: Vec<Duration> = (0..20)
        .map(|_| {
            let t = Instant::now();
            assert!(henki(&["--version"]).status.success());
            t.elapsed()
        })
        .collect();
    times.sort();
    println!("--version: median {:?}, max {:?}", times[10], times[19]);
    assert!(times[10] <= Duration::from_millis(10), "median {:?}", times[10]);
}
