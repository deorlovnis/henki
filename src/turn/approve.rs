//! Asking the user: a call shown in full on the terminal, and the answer: y, always (a saved rule, after a
//! confirmation) or no.
use super::Session;
use crate::{tools::Target, types::Call, types::Outcome};

pub const DENIED: &str = "denied by the user";

impl Session {
    /// Shows `c`, asks until there is an answer, journals `ask` before and `approval` after; true to run the call. INT and
    /// QUIT are a no; TERM, HUP and a closed stdout end the turn; a tripwire found before an append ends it `Error changed`.
    pub(super) fn ask(&mut self, c: &Call, t: Option<&Target>) -> Result<bool, Outcome> {
        let _ = (c, t);
        todo!()
    }
}
