# checkpoints: a snapshot before every write, edit and shell call; /undo, /rewind N and /checkpoints

An observer and commands for you, not for the model (they cost no prompt bytes):

- Before each `write`, `edit` and `shell` call henki runs `on/call_start.wait`, and waits for it (up to 2 s) before
  the call runs. In a git work tree it keeps a snapshot of every file git does not ignore, tracked or not, as the ref
  `refs/henki/checkpoints/SESSION/N`. It goes through a temporary index: your work tree, your index and HEAD are not
  touched, and `git log` does not show it. Outside git it copies the target of a `write` or `edit` to
  `$XDG_STATE_HOME/henki/checkpoints/SESSION/N/` (or notes that it did not exist); a `shell` call there has no
  snapshot, nor has a target whose path is not UTF-8 (henki sends no path for it). In a git work tree it copies so,
  next to the commit, a target the commit cannot hold: outside the work tree, in a repo nested in it, or ignored;
  `/checkpoints` marks such a snapshot `(target kept by path)`. A file a `shell` call changes outside the work tree is
  in no snapshot.
- `/checkpoints`: this session's snapshots, oldest first.
- `/undo`: shows what changed since the newest snapshot (`git diff --stat`), the files made since then and the targets
  kept by path, asks, puts the changed files and the kept targets back, removes the made ones, and drops the snapshot,
  so the next `/undo` goes one call further back. Names are shown as git quotes them: a control character never
  reaches your terminal.
  A made file that the snapshot's own ignore rules ignore stays: it was there then (a `.env` that an edit to
  `.gitignore` uncovered), and no snapshot holds it.
- `/rewind N`: the same for snapshot N, with the targets every later snapshot kept by path; it drops N and every later
  one.

Ignored files (`.gitignore`, under the rules as they are when the snapshot is made) are not in a snapshot. So once an
edit drops a rule, the files it hid are in the snapshots after it, kept by refs: a `git push --mirror` would publish
them. The mod's git calls run no program your git config names (no `core.fsmonitor`, hook, pager or filter driver such
as Git LFS): a snapshot holds the files' bytes as they are, and `/undo` puts those bytes back. A filter driver whose
name holds `=` cannot be turned off so: in such a repo no snapshot is made and `/undo` refuses. A file or directory
git cannot read (mode 000, a nested repo with no commit) is left out of the snapshot, marked `(partial)`. henki shows
an observer's output to no one, so `/checkpoints`, `/undo` and `/rewind` first list the calls that got no snapshot or
a partial one, with git's reasons. A snapshot that takes over 2 s (a very large work tree) still finishes, but henki
stops waiting for it, says so, and runs the calls after it without waiting. The model is not told about an `/undo`:
say so in your next message. Old snapshots stay until you remove them:
`git for-each-ref --format='%(refname)' refs/henki/ | xargs -n 1 git update-ref -d`.
