//! Allow rules: the user's standing approvals for exact shell commands in one project root, kept in a plain text file
//! the user can read, edit and empty.
use crate::{sys, term};
use std::{ffi::OsString, fs, fs::File, fs::TryLockError, io, io::Read, io::Write, os::unix::fs::MetadataExt, os::unix::fs::OpenOptionsExt, path::Path, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub n: usize,
    pub root: String,
    pub cmd: String,
}

#[derive(Debug, Default)]
pub struct Allow {
    pub file: PathBuf,        // <config dir>/allow
    pub rules: Vec<Rule>,     // in file order; empty when the file is absent or not read (auto)
    pub hash: Option<String>, // FNV-1a of the bytes read at load; None when absent or not read
}

const MAX: u64 = 64 << 10;
const HEADER: &str = "# henki allow rules: ROOT<TAB>COMMAND, one per line; COMMAND then runs without asking in ROOT.
# List and remove them with /allow in henki or henki allow [rm N]; or edit this file.\n";

/// The allow file for rule use, at start. `read` false (auto mode): nothing is read. Fails closed.
pub fn load(file: &Path, read: bool) -> Result<Allow, String> {
    let (none, f) = (Allow { file: file.to_path_buf(), ..Allow::default() }, file.display());
    // O_NOFOLLOW: the open itself refuses a symlink, so none can be swapped in after a check.
    let fix = |e: io::Error| match e.raw_os_error() {
        Some(libc::ELOOP) => format!("{f}: a symlink; fix: put the file itself in its place"),
        _ => format!("{f}: {e}; fix: keep the rules in a regular file of at most 64 KiB"),
    };
    let Some(fd) = (if read { open(file, libc::O_NOFOLLOW).map_err(fix)? } else { None }) else { return Ok(none) };
    let (b, m) = bytes(&fd).and_then(|b| Ok((b, fd.metadata()?))).map_err(fix)?;
    if others(m.mode(), m.uid(), sys::euid()) {
        return Err(format!("{f}: another user can change it; fix: chown $USER {f}; chmod 600 {f}"));
    }
    let rules = numbered(&b).enumerate().map(|(k, (i, l))| rule(k + 1, l).map_err(|why| format!("{f}:{}: {why}; fix: henki allow rm {}, or edit the file", i + 1, k + 1)));
    Ok(Allow { rules: rules.collect::<Result<_, _>>()?, hash: Some(sys::fnv(&b)), ..none })
}

/// Rule `r` holds in the project root `root`.
pub fn applies(r: &Rule, root: &Path) -> bool {
    r.root == "*" || Path::new(&r.root) == root
}

impl Allow {
    /// The rule whose COMMAND is `cmd` byte for byte and whose ROOT is `root` or `*`.
    pub fn find(&self, cmd: &str, root: &Path) -> Option<&Rule> {
        self.rules.iter().find(|r| r.cmd == cmd && applies(r, root))
    }

    /// A confirmed `always`: append `ROOT<TAB>CMD\n`; Ok(the new rule's number).
    pub fn append(&mut self, root: &Path, cmd: &str) -> io::Result<usize> {
        let root = root.to_str().filter(|_| fits(cmd, root)).ok_or(io::ErrorKind::InvalidInput)?;
        let (f, new) = match sys::open_append(&self.file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && self.hash.is_none() => (sys::create_excl(&self.file, 0o600, true)?, true),
            r => (r?, false),
        };
        lock(&f)?;
        let b = if new { Vec::new() } else { bytes(&f)? };
        // A last line without its newline stays a line of its own.
        let gap = if b.is_empty() || b.ends_with(b"\n") { "" } else { "\n" };
        let line = format!("{}{gap}{root}\t{cmd}\n", if new { HEADER } else { "" });
        // Past the limit every start would fail closed, and list and remove would refuse the file.
        if (b.len() + line.len()) as u64 > MAX {
            return Err(io::Error::other(format!("would be over {} bytes", term::num(MAX))));
        }
        (&f).write_all(line.as_bytes()).and_then(|()| f.sync_all())?;
        if !intact(b.len() as u64, line.len() as u64, &f.metadata()?, &fs::symlink_metadata(&self.file)?) {
            return Err(io::Error::other("changed while it was written"));
        }
        let n = numbered(&b).count() + 1;
        self.rules.push(Rule { n, root: root.into(), cmd: cmd.into() });
        Ok(n)
    }
}

/// After the write: the file grew by exactly what was written, and its path still names the file written to.
fn intact(old: u64, wrote: u64, fd: &fs::Metadata, path: &fs::Metadata) -> bool {
    fd.len() == old + wrote && (fd.dev(), fd.ino()) == (path.dev(), path.ino())
}

/// `cmd` can be a rule's COMMAND, without a run of more than 32 spaces (a display collapses those), and `root` a rule's ROOT.
pub fn fits(cmd: &str, root: &Path) -> bool {
    !cmd.contains(&" ".repeat(33)) && root.to_str().is_some_and(|r| r.starts_with('/') && rule(0, format!("{r}\t{cmd}").as_bytes()).is_ok())
}

