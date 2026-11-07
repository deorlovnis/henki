#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The command line: `--version`, `--help`, refused arguments, the argument errors of `run`, the REPL's refusal
//! without a terminal, and `henki allow`, which lists and removes allow rules without reading a config.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: henki [--config FILE] [--approve ask|auto] [--model NAME] [--resume[=ID]] [--nested]
       henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki allow [--config FILE] [rm N]
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

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
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

#[test]
fn allow_list_and_rm() {
    let h = Home::new();
    let (file, mine) = (h.allow(), h.0.join("proj"));
    write(&file, &format!("# my rules\n/src/app\tgit status\n\n/src/app\tmake test\n{}\tls -la\n", mine.display()), 0o600);
    let out = h.run(&["allow"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("{}\n1  /src/app  git status\n2  /src/app  make test\n3  ~/proj  ls -la\n", file.display()));
    let out = h.run(&["allow", "rm", "1"], &[]);
    assert_eq!((out.status.code(), text(&out.stdout), text(&out.stderr)), (Some(0), "removed rule 1: /src/app  git status\n", ""));
    assert_eq!(fs::read_to_string(&file).unwrap(), format!("# my rules\n\n/src/app\tmake test\n{}\tls -la\n", mine.display()), "everything else byte for byte");
    assert_eq!(mode(&file), 0o600);
    let out = h.run(&["allow"], &[]);
    assert_eq!(text(&out.stdout), format!("{}\n1  /src/app  make test\n2  ~/proj  ls -la\n", file.display()));
    fs::remove_file(&file).unwrap();
    assert_eq!(text(&h.run(&["allow"], &[]).stdout), format!("{}\n", file.display()), "no file: the path alone");
}

#[test]
fn allow_config_flag() {
    let h = Home::new();
    let alt = h.0.join("alt");
    fs::create_dir_all(&alt).unwrap();
    write(&alt.join("allow"), "/src/app\techo alt\n", 0o600);
    write(&h.allow(), "/src/app\techo home\n", 0o600);
    let conf = alt.join("henki.toml");
    let out = h.run(&["allow", "--config", conf.to_str().unwrap()], &[]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), format!("{}\n1  /src/app  echo alt\n", alt.join("allow").display()).as_str()));
    let out = h.run(&["allow", "--config", conf.to_str().unwrap(), "rm", "1"], &[]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), "removed rule 1: /src/app  echo alt\n"), "{}", text(&out.stderr));
    assert_eq!(fs::read_to_string(alt.join("allow")).unwrap(), "");
    assert_eq!(fs::read_to_string(h.allow()).unwrap(), "/src/app\techo home\n", "the default file is untouched");
}

/// Listing and removing never fail closed: an invalid line and a mode others can write are shown, and `rm` works.
#[test]
fn allow_broken_file_lists() {
    let h = Home::new();
    write(&h.allow(), "/src/app\tgit status\nno tab here\n", 0o666);
    let out = h.run(&["allow"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("{}\n1  /src/app  git status\n2  !no tab here  (invalid: no TAB between ROOT and COMMAND)\n", h.allow().display()));
    let out = h.run(&["allow", "rm", "2"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("removed rule 2: "), "{}", text(&out.stdout));
    assert_eq!((fs::read_to_string(h.allow()).unwrap().as_str(), mode(&h.allow())), ("/src/app\tgit status\n", 0o600));
}

#[test]
fn allow_rm_missing_exit2() {
    let h = Home::new();
    write(&h.allow(), "/src/app\tgit status\n", 0o600);
    let out = h.run(&["allow", "rm", "9"], &[]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""));
    assert_eq!(text(&out.stderr), format!("henki: no rule 9 in {}\n", h.allow().display()));
    for args in [&["allow", "rm", "x"][..], &["allow", "rm"][..], &["allow", "clear"][..]] {
        let out = h.run(args, &[]);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "{args:?}");
        assert!(text(&out.stderr).starts_with("henki: ") && text(&out.stderr).ends_with(USAGE), "{args:?}: {}", text(&out.stderr));
    }
    assert_eq!(fs::read_to_string(h.allow()).unwrap(), "/src/app\tgit status\n");
}

