//! Asking the user: a call shown in full on the terminal, and the answer: y, always (a saved rule, after a
//! confirmation) or no.
use super::Session;
use crate::{allow, journal::rec, supervise, sys, term, tools, tools::Target, types::*};

pub const DENIED: &str = "denied by the user";

impl Session {
    /// Shows `c`, asks until there is an answer, journals `ask` before and `approval` after; the verdict (`yes`, or
    /// `always` with the rule's number) to run the call, None for no.
    /// `always`: the policy can save a rule for it. INT and QUIT are a no; EOF, TERM, HUP and a closed stdout end the
    /// turn; a tripwire found before the policy records a rule ends it `Error changed`. The `ask` record says when
    /// `always` is offered, and the root its rule would be saved for; `henki serve`'s frontend confirms it itself.
    pub(super) fn ask(&mut self, c: &Call, t: Option<&Target>, always: bool) -> Result<Option<(&'static str, Option<usize>)>, Outcome> {
        let (shell, cmd) = (c.name == "shell", c.args["command"].as_str().unwrap_or_default());
        // A rule saves an exact command; none while a process started earlier still runs, since it may be what the
        // command talks to.
        let bg = supervise::reap();
        let offer = always && shell && bg == 0;
        self.record(rec::ask(c, t.map(|t| t.shown.as_str()), self.cfg.root.to_str().filter(|_| offer)))?;
        let ((rows, cols), lines) = (sys::winsize(0).unwrap_or((24, 80)), tools::display(c, t));
        self.out.control(term::RESET);
        lines.iter().for_each(|l| self.out.line(l));
        let n = term::rows(&lines, usize::from(cols)) + 1;
        (n > usize::from(rows)).then(|| self.out.line(&format!("{} rows: taller than the screen; scroll up or answer n", term::num(n as u64))));
        (shell && bg > 0).then(|| self.notice(&format!("{} background processes still run; always is off until they end", term::num(bg as u64)))).transpose()?;
        let root = allow::shown(&self.cfg.root, &self.cfg.home);
        loop {
            match self.typed(if offer { "run? [y/N/always] " } else { "run? [y/N] " })?.as_str() {
                "y" | "yes" => return self.verdict(c, "yes", None),
                "always" if !offer => self.reject("always is not offered for this call; answer y or n"),
                "always" => {
                    self.out.control(term::RESET);
                    self.out.line(&format!("a rule allows whatever this command does in {root}, including the files and config it reads"));
                    if !self.out.json() && !matches!(self.typed(&format!("save rule for {root}: {cmd} ? [y/N] "))?.as_str(), "y" | "yes") {
                        continue;
                    }
                    self.tripwire()?;
                    match self.ports.policy.record(c, &mut self.cfg) {
                        Ok((line, n)) => {
                            // Only std writes a rule (a number): an outside binding's keeping absorbs no change.
                            n.is_some().then(|| self.rearm());
                            self.out.line(&line);
                            return self.verdict(c, "always", n);
                        }
                        Err(e) => {
                            self.out.note(&format!("{e}; ran once"));
                            return self.verdict(c, "yes", None);
                        }
                    }
                }
                _ => return self.verdict(c, "no", None),
            }
        }
    }

    /// The answer to `prompt`, trimmed and in lower case; INT and QUIT give "". A paste is never an answer: it gives "", a
    /// no, with a note, so no call ever runs on pasted text and the turn goes on as after any other no. EOF (Ctrl-D) ends
    /// the turn, and the REPL then quits.
    fn typed(&mut self, prompt: &str) -> Result<String, Outcome> {
        let a = self.reply(prompt)?;
        if self.eof { Err(Outcome::Interrupted) } else { Ok(a.unwrap_or_default()) }
    }

    /// The typed answer to `prompt`, trimmed and in lower case. None, a no, for a paste (with a note), INT, QUIT and
    /// EOF (the REPL then quits at its next prompt). After Ctrl-C, which leaves the cursor after the question, what
    /// follows starts on its own row. Err: TERM, HUP or a closed stdout.
    pub fn reply(&mut self, prompt: &str) -> Result<Option<String>, Stop> {
        match self.line(prompt) {
            Ok(Some((a, true))) => Ok(Some(a.trim().to_ascii_lowercase())),
            Ok(Some(_)) => Ok(None).inspect(|_| self.out.note("a paste is not an answer; taken as no")),
            Ok(None) => Ok(None),
            Err(Stop::Signal(libc::SIGINT | libc::SIGQUIT)) => Ok(None).inspect(|_| self.out.line("")),
            Err(s) => Err(s),
        }
    }

    /// `reply` is y or yes. Always yes in `henki serve`: what the question would start is a `next` record, which the
    /// frontend acts on or not.
    pub fn yes(&mut self, prompt: &str) -> Result<bool, Stop> {
        Ok(self.out.json() || matches!(self.reply(prompt)?.as_deref(), Some("y" | "yes")))
    }

    /// Journals the verdict; it and the rule unless it is `no`.
    fn verdict(&mut self, c: &Call, verdict: &'static str, rule: Option<usize>) -> Result<Option<(&'static str, Option<usize>)>, Outcome> {
        self.record(rec::approval(&c.id, verdict, rule, None))?;
        Ok((verdict != "no").then_some((verdict, rule)))
    }
}
