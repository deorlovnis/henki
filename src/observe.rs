//! Observers (`observe/1`, the watch lane): a module's `on/EVENT` programs, run once per event with the event as one
//! JSON line on stdin, in the session's cwd, with the tool environment. They watch and cannot change the session: their
//! exit status and output are ignored (both go to the kernel log). The runs of `on/turn_start.wait` and
//! `on/call_start.wait` are waited for, all of an event's together, for at most `WAIT`; no other run is ever waited
//! for, and a run is never killed for being slow. An observer with `LIVE` runs still going misses events, with one
//! notice. Runs are kernel children: a turn's stop leaves them; the session end gives them `supervise::GRACE`, then
//! kills them.
use crate::{ext::Ext, supervise, sys, term, types::Ev, types::lock};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc::Sender};
use std::{ffi::OsString, fs, fs::File, io, path::Path, path::PathBuf, process::Stdio, thread, time::Duration};

/// The events: rev 2's seven, then `record`, every journal record a turn writes.
pub const EVENTS: [&str; 8] = ["session_start", "turn_start", "call_start", "call_end", "turn_end", "question", "session_end", "record"];
/// The journal records a turn writes that are events by another name (`close`, `session_end`, is the session's).
const NAMED: [(&str, &str); 4] = [("user", "turn_start"), ("tool_result", "call_end"), ("turn_end", "turn_end"), ("ask", "question")];
/// How long the `.wait` runs of one event are waited for, together.
pub const WAIT: Duration = Duration::from_secs(2);
const LIVE: usize = 4;
/// At most this many observer files over all modules (discovery refuses more).
pub const MAX: usize = 16;

/// One `on/` file: its module, its event, whether its runs are waited for (`.wait`, until one misses `WAIT`).
struct Observer {
    module: String,
    event: &'static str,
    wait: bool,
    path: PathBuf,
    missed: bool,
}

/// Every observer, and what their runs share: the tool environment, the cwd, the dir of the event files, the kernel
/// log, MAIN's channel, the live runs as (observer, pid), and the event counter.
pub struct Observers {
    all: Vec<Observer>,
    env: Vec<(OsString, OsString)>,
    cwd: PathBuf,
    dir: PathBuf,
    log: Option<File>,
    tx: Sender<Ev>,
    live: Arc<Mutex<Vec<(usize, i32)>>>,
    seq: u64,
}

/// An `on/` entry's event and whether its runs are waited for: `EVENT`, or `EVENT.wait` for turn_start and call_start.
pub fn parse(name: &str) -> Option<(&'static str, bool)> {
    let (e, wait) = name.strip_suffix(".wait").map_or((name, false), |e| (e, true));
    EVENTS.into_iter().find(|v| *v == e).filter(|v| !wait || ["turn_start", "call_start"].contains(v)).map(|v| (v, wait))
}

/// The event names of a journal record: its own, if it has one, then `record`.
pub fn of(rec: &Value) -> impl Iterator<Item = &'static str> + '_ {
    NAMED.iter().filter(move |n| rec["t"] == n.0).map(|n| n.1).chain(["record"])
}

impl Observers {
    /// The observers of `exts`. Their runs get `env` (the tool environment) plus HENKI_STATE (`state`), run in `cwd`,
    /// and write to the kernel log `log`, opened only when there is an observer.
    pub fn new(exts: &[Ext], env: Vec<(OsString, OsString)>, cwd: &Path, state: &Path, log: &Path, tx: Sender<Ev>) -> Observers {
        let one = |x: &Ext, n: &String| parse(n).map(|(event, wait)| Observer { module: x.name.clone(), event, wait, path: x.dir.join("on").join(n), missed: false });
        let all: Vec<Observer> = exts.iter().flat_map(|x| x.on.iter().filter_map(move |n| one(x, n))).collect();
        let log = (!all.is_empty()).then(|| sys::create_excl(log, 0o600, true).or_else(|_| sys::open_append(log)).ok()).flatten();
        let env = env.into_iter().chain([("HENKI_STATE".into(), state.into())]).collect();
        Observers { all, env, cwd: cwd.into(), dir: state.join("spill"), log, tx, live: Arc::default(), seq: 0 }
    }

