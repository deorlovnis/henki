//! The file tools, run on a worker thread: read (a file or a directory, bounded), write (atomic, through the directory's
//! descriptor) and edit (bytes, one exact span).
use crate::sys::{self, Dir};
use crate::term::num;
use crate::tools::{self, READ_MAX, Target};
use crate::types::{Call, End, Ev, ToolOutput};
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;
use std::{fs, thread};

/// Starts the worker for one validated file call; it sends exactly one `Ev::Tool { tag, .. }`.
pub fn start(c: Call, t: Target, cap: usize, tag: u32, tx: &Sender<Ev>) {
    let worker = tx.clone();
    let spawned = thread::Builder::new().name("file".into()).spawn(move || {
        let _ = worker.send(Ev::Tool { tag, out: run(&c, &t, cap) });
    });
    if let Err(e) = spawned {
        let text = format!("not started: {e}");
        let _ = tx.send(Ev::Tool { tag, out: ToolOutput { bytes: text.len() as u64, text, ms: 0, end: End::File { failed: true } } });
    }
}

/// The work itself: `End::File { failed }`; `bytes` = the result text length.
pub fn run(c: &Call, t: &Target, cap: usize) -> ToolOutput {
    let (t0, a) = (Instant::now(), &c.args);
    let (p, s) = (a["path"].as_str().unwrap_or_default(), |k: &str| a[k].as_str().unwrap_or_default());
    let r = match c.name.as_str() {
        "read" => read(p, t, a["offset"].as_u64().unwrap_or(1).max(1), a["limit"].as_u64().unwrap_or(2000).max(1), READ_MAX.min(cap)),
        "write" => put(p, t, s("content").as_bytes())
            .map(|()| format!("wrote {} bytes to {p} ({})", num(s("content").len() as u64), if t.mode.is_some() { "replaced" } else { "created" })),
        "edit" => edit(p, t, s("old_text"), s("new_text")),
        name => Err(format!("{name} is not a file tool")),
    };
    let (text, failed) = r.map_or_else(|e| (e, true), |s| (s, false));
    ToolOutput { bytes: text.len() as u64, ms: t0.elapsed().as_millis() as u64, text, end: End::File { failed } }
}

/// A file's lines or a directory's entries (sorted bytewise, `/` after subdirectories), through one pager.
fn read(p: &str, t: &Target, offset: u64, limit: u64, max: usize) -> Result<String, String> {
    let fail = tools::io_error(p);
    let (f, m) = sys::open_read(&t.path).map_err(fail)?;
    let (dir, mut total) = (m.is_dir(), m.len());
    let (what, unit) = if dir { ("directory", "entries") } else { ("file", "lines") };
    let mut r: Box<dyn BufRead> = if dir {
        let mut names: Vec<_> = fs::read_dir(&t.path).map_err(fail)?.flatten().map(|e| (e.file_name(), e.file_type().is_ok_and(|t| t.is_dir()))).collect();
        names.sort();
        total = names.len() as u64;
        let list: Vec<u8> = names.iter().flat_map(|(n, d)| [n.as_bytes(), if *d { &b"/\n"[..] } else { b"\n" }]).flatten().copied().collect();
        Box::new(Cursor::new(list))
    } else if m.is_file() {
        Box::new(BufReader::with_capacity(64 << 10, f))
    } else {
        return Err(tools::not_regular(p, m.file_type()));
    };
    if r.fill_buf().map_err(fail)?.iter().take(8 << 10).any(|&b| b == 0) {
        return Err(format!("binary file ({} bytes); use the shell", num(total)));
    }
    let (out, last, more) = page(&mut r, offset, limit, max).map_err(fail)?;
    match (out.is_empty(), dir) {
        (true, _) if last > 0 => return Err(format!("offset {offset} is past the end ({what} has {} {unit})", num(last))),
        (true, false) => return Ok("(empty file)".into()),
        _ => {}
    }
    let mut text = String::from_utf8_lossy(&out).into_owned();
    match (more, dir) {
        (true, false) => text += &format!("[lines {offset}-{last} shown; file is {} bytes; continue with offset={}]\n", num(total), last + 1),
        (true, true) => text += &format!("[entries {offset}-{last} shown of {}; continue with offset={}]\n", num(total), last + 1),
        (false, true) => text += &format!("[directory, {} entries]\n", num(total)),
        (false, false) => {}
    }
    if std::str::from_utf8(&out).is_err() {
        text += "[invalid UTF-8 replaced with U+FFFD]\n";
    }
    Ok(text)
}

