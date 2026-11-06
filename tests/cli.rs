#![allow(clippy::disallowed_methods, clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::string_slice)]
use std::process::{Command, Output};

const USAGE: &str = "usage: henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki --version | --help
";

fn henki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_henki")).args(args).output().expect("run henki")
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
    let cases = [(&[][..], "no command"), (&["--bogus"][..], "--bogus"), (&["--project-agents"][..], "--project-agents"), (&["--version", "extra"][..], "extra")];
    for (args, says) in cases {
        let out = henki(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&out.stdout), "", "{args:?}");
        let err = text(&out.stderr);
        assert!(err.starts_with("henki: ") && err.contains(says) && err.ends_with(USAGE), "{args:?}: {err}");
    }
}
