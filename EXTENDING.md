# Extending henki

henki is a small kernel: a turn loop with its safety rules, and six ports (`model`, `history`, `context`, `exec`,
`policy`, `frontend`), each with a standard binding built in. Everything else is a **module** you add: a tool the
model learns about, a command for you, an observer of a session, a context source, or a binding that replaces one of
the ports. This guide takes you from nothing to a working module in ten minutes, then through each kind. The formats
are specified in [CONTRACTS.md](CONTRACTS.md); where this guide and it differ, CONTRACTS.md is right.

You need henki with its module manager, which comes with it (`henki mods ARGS` runs `henki-mods ARGS`; in a henki
checkout it is `manager/henki-mods`), a model server henki can talk to (README.md, Quick start), git and a POSIX shell.

## Ten minutes to a module

**1. Make one (1 minute).**

```sh
henki mods new hello --cmd
```

`new` writes a repository directory `hello/` with the module `hello/hello/` in it, plus `test.sh`, a README and a
0BSD LICENSE beside it. A module may hold only what henki loads, so tests and notes live next to it, not in it:

```
hello/                     the repository
  hello/                   the module
    doc.md                 line 1: the summary; the rest: the doc
    cmd/hello              /hello in henki runs it on your terminal
    .henki/module.toml     the manifest
  test.sh                  your tests of the module's programs
  README.md  LICENSE
```

The kinds: `--tool` (the default: a command for the model, `bin/NAME`, with an eval task), `--cmd` (a command for you
that hands back a message), `--template` (a prompt template), `--observer EVENT`, `--port history` (a history binding
in bash that passes its conformance suite). `--dir D` puts it in `D/NAME` instead.

**2. Check it (1 minute).**

```sh
cd hello
henki mods test
```

```
ok   hello: layout, texts and links as an install checks them (0.1.0, 0 B of prompt, hd1:eba5f2...)
     | test.sh: ok
ok   test.sh: exit 0 (a scratch home)
test: ok
```

