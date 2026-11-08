# project: the project's AGENTS.md (or CLAUDE.md) at session start

A context source, not a tool: at every start henki runs `ctx/project`, and what it prints goes into the model's
system prompt after your own `AGENTS.md`. henki shows its size at start (`context project/project: N bytes`).

- It prints the `AGENTS.md` at the git root of the working directory, or `CLAUDE.md` when there is no `AGENTS.md`,
  under `# project instructions (PATH)`, then `git: ROOT on BRANCH`. Outside a git work tree, or in a repository at
  or above your home directory (a dotfiles repo), it prints nothing.
- A file over 15,000 bytes is cut at a line end, with a line telling the model to read the file for the rest.
- It reads regular files only, never a link (a cloned repository could link `AGENTS.md` to a secret), and runs only
  `git rev-parse` and `git symbolic-ref`, with `core.fsmonitor` off, so a repository's own config runs nothing.
- Your global `AGENTS.md` (in henki's config dir) is never printed twice.

The model, or anyone who can write in the project, can change these files for your next session: read a changed
`AGENTS.md` as you would a changed script.
