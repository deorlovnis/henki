# itse runs a workspace's tests, builds and language servers and answers with located results; on a Mac it is in a box

Run it as a command (henki uses no itse server or hook): `itse VERB [args]` where itse is on PATH, as inside a box.
On a Mac it runs in the workspace's box: `halti sh WS -- itse VERB [args]` (the box's folder is /work; `--root DIR`
names the workspace when the folder is not inside its git repo). `itse verbs` lists the verbs, their arguments and
this workspace's actions.

Ask (no change):
- `itse status`: the workspace, its items and their states. `itse problems [scope] [--new]`: compiler and language
  server problems.
- `itse find NAME [--kind K]`, `itse info NAME`, `itse refs NAME`, `itse outline PATH`: symbols, by name or file.
Run:
- `itse test [target] [filter]`: failing tests at their lines. `itse build [target]`: located problems.
- `itse run LABEL`: a declared action. `itse exec CMD...`: CMD in the workspace's tools. `itse cancel RUN`.
- `itse show ID [--page N] [--grep RE] [--tail N]`: a result, an item of it or a log by id, such as run:14 or q:7.
Only when the user asks: `trust`, `consent`, `use`, `unuse`, `install`, `lock`, `gc`, `integrate`, `daemon`.

Results: line 1 is `VERDICT verb scope — counts; cost; id` (`FAIL test :data — 2 failed, 141 passed; 38.2 s;
run:14`); each item's id is a letter (E error, W warning, T test, S symbol, R reference) and its number, and
`itse show run:14/ITEM` opens it.

Exit codes:
- 0 ok, or an empty listing.
- 1 the verb reports failure: failing tests, a build with errors, a name that `find`, `info` or `refs` did not find.
- 2 error: itse could not do it; read the message.
- 3 a human acts next (NEEDS_HUMAN, NEEDS_CONSENT, UNTRUSTED; BLOCKED: the box's door refused a host): tell the user
  what it names; do not run it yourself.
- 4 running: the run outlasted the wait; `itse show run:N` follows it.
- 130 interrupted.