`test` checks every module in the directory the way an install does (layout, every text, links, the manifest, the
eval tasks, the names henki keeps, each `port/PORT`'s conformance suite, `shellcheck` when you have it), then runs
`./test.sh` with HOME and the XDG dirs in a scratch directory. It runs your code, as it should for your own module.

**3. Try it (3 minutes).**

```sh
henki mods try ./hello
```

henki starts with only `hello` on, in a scratch home that holds a copy of your `config.toml`. Type `/hello only the
scripts`: henki runs `cmd/hello` on your terminal, shows the message it hands back and asks `send? [Y/n]`; Enter
sends it to the model. `/exit` ends it, and the scratch home goes with it (`--keep` keeps it). Nothing of a try
reaches your own config, store or sessions.

**4. Install it (1 minute).**

```sh
henki mods add ./hello
```

The screen shows what the module adds to every request (0 bytes here), every program it brings and when each runs
(`cmd/hello (sh, 14 lines): when you type /hello`), where it connects, and its pin; `y` installs a read-only copy and
turns it on. Restart henki (or, inside henki, answer the restart question) and `/hello` is there. Your later edits do
not reach the installed copy: `henki mods update hello` takes them, after a screen of what changed.

**5. Make it yours (4 minutes).** Edit `hello/doc.md` (line 1 is the summary the install screen shows),
`hello/cmd/hello` and `test.sh`, and set `name`, `version` and `license` in `hello/.henki/module.toml`. To rename it,
rename the module directory, its command and the manifest's `name` together. `henki mods test` again; then share it
(Sharing, below).

## What a module is

A module is a directory named after it: 1 to 32 of `a-z`, `0-9`, `_` and `-`, starting with a letter or a digit, and
not a name henki keeps (`std`, `henki`, the port names, `mods`, `allow`, `help`, `exit`, `index`, `x-...`) or an
official module's.

```
NAME/doc.md              required: line 1 the summary, the rest the doc
NAME/bin/CMD             commands for the model, first on the PATH of its calls (then the module costs prompt bytes)
NAME/cmd/CMD             commands for you: /CMD in henki
NAME/cmd/CMD.md          a command's data (a prompt template's text)
NAME/on/EVENT            observers of the session's events
NAME/ctx/SOURCE          context sources: their output goes into the system prompt at each start (henki 0.3)
NAME/port/PORT           a binding of one of henki's ports, once henki mods swap binds it
NAME/eval/TASK/          tasks that measure what the module does for a model
NAME/.henki/module.toml  the manifest
NAME/.henki/card.md      optional: text for every request
NAME/.henki/...          data your own programs read (mode 644, never a link or a program)
NAME/README* LICENSE* CHANGELOG*   ignored by henki
```

- Anything else in a module (a `src/`, a `Makefile`, a `test.sh`) stops henki at start: keep it in the repository
  around the module.
- Every entry of `bin/`, `cmd/`, `on/`, `ctx/` and `port/` is a file its owner can run (`chmod 755`), named with
  `A-Z a-z 0-9 . _ -`. Every link in a module is relative and stays inside it.
- `doc.md` and each `cmd/*.md` are at most 16 KiB of UTF-8 with no control or invisible character (henki names the
  line that holds one). `on/` and `ctx/` run unasked, so they and the directories above them must not be writable by
  others.
- henki stamps every file it loads from a module when it starts. When one changes while henki runs, the turn ends and
  henki takes no more messages until you restart it: a module's new code runs only after a restart.

## Three lanes instead of hooks

A hook can block, rewrite and watch at once, so every hook is as dangerous as the most dangerous one and their order
matters. henki gives each power its own lane:

| Lane | Who | Power | How many |
|---|---|---|---|
| decide | port bindings (`port/`) | answers the one question its port asks: allow this call? what does the model see? where are sessions kept? where does a shell call run? | one per port |
| watch | observers (`on/`) | receives each event; nothing it does changes the session | any |
| add | tools, commands, templates, context sources, cards | adds typed things at fixed points, each counted and listed | any |

Coming from a harness with hooks:

| A hook elsewhere | In henki |
|---|---|
| before a tool call: allow, deny, ask | a `policy` binding |
| after a tool call | an observer on `call_end` |
| at session start or on each prompt: add context | a context source, or a `context` binding |
| stop, notification | an observer on `turn_end` or `question` |
| before compaction | a `context` binding |
| rewrite or wrap a command | not offered: bind `exec` (a sandbox, a box) instead |

## Tools for the model: `bin/` and `doc.md`

With a `bin/`, every request's system prompt gets, once for all modules,
`Extensions are commands on PATH; read the doc in brackets before first use.`, and for this module the line

```
- hello: greets someone by name: `hello [WHO]`; an example command that changes nothing [~/.config/henki/ext/hello/doc.md]
```

That line is all the module costs per request until the model reads the doc; the install screen and `henki mods list`
show the bytes. A module of commands for you, observers, sources or bindings (no `bin/`) costs no line.

**Line 1 for a small model.** The model decides from line 1 alone whether to reach for your tool, so put the use and
the main form in it: `greets someone by name: hello [WHO]` beats `hello: a tool`. Keep it under 160 bytes.

**The rest of doc.md** is read before the first use: each verb and its form, what it prints, the exit codes, one or two
examples, and the mistake a model is likely to make with the corrected form (`not hello --name X: hello X`). Keep it
short and exact; the model reads all of it.

**The commands** run as you, unsandboxed (unless an exec binding is bound), and henki asks before each call that runs
one. Give each a `--help`, one line on stderr per problem that says what to do next (`hello: takes one WHO, got 2;
fix: quote a name that holds spaces`), and the exit codes the doc lists. A command named like one on your PATH
shadows it, and henki loads it only once `config.toml` lists `shadow = ["hello/hello"]`; a wrapper runs the real one
with `PATH="$HENKI_USER_PATH" exec TOOL "$@"`. A call's environment also has `HENKI_SESSION`, `HENKI_VERSION` and
`HENKI_DEPTH`.

## Commands for you: `cmd/`

`/NAME TEXT` runs `cmd/NAME` on your terminal, with TEXT (trimmed) as one argument; the model is not told. It gets
the tool environment plus `HENKI_CONFIG` (the config dir), `HENKI_STATE` (the state dir), `HENKI_BIN` (henki),
`HENKI_MODULE` (its module's dir) and `HENKI_NEXT`: an empty file in which the command can hand something back.

| Line 1 of `$HENKI_NEXT` | henki then |
|---|---|
| `send`, then the text | shows the text whole and asks `send? [Y/n]` |
| `resume ID` | asks, then starts henki again on session ID |
| `resume ID@N` | asks, then forks session ID at its line N (a `user` record) into a new session |
| `new` | starts henki again on a new session |
| `restart` | starts henki again on this session (to load a change) |

A prompt template is the simplest command: the text in `cmd/NAME.md`, and `cmd/NAME` a link to `../.henki/template`
(the program `henki mods new NAME --template` writes), which hands the text back as `send` with TEXT after it. For more
templates in one module add `cmd/OTHER.md` and `ln -s ../.henki/template cmd/OTHER`. Two modules on with the same
`/NAME` stop henki at start; `henki mods off MOD/NAME` masks one of them.

When a frontend drives henki (`henki serve`), a command runs with no terminal: it cannot ask anything, its output goes
to the frontend, and so does what it hands back.

## Observers: `on/EVENT`

henki runs `on/EVENT` once per event, unasked, in a process group of its own, with the event as one JSON line on stdin
(`{"data":...,"event":"turn_end","seq":N,"session":"ID","v":1}`) and `HENKI_EVENT`, `HENKI_MODULE` and
`HENKI_STATE` in its environment. Events: `session_start`, `turn_start`, `call_start`, `call_end`, `question`,
`turn_end`, `session_end`, and `record` (every record). henki never waits for an observer, except
`on/turn_start.wait` and `on/call_start.wait`, for at most 2 s (a snapshot before a call runs, as `checkpoints` takes).
Its output and exit status change nothing. Keep it quick, leave no process behind, and run only files henki stamps
(the module's own programs). `henki mods new NAME --observer turn_end` writes one that logs each event. At most 16
observers over all modules.

## Context sources: `ctx/SOURCE` (henki 0.3)

henki runs every source of every module that is on once per start (a new session, a resume, a fork, a restart),
before the first request, in the session's working directory, as a call runs but never through a bound exec program.
What it prints goes into the system prompt after your `AGENTS.md`. All sources of a start share 2 seconds; a source
that fails, runs past them, prints more than 16 KiB or prints an invisible character is skipped with a notice, never
cut. Each start shows each source's bytes (`context todo/todo: 54 bytes`).

```sh
#!/bin/sh
# ctx/todo: the first lines of the project's TODO.md, in every request.
# A regular file only, never a link: a cloned repository can plant a link to a secret.
[ -f TODO.md ] && [ ! -L TODO.md ] || exit 0
echo "# open items (TODO.md)"
head -n 40 TODO.md
```

Rules: leave no process behind (a leftover is a background process of the session, and `always` is not offered while
it runs); never follow a link out of the project; do not run what a repository's own config can change (for git,
`git -c core.fsmonitor=false`, and no command that reads the index); cut your own text when it could pass 16 KiB. A
module with `ctx/` needs `requires = ["henki >= 0.3"]`. The official `project` module is the full example.

## Ports and bindings

| Port | Standard binding | A module's binding speaks | Example |
|---|---|---|---|
| `model` | the openai and anthropic dialects | (not bindable in this version) | |
| `history` | the session journal | `history/1` over `port/1` | `tree`, `henki mods new NAME --port history` |
| `context` | henki's elision of old tool output | `context/1` over `port/1` | the official `compact` |
| `exec` | `bash` on this machine | `exec/1`: a program put in front of every shell call | the official `sandbox` |
| `policy` | approvals and allow rules | `policy/1` over `port/1` | |
| `frontend` | the REPL, `henki run --json` | `front/1`: a program that runs `henki serve` | |

A module brings a binding as `port/PORT`; `henki mods swap PORT NAME` binds it once it passes the port's conformance
suite and you said yes, `henki mods swap PORT std` goes back, and `henki --port PORT=std` runs one session without it.
`henki mods test --port PORT ./NAME` runs the suite alone. A bound port is a notice at every start and is named in
the session record.

### `port/1`: the protocol

henki starts the program once per session, with no arguments, in a process group and session of its own, with the
tool environment (no modules' `bin/`), and talks JSON lines over stdin and stdout: one request at a time, one answer
each. The first is `hello`:

```
{"id":0,"method":"hello","params":{"henki":"0.3.0","port":"history","protocol":[1],"scope":"/home/you/src/app","state":"/home/you/.local/state/henki/port/history","versions":[1]}}
{"id":0,"result":{"caps":[],"impl":"0.1.0","name":"mine","protocol":1,"version":1}}
```

`state` is the binding's own dir: keep your files in a subdirectory named after your module. An answer is
`{"id":N,"result":R}` or `{"error":{"code":C,"message":S},"id":N}`. Exit when stdin closes, and leave no process
behind. A binding that times out, dies or writes a broken line is dead for the rest of the session; what henki does
then is the port's rule, below. `henki port PORT -- PROGRAM` talks to a program by hand: type
`{"method":"list","params":{"scope":"/tmp"}}` and read the answer.

### `history/1`

`create`, `open`, `read`, `append` (durable before it answers), `discard`, `list`, and, with cap `fork`, `fork`. henki
checks what a binding returns as it checks its own journal, and a history failure ends the turn: henki never falls
back to its own files in a session. `henki mods new NAME --port history` writes an 80-line bash binding that passes
the suite (no fsync, so a start, not a store); `tree` (github.com/deorlovnis/henki-tree) is the full example, with
forks kept by reference. `henki mods history import` copies henki's own sessions into a bound binding.

### `policy/1`

`decide` gets each call that passed henki's own checks and answers `allow` (with a reason henki shows), `ask` or
`deny`; with cap `record`, `record` keeps an `always`. henki's floor stays: a write to its own files is refused before
`decide`, an allow that touches them is asked instead, and a binding that fails turns its calls into questions.

### `context/1`

`fit` gets the whole history before every request, with the budget (`window`, `fixed`, `limit`), and answers
`{"keep":true}` (henki elides as without a binding) or `{"records":[...]}`, what the model sees. henki takes the
records only when every call and result in them is the history's own, unchanged and in order, they pair, the last is
the history's last, and they fit `limit`; otherwise henki's elision shapes that request, with a notice. Nothing a
binding answers is written to the journal. With the anthropic dialect it is not used. `compact` (python 3) is the
example: it summarises the older turns with your local model.

### `exec/1`

Not a protocol: henki runs every shell call of the model as `PROGRAM BASH --noprofile --norc -c COMMAND`, in the
session's directory, and treats PROGRAM as the call (its timeout, kill and sweep). PROGRAM runs its arguments after
its own setup, passes stdin, stdout and stderr through, exits with the command's status (126 with one line on stderr
for its own failures), and keeps the command in its own process tree. It should drop `SSH_AUTH_SOCK`. A program whose
commands run on another machine replaces argument 1 with that machine's bash. While one is bound the model gets the
`shell` tool only. A minimal one:

```sh
#!/bin/sh
# port/exec: every shell call of the model at a low CPU priority, without your ssh agent.
command -v nice >/dev/null 2>&1 || { echo "lowprio: no nice here; fix: henki mods swap exec std" >&2; exit 126; }
unset SSH_AUTH_SOCK
exec nice -n 10 "$@"
```

with `ports = ["exec"]`, `contracts = ["exec/1", "ext/2", "module/1"]` and `requires = ["henki >= 0.3"]` in its
manifest. `henki mods test --port exec ./lowprio` runs the checks R01 to R07 (output, exit status, arguments, stdin,
both streams, the kill, the environment). The official `sandbox` is the full example.

### `front/1`: a frontend

A frontend is not a module: it is a program that starts `henki serve` and talks to the person in henki's place (a
TUI, an editor panel). henki writes the session's records on stdout (as `henki run --json` does) and reads one JSON
command per line on stdin: `{"c":"send","text":...}` when idle, `{"c":"answer","id":ID,"verdict":"yes"}` at a
question (an `ask` record), `{"c":"command","name":...,"args":...}` for a module command, `{"c":"cancel"}` at any
time. The frontend is the approver: it must show the person what the REPL shows before it answers, above all before
`always`, which saves an allow rule for the root the `ask` record names. Start it in a session of its own without a
terminal, and turn the person's interrupt into `cancel`.

## The manifest: `.henki/module.toml`

Flat `key = value` lines: strings in double quotes, integers, arrays of strings, `#` comments. Unknown keys are
refused; henki itself never reads it. An index takes each array sorted and without repeats.

| Key | | What |
|---|---|---|
| `henki_module = 1` | required | the format |
| `name` | required | the directory's name |
| `version` | required | a label for people (an index wants `MAJOR.MINOR.PATCH`); installs are pinned by commit and hash |
| `license` | required | an SPDX id or expression |
| `summary` | optional | at most 160 bytes; defaults to doc.md's line 1 |
| `requires` | optional | `["henki >= 0.3"]`: the oldest henki it works with |
| `contracts` | optional | the formats it is written for (CONTRACTS.md's index); refused where one is unknown |
| `os` | optional | `["linux", "macos"]` |
| `needs` | optional | programs it runs (`git`, `python3`): looked for at install, a warning, never installed |
| `shadow` | optional | the `MODULE/COMMAND` lines your commands need in `config.toml` |
| `caps_net` | optional | `none`, `loopback` or `any`: where its programs connect (declared, not enforced) |
| `maintainer` | optional | your GitHub login |
| `ports` | optional | exactly the programs in `port/` |
| `asset_TARGET` | optional | a prebuilt program per machine (below) |

**A card**, `.henki/card.md`, is text the model should always have (a rule such as "never print a key's value"):
henki-mods writes the cards of the modules that are on into `~/.config/henki/AGENTS.md`, between `# >>> mods cards`
and `# <<< mods cards`. It costs its bytes in every request, so most modules should not have one.

**A prebuilt program**: `asset_aarch64_apple_darwin = ["bin/NAME", "https://...", "sha256:HEX"]` (also
`x86_64_apple_darwin`, `aarch64_unknown_linux_musl`, `x86_64_unknown_linux_musl` and the `_gnu` ones). The install
downloads this machine's over https and refuses other bytes. A review can say whether it rebuilt the program from
source; one that did not shows "binary NOT reproduced".

## Measuring: `eval/`

`eval/TASK/` holds `prompt.txt` (the task), `setup` (makes the task's files in an empty work dir), `check` (exit 0 is
a pass; it gets the model's answer on stdin) and optionally `fake-bin/` (stand-ins first on PATH) and `task.toml`
(`max_steps`, `max_seconds`); `eval/lib/` holds what the tasks share. `henki mods eval ./NAME` runs every task with
the module on and off, 3 repeats, each run in a scratch home inside the `sandbox` program, and prints the passes;
`--suite own,general` adds the general suite, 12 everyday shell tasks, which shows the module costs nothing on tasks it
is not for. `henki mods eval report RUN` gives the effect with its confidence interval, and `henki mods eval badge
RUN` says whether it earns a badge: the module's tasks better (CI above 0, sign test p at most 0.05) and the general
suite not worse. Write checks that judge the result, not the words, and give the model nothing in the task's files
that gives the answer away.

## Sharing

Push the repository anywhere git can clone from:

```sh
henki mods add https://github.com/YOU/hello#hello            # the default branch
henki mods add https://github.com/YOU/hello@v0.1.0#hello     # a tag, a branch or a commit
henki mods share hello                                       # the line that installs exactly these files
```

henki-mods copies the committed files of that directory and nothing else, pins the commit and a hash of the files,
and runs nothing from the repository. Its install screen marks the module UNREVIEWED in red, because it is: keep the
programs short enough to read, and say in the doc what each does. `share` prints `add URL@COMMIT#DIR --expect
hd1:HASH`, which installs exactly your files or refuses.

**The index** lists modules someone has read, so `henki mods add NAME` installs them with their review. For a
module that is committed and pushed, `henki mods publish ./hello` prints the index's two entry files
(`mods/NAME.toml`, `mods/NAME/VERSION.toml`), derived from the commit and checked by the index's rules, and the steps
of the pull request; `--eval RUN` adds a badge and the rows behind it. The reviewer reads every file of the module at
that commit, recomputes every key the entry derives from the code, and runs a binding's conformance suite apart from
the signing key; then the entry gets its `reviewed` and `reviewer`. What gets a module refused: code that does
something its doc does not say, network use it does not declare, binaries that cannot be rebuilt without saying so,
look-alike names, keys that do not match the code. Licenses the index takes: Apache-2.0, MIT, BSD-2-Clause,
BSD-3-Clause, 0BSD, ISC, MPL-2.0, Zlib, BSL-1.0, Unlicense, CC0-1.0, the GPL, LGPL and AGPL versions in the index's
CONTRIBUTING.md, and `OR`/`AND` expressions of them. `henki mods new` writes 0BSD.

## Where next

- [CONTRACTS.md](CONTRACTS.md): every format above, with its version and stability rule; the conformance suites are
  `share/henki/conform/` in a release (`manager/conform/` in the repository).
- [SECURITY.md](SECURITY.md): what henki guarantees around modules and bindings, and what it does not.
- The official modules in `mods/` (`share/henki/official/` in a release): `prompts` (templates), `sessions`
  (commands that hand back `resume`), `checkpoints` (a waited-for observer), `project` (a context source), `compact`
  (a context binding), `sandbox` (an exec binding), `halti` (tools with a card and an eval suite).
- The module template, `github.com/deorlovnis/henki-module-template`: what `henki mods new` writes, as a repository
  to copy.
