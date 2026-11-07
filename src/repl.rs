//! The line REPL: a banner, a prompt, `/` commands, `!cmd`, and one turn per message, on the terminal henki was
//! started from.
use crate::turn::Session;

/// From the banner to the exit code; closes the session on every path.
pub fn run(s: &mut Session) -> u8 {
    let _ = s;
    todo!()
}