    /// Runs `event`'s observers, the event numbered `seq`; their output goes to the kernel log, or /dev/null without
    /// one. Each wait run sends `Ev::Observed { tag }` once it is reaped (another run tag 0, which the session end's
    /// grace waits on): returns how many will send `tag`, and the notices.
    pub fn emit(&mut self, event: &str, session: &str, data: &Value, tag: u32) -> (usize, Vec<String>) {
        let (mut waits, mut notes, mut line) = (0, Vec::new(), None);
        self.seq += 1;
        let out = || self.log.as_ref().map_or(Ok(Stdio::null()), |l| l.try_clone().map(Stdio::from));
        for (i, o) in self.all.iter_mut().enumerate().filter(|(_, o)| o.event == event) {
            if lock(&self.live).iter().filter(|l| l.0 == i).count() >= LIVE {
                let first = !std::mem::replace(&mut o.missed, true);
                first.then(|| notes.push(format!("observer {}/on/{event} is slow: {LIVE} runs still going; it misses events until one ends", o.module)));
                continue;
            }
            let module = o.path.parent().and_then(Path::parent).unwrap_or(&o.path);
            let env: Vec<_> = self.env.iter().cloned().chain([("HENKI_EVENT".into(), event.into()), ("HENKI_MODULE".into(), module.into())]).collect();
            // Built once an observer runs: no cost for an event nobody watches.
            let line = line.get_or_insert_with(|| term::json_line(&json!({"v": 1, "seq": self.seq, "event": event, "session": session, "data": data})) + "\n");
            let io = input(&self.dir, line).and_then(|f| Ok([Stdio::from(f), out()?, out()?]));
            match io.and_then(|io| supervise::kernel_child(&o.path, &self.cwd, &env, io)) {
                Ok(child) => {
                    let (pid, live, tx, wait) = (child.id() as i32, Arc::clone(&self.live), self.tx.clone(), o.wait);
                    lock(&live).push((i, pid));
                    waits += usize::from(wait);
                    // The waiter owns the run: it sees the end without reaping, then reaps and forgets it under
                    // supervise's lock, so a pid henki may signal is never a recycled one. Without the thread the run
                    // is killed at the session end, and a wait for it runs out.
                    let _ = thread::Builder::new().spawn(move || {
                        // Should waitid fail, the run is left as if the thread had not started: never reaped, so
                        // its pid stays its own for the session end's kill.
                        if !sys::exited(pid) {
                            return;
                        }
                        supervise::kernel_end(Some(child), false);
                        lock(&live).retain(|l| l.1 != pid);
                        let _ = tx.send(Ev::Observed { tag: if wait { tag } else { 0 } });
                    });
                }
                Err(e) => notes.push(format!("observer {}/on/{event} did not start: {e}", o.module)),
            }
        }
        (waits, notes)
    }

    /// A wait on `event` ran out: its wait observers whose runs still go are not waited for again (rev 2: "then
    /// skipped"); one notice each.
    pub fn unwait(&mut self, event: &str) -> Vec<String> {
        let live: Vec<usize> = lock(&self.live).iter().map(|l| l.0).collect();
        let slow = self.all.iter_mut().enumerate().filter(|(i, o)| o.event == event && o.wait && live.contains(i));
        slow.map(|(_, o)| (o.wait = false, format!("observer {}/on/{event}.wait took over 2 s; it is no longer waited for", o.module)).1).collect()
    }

    /// The pids of the runs still going.
    pub fn running(&self) -> Vec<i32> {
        lock(&self.live).iter().map(|l| l.1).collect()
    }
}

/// The event line as the file a run reads on stdin, unlinked at once: no pipe a slow reader could fill. In the spill
/// dir, so the 24 h sweep takes one a crash left; a dot name never matches a session's spill file.
fn input(dir: &Path, line: &str) -> io::Result<File> {
    let p = dir.join(format!(".event-{}-{}", std::process::id(), sys::random4()?));
    let written = io::Write::write_all(&mut sys::create_excl(&p, 0o600, false)?, line.as_bytes());
    let f = written.and_then(|()| sys::open_read(&p).map(|r| r.0));
    let _ = fs::remove_file(&p);
    f
}

#[cfg(test)]
mod tests;
