#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
//! The command line: `--version`, `--help`, refused arguments, and the argument errors of `run`, all before any
//! config is read.
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki --version | --help
";

/// henki with an empty environment and a HOME that does not exist, so nothing outside the test is read.
fn henki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_henki")).args(args).env_clear().env("HOME", "/nonexistent/henki-cli-home").env("PATH", "/usr/bin:/bin").output().expect("run henki")
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("utf-8 output")
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
    let cases = [(&[][..], ""), (&["--bogus"][..], "--bogus"), (&["--project-agents"][..], "--project-agents"), (&["--version", "extra"][..], "extra"), (&["repl"][..], "repl")];
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
