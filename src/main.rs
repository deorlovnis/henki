//! henki: a small terminal agent.

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "usage: henki [--help | --version]\n";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprint!("henki: {e:#}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run() -> anyhow::Result<()> {
    use lexopt::prelude::*;
    let mut args = lexopt::Parser::from_env();
    let out = match args.next()? {
        Some(Long("version") | Short('V')) => format!("henki {}\n", env!("CARGO_PKG_VERSION")),
        Some(Long("help") | Short('h')) => format!("henki {}: a small terminal agent (early development)\n\n{USAGE}", env!("CARGO_PKG_VERSION")),
        Some(arg) => return Err(arg.unexpected().into()),
        None => anyhow::bail!("no argument given"),
    };
    if let Some(arg) = args.next()? {
        return Err(arg.unexpected().into());
    }
    std::io::stdout().lock().write_all(out.as_bytes())?;
    Ok(())
}
