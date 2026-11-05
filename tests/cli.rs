use std::process::{Command, Output};

fn henki(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_henki")).args(args).output().expect("run henki")
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("utf-8 output")
}

#[test]
fn version() {
    for flag in ["--version", "-V"] {
        let out = henki(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(text(&out.stdout), format!("henki {}\n", env!("CARGO_PKG_VERSION")));
        assert_eq!(text(&out.stderr), "");
    }
}

#[test]
fn help() {
    for flag in ["--help", "-h"] {
        let out = henki(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert!(text(&out.stdout).contains("usage: henki [--help | --version]\n"), "{}", text(&out.stdout));
        assert_eq!(text(&out.stderr), "");
    }
}

#[test]
fn unknown_argument() {
    for (args, says) in [(&["--bogus"][..], "--bogus"), (&["run"][..], "run"), (&["--version", "extra"][..], "extra"), (&[][..], "no argument")] {
        let out = henki(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&out.stdout), "", "{args:?}");
        let err = text(&out.stderr);
        assert!(err.starts_with("henki: ") && err.contains(says) && err.ends_with("usage: henki [--help | --version]\n"), "{args:?}: {err}");
    }
}
