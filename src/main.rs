//! henki: a small terminal agent. `henki run [--json] TASK` drives a model through a shell and three file tools.
// Removed when the modules are wired: until then most of the crate has no caller, and the stubs' parameters are unused.
#![allow(dead_code, unused_variables)]

mod config;
mod files;
mod http;
mod journal;
mod openai;
mod supervise;
mod sys;
mod term;
mod tools;
mod turn;
mod types;

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage: henki run [--json] [--max-steps N] [--max-seconds N] [--approve ask|auto] [--config FILE] [--model NAME] [--resume[=ID]] [--nested] [TASK | -]
       henki --version | --help
";

fn main() -> ExitCode {
    match args() {
        Ok(text) => {
            write(true, &text);
            ExitCode::SUCCESS
        }
        Err(e) => {
            write(false, &format!("henki: {e}\n{USAGE}"));
            ExitCode::from(2)
        }
    }
}

/// `--version` or `--help`; `run` is not wired yet, so it is refused like any other argument.
fn args() -> Result<String, lexopt::Error> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_env();
    let text = match p.next()? {
        Some(Long("version")) => {
            let commit = option_env!("HENKI_COMMIT").unwrap_or("unknown");
            format!("henki {} ({commit}, {}-{})\n", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH, std::env::consts::OS)
        }
        Some(Long("help")) => format!("henki {}: a small terminal agent\n\n{USAGE}", env!("CARGO_PKG_VERSION")),
        Some(arg) => return Err(arg.unexpected()),
        None => return Err("no command given".into()),
    };
    if let Some(arg) = p.next()? {
        return Err(arg.unexpected());
    }
    Ok(text)
}

#[allow(clippy::disallowed_methods)] // temporary: main writes through term::Out once the modules are wired
fn write(stdout: bool, text: &str) {
    let _ = if stdout { std::io::stdout().lock().write_all(text.as_bytes()) } else { std::io::stderr().lock().write_all(text.as_bytes()) };
}