/// `/allow` and `henki allow`: the file's path, then one line per rule. Lenient: any mode, invalid lines listed as such.
pub fn list(file: &Path, home: &Path) -> Vec<String> {
    let b = open(file, 0).and_then(|f| f.map(|f| bytes(&f)).transpose()).map_err(|e| format!("cannot read it: {e}"));
    let rules = b.map(|b| numbered(&b.unwrap_or_default()).enumerate().map(|(k, (_, l))| format!("{}  {}", k + 1, show(l, home))).collect());
    [vec![file.display().to_string()], rules.unwrap_or_else(|e| vec![e])].concat()
}

/// Removes rule `n`, every other byte kept. Ok: `removed rule N: ROOT  COMMAND`; Err: `no rule N in FILE`, or the IO error.
pub fn remove(file: &Path, n: usize, home: &Path) -> Result<String, String> {
    let err = |e: io::Error| format!("{}: {e}", file.display());
    let none = || format!("no rule {n} in {}", file.display());
    let f = open(file, 0).map_err(err)?.ok_or_else(none)?;
    let b = lock(&f).and_then(|()| bytes(&f)).map_err(err)?;
    let (i, l) = n.checked_sub(1).and_then(|k| numbered(&b).nth(k)).ok_or_else(none)?;
    let rest: Vec<&[u8]> = b.split(|&c| c == b'\n').enumerate().filter(|&(j, _)| j != i).map(|(_, l)| l).collect();
    // The directory's realpath: a config directory that is a symlink (dotfiles) works as it does for load and append.
    let dir = fs::canonicalize(file.parent().unwrap_or(Path::new("/"))).and_then(|d| sys::Dir::open(&d)).map_err(err)?;
    let tmp = OsString::from(format!(".allow.henki-{}", std::process::id()));
    let write = |mut t: File| t.write_all(&rest.join(&b'\n')).and_then(|()| t.sync_all());
    let done = dir.create(&tmp, 0o600).and_then(write).and_then(|()| dir.rename(&tmp, file.file_name().unwrap_or_default()));
    done.inspect_err(|_| dir.unlink(&tmp)).map_err(err)?;
    Ok(format!("removed rule {n}: {}", show(l, home)))
}

/// A path with `$HOME` shown as `~`.
pub fn shown(p: &Path, home: &Path) -> String {
    p.strip_prefix(home).map_or_else(|_| p.display().to_string(), |r| format!("~/{}", r.display()).trim_end_matches('/').into())
}

/// The file opened without blocking (`flags`: O_NOFOLLOW or 0), None when absent; anything but a regular file is an error.
fn open(file: &Path, flags: i32) -> io::Result<Option<File>> {
    match fs::OpenOptions::new().read(true).custom_flags(flags | libc::O_NONBLOCK | libc::O_NOCTTY).open(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(f) if !f.metadata()?.is_file() => Err(io::Error::other("not a regular file")),
        r => r.map(Some),
    }
}

/// All of `f` from where it stands; over 64 KiB is an error.
fn bytes(f: &File) -> io::Result<Vec<u8>> {
    let mut b = Vec::new();
    f.take(MAX + 1).read_to_end(&mut b)?;
    if b.len() as u64 > MAX { Err(io::Error::other(format!("over {} bytes", term::num(MAX)))) } else { Ok(b) }
}

fn lock(f: &File) -> io::Result<()> {
    f.try_lock().map_err(|e| if let TryLockError::Error(e) = e { e } else { io::Error::other("locked by another henki") })
}

/// The rule lines (not empty, not a comment; one trailing `\r` dropped) as (line index, bytes), in order: rule N is the
/// Nth, valid or not.
fn numbered(b: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    b.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l)).enumerate().filter(|(_, l)| !l.is_empty() && !l.starts_with(b"#"))
}

/// `ROOT<TAB>COMMAND`: ROOT `*` or an absolute path a terminal shows as it is; COMMAND printable ASCII, 1 to 200 bytes,
/// not starting with `#` or a space.
fn rule(n: usize, l: &[u8]) -> Result<Rule, &'static str> {
    let tab = l.iter().position(|&c| c == b'\t').ok_or("no TAB between ROOT and COMMAND")?;
    let (root, cmd) = (std::str::from_utf8(&l[..tab]).unwrap_or_default(), &l[tab + 1..]);
    if root != "*" && (!root.starts_with('/') || root.contains(|c: char| c.is_control() || term::escaped(c))) {
        return Err("ROOT is not * or an absolute path");
    }
    let printable = (1..=200).contains(&cmd.len()) && cmd.iter().all(|b| (0x20..=0x7e).contains(b)) && !matches!(cmd.first(), Some(b'#' | b' '));
    if printable { Ok(Rule { n, root: root.into(), cmd: String::from_utf8_lossy(cmd).into() }) } else { Err("COMMAND is not one printable-ASCII line of 1 to 200 bytes") }
}

/// `ROOT  COMMAND`, or `!LINE  (invalid: REASON)` with LINE escaped.
fn show(l: &[u8], home: &Path) -> String {
    rule(0, l).map_or_else(|why| format!("!{}  (invalid: {why})", term::escape(&String::from_utf8_lossy(l))), |r| format!("{}  {}", shown(Path::new(&r.root), home), r.cmd))
}

/// Someone else could change the file: an owner other than you or root, or group or world write.
fn others(mode: u32, uid: u32, euid: u32) -> bool {
    (uid != euid && uid != 0) || mode & 0o022 != 0
}

#[cfg(test)]
mod tests;