/// From line `offset`, at most `limit` lines and `max` bytes of `r`. Returns the lines, the number of the last line
/// shown (or of the last line skipped when none is), and whether more remains. A line that does not fit ends the page,
/// unless it is the first: that one is cut with `[line cut: N bytes]`, so a page that reached a line is never empty.
fn page(r: &mut dyn BufRead, offset: u64, limit: u64, max: usize) -> io::Result<(Vec<u8>, u64, bool)> {
    let (mut out, mut buf, mut n) = (Vec::new(), Vec::new(), 0);
    while n + 1 < offset.saturating_add(limit) {
        let skip = n + 1 < offset;
        let room = if skip { 0 } else { max.saturating_sub(out.len()) };
        buf.clear();
        let Some(len) = line(r, room, &mut buf)? else { return Ok((out, n, false)) };
        n += 1;
        match (skip, len < room, out.is_empty()) {
            (true, ..) => {}
            (false, true, _) => {
                out.extend_from_slice(&buf);
                out.push(b'\n');
            }
            (false, false, false) => return Ok((out, n - 1, true)),
            (false, false, true) => {
                // Cut on a character boundary, so the cut alone never reads as invalid UTF-8.
                buf.truncate(std::str::from_utf8(&buf).err().filter(|e| e.error_len().is_none() && e.valid_up_to() > 0).map_or(buf.len(), |e| e.valid_up_to()));
                out.extend_from_slice(&buf);
                out.extend_from_slice(format!(" [line cut: {} bytes]\n", num(len as u64)).as_bytes());
                break;
            }
        }
    }
    Ok((out, n, !r.fill_buf()?.is_empty()))
}

/// One line of `r` without its newline: keeps at most `keep` bytes of it in `out` and returns its full length, or None
/// at the end. However long the line, it costs one buffer.
fn line(r: &mut dyn BufRead, keep: usize, out: &mut Vec<u8>) -> io::Result<Option<usize>> {
    let (mut n, mut any) = (0, false);
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(any.then_some(n));
        }
        any = true;
        let (take, nl) = buf.iter().position(|&b| b == b'\n').map_or((buf.len(), false), |i| (i, true));
        out.extend_from_slice(&buf[..take.min(keep.saturating_sub(n))]);
        n += take;
        r.consume(take + usize::from(nl));
        if nl {
            return Ok(Some(n));
        }
    }
}

/// Bytes, one exact span: `old_text` must occur exactly once (overlapping matches count); with no match, a `\n` in it
/// and `\r\n` in the file, both texts are matched again in CRLF form. Nothing outside the span changes.
fn edit(p: &str, t: &Target, old: &str, new: &str) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_text is empty".into());
    }
    let fail = tools::io_error(p);
    let (mut f, m) = sys::open_read(&t.path).map_err(fail)?;
    if !m.is_file() {
        return Err(tools::not_regular(p, m.file_type()));
    }
    let mut data = Vec::new();
    f.read_to_end(&mut data).map_err(fail)?;
    let (mut old, mut new) = (old.to_string(), new.to_string());
    let mut found = find(&data, old.as_bytes());
    if found.1 == 0 && old.contains('\n') && find(&data, b"\r\n").1 > 0 {
        (old, new) = (old.replace('\n', "\r\n"), new.replace('\n', "\r\n"));
        found = find(&data, old.as_bytes());
    }
    let at = match found {
        (Some(at), 1) => at,
        (_, 0) => return Err(format!("old_text not found in {p}")),
        (_, k) => return Err(format!("old_text occurs {k} times in {p}; include more context")),
    };
    let first = 1 + data[..at].iter().filter(|&&b| b == b'\n').count();
    data.splice(at..at + old.len(), new.bytes());
    put(p, t, &data)?;
    let (o, n) = (lines(old.as_bytes()), lines(new.as_bytes()));
    Ok(format!("edited {p}: lines {first}-{} ({o}) -> {n} lines", first + o - 1))
}

/// The first match of `needle` (not empty) in `hay` and the number of matches, overlapping ones included.
fn find(hay: &[u8], needle: &[u8]) -> (Option<usize>, usize) {
    let mut at = hay.windows(needle.len()).enumerate().filter(|w| w.1 == needle).map(|w| w.0);
    let first = at.next();
    (first, first.map_or(0, |_| 1 + at.count()))
}

/// The lines a text covers: `a\nb` and `a\nb\n` are two, `` none.
fn lines(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count() + usize::from(!b.is_empty() && !b.ends_with(b"\n"))
}

/// The atomic write through the directory's descriptor: missing parents made, the temp `.NAME.henki-PID-N` created
/// beside the target, written, synced, given the old mode without setuid and setgid, and renamed over the name; the temp
/// is unlinked on every error path. A symlink stays a symlink, because the target is already resolved.
fn put(p: &str, t: &Target, data: &[u8]) -> Result<(), String> {
    static N: AtomicU32 = AtomicU32::new(0);
    let fail = tools::io_error(p);
    let mut dir = Dir::open(&t.base).map_err(fail)?;
    if dir.id().map_err(fail)? != t.dir_id {
        return Err("target changed since approval; not run".into());
    }
    for m in &t.missing {
        dir = dir.sub(m, true).map_err(fail)?;
    }
    let mut tmp = OsString::from(".");
    tmp.push(&t.name);
    tmp.push(format!(".henki-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let mut f = dir.create(&tmp, if t.mode.is_some() { 0o600 } else { 0o644 }).map_err(fail)?;
    let r = (|| {
        f.write_all(data)?;
        f.sync_all()?;
        if let Some(mode) = t.mode {
            f.set_permissions(fs::Permissions::from_mode(mode & 0o1777))?;
        }
        dir.rename(&tmp, &t.name)
    })();
    if r.is_err() {
        dir.unlink(&tmp);
    }
    r.map_err(fail)
}

#[cfg(test)]
mod tests;
