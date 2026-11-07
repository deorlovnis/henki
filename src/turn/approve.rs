//! Asking the user: a call shown in full on the terminal, and the answer: y, always (a saved rule, after a
//! confirmation) or no.
use super::Session;
use crate::{allow, journal::rec, supervise, sys, term, tools, tools::Target, types::*};

pub const DENIED: &str = "denied by the user";

impl Session {
    /// Shows `c`, asks until there is an answer, journals `ask` before and `approval` after; true to run the call.
    /// INT and QUIT are a no; TERM, HUP and a closed stdout end the turn; a tripwire found before an append ends it
    /// `Error changed`.
    pub(super) fn ask(&mut self, c: &Call, t: Option<&Target>) -> Result<bool, Outcome> {
        self.record(rec::ask(c, t.map(|t| t.shown.as_str())))?;
        let (shell, cmd) = (c.name == "shell", c.args["command"].as_str().unwrap_or_default());
        // A rule saves an exact command; none while a process started earlier still runs, since it may be what the
        // command talks to.
        let bg = supervise::reap();
        let offer = shell && bg == 0 && allow::fits(cmd, &self.cfg.root);
        let ((rows, cols), lines) = (sys::winsize(0).unwrap_or((24, 80)), tools::display(c, t));
        sys::flush_input();
        self.out.reset();
        lines.iter().for_each(|l| self.out.line(l));
        let n = term::rows(&lines, usize::from(cols)) + 1;
        if n > usize::from(rows) {
            self.out.line(&format!("{} rows: taller than the screen; scroll up or answer n", term::num(n as u64)));
        }
        if shell && bg > 0 {
            self.notice(&format!("{} background processes still run; always is off until they end", term::num(bg as u64)))?;
        }
        let (root, file) = (allow::shown(&self.cfg.root, &self.cfg.home), allow::shown(&self.cfg.allow.file, &self.cfg.home));
        loop {
            self.out.prompt(if offer { "run? [y/N/always] " } else { "run? [y/N] " });
            match self.typed()?.as_str() {
                "y" | "yes" => return self.verdict(c, "yes", None),
                "always" if offer => {}
                "always" => {
                    self.out.note("always is not offered for this call; answer y or n");
                    continue;
                }
                _ => return self.verdict(c, "no", None),
            }
            sys::flush_input();
            self.out.reset();
            self.out.line(&format!("a rule allows whatever this command does in {root}, including the files and config it reads"));
            self.out.prompt(&format!("save rule for {root}: {cmd} ? [y/N] "));
            if self.typed()? != "y" {
                continue;
            }
            self.tripwire()?;
            match self.cfg.allow.append(&self.cfg.root, cmd) {
                Ok(n) => {
                    self.rearm();
                    self.out.line(&format!("added to {file} as rule {n} for {root}; /allow lists and removes rules"));
                    return self.verdict(c, "always", Some(n));
                }
                Err(e) => {
                    self.out.note(&format!("could not write {file}: {e}; ran once"));
                    return self.verdict(c, "yes", None);
                }
            }
        }
    }

    /// An answer, trimmed and in lower case; EOF, INT and QUIT give "".
    fn typed(&mut self) -> Result<String, Outcome> {
        match self.line() {
            Ok(a) => Ok(a.unwrap_or_default().trim().to_ascii_lowercase()),
            Err(Stop::Signal(libc::SIGINT | libc::SIGQUIT)) => Ok(String::new()),
            Err(s) => Err(s.into()),
        }
    }

    /// Journals the verdict; true unless it is `no`.
    fn verdict(&mut self, c: &Call, verdict: &str, rule: Option<usize>) -> Result<bool, Outcome> {
        self.record(rec::approval(&c.id, verdict, rule))?;
        Ok(verdict != "no")
    }
}
