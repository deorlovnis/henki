//! The file tools, run on a worker thread: read (a file or a directory, bounded), write (atomic, through the directory's
//! descriptor) and edit (bytes, one exact span).
use crate::tools::Target;
use crate::types::{Call, Ev, ToolOutput};
use std::sync::mpsc::Sender;

/// Starts the worker for one validated file call; it sends exactly one `Ev::Tool { tag, .. }`.
pub fn start(c: Call, t: Target, cap: usize, tag: u32, tx: &Sender<Ev>) {
    todo!()
}

/// The work itself: `End::File { failed }`; `bytes` = the result text length.
pub fn run(c: &Call, t: &Target, cap: usize) -> ToolOutput {
    todo!()
}
