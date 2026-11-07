//! The data every module shares: events to MAIN, a parsed call, a model reply, a tool's output, and how a turn ends.
use serde_json::Value;
use std::time::Duration;

/// Everything that wakes MAIN. A tagged event carries the tag MAIN gave its worker (one per model attempt, one per tool
/// call); MAIN drops a tagged event whose tag is not the one it is waiting for.
#[derive(Debug)]
pub enum Ev {
    /// INT, QUIT, TERM or HUP, from the signal thread.
    Signal(i32),
    /// `run -`: the task read from stdin; `None` when it is over 1 MiB or unreadable.
    Line(Option<String>),
    /// A streamed piece of attempt `tag`'s reply.
    Part { tag: u32, part: Part },
    /// The end of attempt `tag`: exactly one per attempt.
    Reply { tag: u32, reply: Result<Reply, ProviderError> },
    /// The end of tool call `tag`: exactly one per call.
    Tool { tag: u32, out: ToolOutput },
    /// The output writer got EPIPE on stdout.
    OutputClosed,
}

/// A streamed piece of a reply. `Call` names a call slot the stream opened, so a cancelled stream can name what it dropped.
#[derive(Debug, PartialEq)]
pub enum Part {
    Text(String),
    Reasoning(String),
    Call(String),
}

/// Why a wait ended before its event: every busy point turns it into an `Outcome` the same way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stop {
    Signal(i32),
    Budget,
    Closed,
}

/// One tool call. `args` is `{}` when the arguments did not parse (that is what is replayed); `raw` keeps what was sent;
/// `error` set means the call does not run and this text is its result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
    pub raw: Option<String>,
    pub error: Option<String>,
}

/// A finished model reply. `finish` is the server's finish reason (`stop`, `tool_calls`, `length`, or what it sent).
#[derive(Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub reasoning: String,
    pub calls: Vec<Call>,
    pub finish: String,
    pub usage: Usage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

impl Usage {
    pub fn add(&mut self, o: Usage) {
        *self = Usage { input: self.input + o.input, output: self.output + o.output, cached: self.cached + o.cached };
    }
}

/// A failed model request. `retry` is the cause (`HTTP 429`, `connection reset`, `could not connect ...`) exactly when
/// the request may be retried.
#[derive(Debug, PartialEq)]
pub struct ProviderError {
    pub kind: &'static str,
    pub message: String,
    pub retry: Option<String>,
    pub retry_after: Option<Duration>,
}

/// What a tool worker reports. MAIN adds why it killed the call when it builds the result.
#[derive(Debug, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub bytes: u64,
    pub ms: u64,
    pub end: End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    Exit(i32),
    Signal(i32),
    File { failed: bool },
}

/// The model endpoint from `base_url`: `http://HOST[:PORT]/PATH`.
#[derive(Clone, Debug, PartialEq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// How a turn ends. `exit()` is the one exit table.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Ok,
    GaveUp(&'static str),
    Error { kind: &'static str, message: String },
    NeedsApproval(Call),
    Budget,
    Interrupted,
    OutputClosed,
}

impl Outcome {
    /// The `status` word of the `turn_end` and `result` records.
    pub fn status(&self) -> &'static str {
        self.row().0
    }

    /// The process exit code.
    pub fn exit(&self) -> i32 {
        self.row().1
    }

    /// The exit table: each end's status word and exit code.
    fn row(&self) -> (&'static str, i32) {
        match self {
            Outcome::Ok => ("ok", 0),
            Outcome::GaveUp(_) => ("gave_up", 1),
            Outcome::Error { .. } => ("error", 2),
            Outcome::NeedsApproval(_) => ("needs_approval", 3),
            Outcome::Budget => ("budget", 124),
            Outcome::Interrupted => ("interrupted", 130),
            Outcome::OutputClosed => ("output_closed", 141),
        }
    }

    /// `GaveUp`'s reason: `max_steps`, `stuck` or `empty_answer`.
    pub fn reason(&self) -> Option<&'static str> {
        if let Outcome::GaveUp(r) = self { Some(r) } else { None }
    }

    /// REASON of `not run: REASON` for the calls this end leaves unstarted.
    pub fn not_run(&self) -> &'static str {
        match self {
            Outcome::NeedsApproval(_) => "needs approval",
            Outcome::Budget => "budget",
            Outcome::OutputClosed => "output closed",
            _ => "interrupted",
        }
    }

    /// The ends that kill every process group at once.
    pub fn stops(&self) -> bool {
        matches!(self, Outcome::Interrupted | Outcome::Budget | Outcome::OutputClosed)
    }

    pub fn error(kind: &'static str, message: impl Into<String>) -> Outcome {
        Outcome::Error { kind, message: message.into() }
    }
}

impl From<Stop> for Outcome {
    fn from(s: Stop) -> Outcome {
        match s {
            Stop::Signal(_) => Outcome::Interrupted,
            Stop::Budget => Outcome::Budget,
            Stop::Closed => Outcome::OutputClosed,
        }
    }
}

#[cfg(test)]
mod tests;
