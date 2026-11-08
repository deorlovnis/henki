# checkpoints: a snapshot before every write, edit and shell call; /undo, /rewind N and /checkpoints

An observer and commands for you, not for the model (they cost no prompt bytes):

- Before each `write`, `edit` and `shell` call henki runs `on/call_start.wait`, and waits for it (up to 2 s) before
  the call runs. In a git work tree it keeps a snapshot of every file git does not ignore, tracked or not, as the ref
  `refs/henki/checkpoints/SESSION/N`. It goes through a temporary index: your work tree, your index and HEAD are not
  touched, and `git log` does not show it. Outside git it copies the target of a `write` or `edit` to
  `$XDG_STATE_HOME/henki/checkpoints/SESSION/N/` (or notes that it did not exist); a `shell` call there has no
  snapshot, nor has a target whose path is not UTF-8 (henki sends no path for it).
- `/checkpoints`: this session's snapshots, oldest first.
- `/undo`: shows what changed since the newest snapshot (`git diff --stat`), asks, puts those files back, and drops the
  snapshot, so the next `/undo` goes one call further back. Files made since then are removed.
- `/rewind N`: the same for snapshot N; it drops N and every later one.

Ignored files (`.gitignore`) are never in a snapshot. Its git calls run no program your git config names (no
`core.fsmonitor`, hook, pager or filter driver such as Git LFS): a snapshot holds the files' bytes as they are, and
`/undo` puts those bytes back. A snapshot that takes over 2 s (a very large work tree) still finishes, but henki stops
waiting for it, says so, and runs the calls after it without waiting. The model is not told about an `/undo`: say so
in your next message. Old snapshots stay until you remove them:
`git for-each-ref --format='%(refname)' refs/henki/ | xargs -n 1 git update-ref -d`.
