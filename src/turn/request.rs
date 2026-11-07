//! The model request of a step: the provider thread over either wire format, retries and their waits, partial replies
//! and context elision.
use super::{ATTEMPTS, Session};
use crate::{anthropic, http, journal, journal::rec, openai, term, types::*};
use std::{sync::Arc, sync::Mutex, time::Instant};

/// `attempt` for one wire format.
type Attempt = fn(http::Resp, &[&'static str], &http::Limits, &mut dyn FnMut(Part)) -> Result<Reply, ProviderError>;

impl Session {
    /// One reply for the step. Pairing holds before every attempt (else nothing is sent); old tool results are elided
    /// once when the window fills or the server says it overflowed; a retryable error waits, then tries again. Every
    /// attempt that ends, the request timeout included, has its socket shut.
    pub(super) fn request(&mut self) -> Result<Reply, Outcome> {
        let (mut attempt, mut elided) = (1, false);
        loop {
            journal::pairing(&self.hist).map_err(|e| Outcome::error("internal", e))?;
            let mut body = self.body();
            if !elided && ctx_use(self.ctx, body.len()) * 5 >= self.cfg.window * 4 && self.elide()? {
                (elided, body) = (true, self.body());
            }
            let (tag, len) = (self.next_tag(), body.len());
            (self.t.partial, self.t.opened) = (String::new(), Vec::new());
            self.t.conn = Some(self.provider(tag, body).map_err(|e| Outcome::error("internal", format!("cannot start a thread: {e}")))?);
            let r = match self.wait(tag, Some(Instant::now() + self.lim.request)) {
                Ok(Some(Ev::Reply { reply, .. })) => reply,
                Ok(_) => Err(http::perr("provider", format!("request took over {} s", term::num(self.lim.request.as_secs())))),
                Err(s) => return Err(self.partial_reply("interrupted", s.into())), // FINISHING shuts the socket
            };
            self.t.conn.take().iter().for_each(http::cancel);
            match r {
                Ok(mut rep) => {
                    let u = &mut rep.usage; // the server's counts, capped so that no sum of them overflows
                    (u.input, u.output, u.cached) = (u.input.min(1 << 32), u.output.min(1 << 32), u.cached.min(1 << 32));
                    (self.ctx, self.t.opened) = ((u.input + u.output, len), Vec::new());
                    return Ok(rep);
                }
                Err(e) if e.retry.is_some() && attempt < ATTEMPTS => {
                    let d = http::retry_wait(attempt, e.retry_after);
                    let cause = e.retry.unwrap_or_default();
                    self.notice(&format!("{cause}; retry {attempt}/{} in {} s", ATTEMPTS - 1, term::secs(d.as_millis() as u64)))?;
                    let timer = self.next_tag(); // nothing answers it: an interruptible timer
                    self.wait(timer, Some(Instant::now() + d))?;
                    attempt += 1;
                }
                Err(e) if e.kind == "context_exceeded" && !elided && self.elide()? => elided = true,
                Err(e) => return Err(self.partial_reply("error", self.gave_up(e, attempt))),
            }
        }
    }

    /// One attempt on its own thread, in the session's wire format: it sends the streamed pieces, then exactly one
    /// `Ev::Reply`. MAIN cancels it through the returned slot.
    fn provider(&self, tag: u32, body: Vec<u8>) -> std::io::Result<http::Slot> {
        let slot: http::Slot = Arc::new(Mutex::new((false, None)));
        let (tx, url, key, offered, s) = (self.tx.clone(), self.cfg.url.clone(), &self.cfg.api_key, self.cfg.tools.clone(), slot.clone());
        let (loopback_only, lim) = (!key.is_empty() || self.cfg.url_from == "HENKI_BASE_URL", self.lim);
        let (path, headers, read): (_, _, Attempt) = match self.cfg.dialect {
            "anthropic" => (anthropic::PATH, anthropic::headers(key), attempt::<anthropic::Acc>),
            _ => (openai::PATH, openai::headers(key), attempt::<openai::Acc>),
        };
        std::thread::Builder::new().spawn(move || {
            let part = &mut |part| drop(tx.send(Ev::Part { tag, part }));
            let reply = http::post(&url, path, &headers, &body, loopback_only, &lim, &s).and_then(|resp| read(resp, &offered, &lim, part));
            let _ = tx.send(Ev::Reply { tag, reply });
        })?;
        Ok(slot)
    }

    /// A streamed piece of the attempt MAIN waits for: a `--json` delta, the REPL's streamed answer, the partial text, an
    /// opened call's name.
    pub(super) fn part(&mut self, p: Part) {
        match p {
            Part::Text(t) => {
                self.out.record(&rec::delta(false, &t));
                self.out.delta(&t);
                self.t.partial += &t;
            }
            Part::Reasoning(t) => self.out.record(&rec::delta(true, &t)),
            Part::Call(name) => self.t.opened.push(name),
        }
    }

    /// A request that ends without a reply journals its streamed text, if any, as an assistant record without calls
    /// (`finish` is `interrupted` or `error`); FINISHING names the calls it opened.
    fn partial_reply(&mut self, finish: &str, o: Outcome) -> Outcome {
        let text = std::mem::take(&mut self.t.partial);
        if text.is_empty() {
            return o;
        }
        self.record(rec::assistant(&text, &[], finish, Usage::default(), None)).err().filter(|_| !matches!(o, Outcome::Error { .. })).unwrap_or(o)
    }

    /// The error as the turn's outcome. After the last retry it says so, and a loopback server that refused every
    /// connect gets the fix.
    pub(super) fn gave_up(&self, e: ProviderError, attempt: u32) -> Outcome {
        let mut m = e.message;
        if let Some(cause) = e.retry {
            m += &format!(" (gave up after {attempt} attempts)");
            let (u, file) = (&self.cfg.url, self.cfg.file.as_ref().filter(|_| self.cfg.url_from == "config"));
            if cause.starts_with("could not connect") && http::loopback_literal(&u.host) {
                let set = if let Some(f) = file { format!("base_url in {}", f.display()) } else { "HENKI_BASE_URL".into() };
                m += &format!("; fix: start the model server on {} or set {set}", http::host_port(u));
            }
        }
        Outcome::error(e.kind, m)
    }

    /// Elides the old tool results and journals which; false when there was nothing to elide. Anthropic replays the
    /// history unchanged, so an overflow ends the turn.
    pub(super) fn elide(&mut self) -> Result<bool, Outcome> {
        let ids = if self.cfg.dialect == "openai" { journal::elide(&mut self.hist) } else { Vec::new() };
        Ok(!ids.is_empty() && self.record(rec::elide(&ids)).map(|()| true)?)
    }

    pub(super) fn body(&self) -> Vec<u8> {
        let body = if self.cfg.dialect == "anthropic" { anthropic::body } else { openai::body };
        body(&self.cfg.model, self.cfg.max_tokens, &self.system, &self.tools, &self.hist)
    }
}

/// One attempt's reply through wire format `W`: a whole JSON completion, or the stream until its end marker, or EOF or
/// idle after its finish.
fn attempt<W: Wire>(mut resp: http::Resp, offered: &[&'static str], lim: &http::Limits, part: &mut dyn FnMut(Part)) -> Result<Reply, ProviderError> {
    let mut acc = W::new(offered);
    if !resp.sse {
        return acc.json(&resp.body(http::JSON_MAX));
    }
    let mut flow = Flow::More;
    loop {
        match resp.event(flow == Flow::Finished, lim)? {
            None => return acc.finish(flow == Flow::Finished),
            Some((_, data)) => flow = acc.data(&data, part)?,
        }
        if flow == Flow::Done {
            return acc.finish(true);
        }
    }
}

/// The window in use before a request: the last reply's tokens plus a token per 4 bytes added since; before any usage,
/// a token per 4 bytes of the request.
pub(super) fn ctx_use((tokens, then): (u64, usize), body: usize) -> u64 {
    if tokens == 0 { body as u64 / 4 } else { tokens + body.saturating_sub(then) as u64 / 4 }
}