#[test]
fn allow_ignores_nested() {
    let h = Home::new();
    write(&h.allow(), "/src/app\tgit status\n", 0o600);
    let out = h.run(&["allow"], &[("HENKI_DEPTH", "1")]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), format!("{}\n1  /src/app  git status\n", h.allow().display()).as_str()), "{}", text(&out.stderr));
}

/// A relative `--config` is taken from the working directory.
#[test]
fn allow_config_flag_relative() {
    let h = Home::new();
    fs::create_dir_all(h.0.join("alt")).unwrap();
    write(&h.0.join("alt/allow"), "/src/app\techo alt\n", 0o600);
    let out = h.run(&["allow", "--config", "alt/henki.toml"], &[]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), format!("{}\n1  /src/app  echo alt\n", h.0.join("alt/allow").display()).as_str()), "{}", text(&out.stderr));
}

/// N is a rule number: anything else is a usage error, and nothing is removed.
#[test]
fn allow_rm_not_a_number() {
    let h = Home::new();
    write(&h.allow(), "/src/app\tgit status\n", 0o600);
    for n in ["x", "-1", "18446744073709551616", ""] {
        let out = h.run(&["allow", "rm", n], &[]);
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "rm {n:?}");
        assert!(text(&out.stderr).starts_with("henki: ") && text(&out.stderr).ends_with(USAGE), "rm {n:?}: {}", text(&out.stderr));
    }
    assert_eq!(fs::read_to_string(h.allow()).unwrap(), "/src/app\tgit status\n");
}

/// Without an absolute HOME there is no allow file to name: the config's own refusal, exit 2, nothing read or written
/// below the working directory.
#[test]
fn allow_home_unset_refused() {
    let h = Home::new();
    fs::create_dir_all(h.0.join("rel/.config/henki")).unwrap();
    write(&h.0.join("rel/.config/henki/allow"), "/x\tls\n", 0o600);
    write(&h.0.join(".config/henki/allow"), "/x\tls\n", 0o600);
    for home in [None, Some("rel"), Some("")] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_henki"));
        c.args(["allow", "rm", "1"]).current_dir(&h.0).env_clear().env("PATH", "/usr/bin:/bin").stdin(Stdio::null());
        home.iter().for_each(|v| _ = c.env("HOME", v));
        let out = c.output().unwrap();
        assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""), "HOME {home:?}");
        assert_eq!(text(&out.stderr), "henki: HOME is not set; fix: set HOME to your home directory\n", "HOME {home:?}");
    }
    for f in [h.0.join("rel/.config/henki/allow"), h.0.join(".config/henki/allow")] {
        assert_eq!(fs::read_to_string(&f).unwrap(), "/x\tls\n", "{}", f.display());
    }
}

/// An allow file that is a FIFO is neither opened for reading nor waited on: the list names the problem, `rm` fails.
#[test]
fn allow_fifo_not_blocking() {
    let h = Home::new();
    assert!(Command::new("mkfifo").arg(h.allow()).status().unwrap().success());
    let run = |args: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_henki"));
        c.args(args).current_dir(&h.0).env_clear().env("HOME", &h.0).env("PATH", "/usr/bin:/bin").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > end {
                let _ = child.kill();
                panic!("henki {args:?} waits on the FIFO");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child.wait_with_output().unwrap()
    };
    let out = run(&["allow"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(0), format!("{}\ncannot read it: not a regular file\n", h.allow().display()).as_str()));
    let out = run(&["allow", "rm", "1"]);
    assert_eq!((out.status.code(), text(&out.stdout)), (Some(2), ""));
    assert_eq!(text(&out.stderr), format!("henki: {}: not a regular file\n", h.allow().display()));
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
