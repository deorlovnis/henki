# henki

henki is a small terminal agent in early development. It gives your task to a model and lets the model act through four tools (`shell`, `read`, `write`, `edit`) until it answers. On your machine it is a line REPL that talks to a local model server (plain HTTP, OpenAI-compatible) and asks before a command runs; in an isolated Linux environment it runs headless, one static binary with no install step.

## Usage

```sh
mkdir -p ~/.config/henki && cp $(brew --prefix)/share/henki/config.mac.toml ~/.config/henki/config.toml   # then set model and base_url
                                        # (from a tarball: share/henki/config.mac.toml; from a checkout: profiles/)
henki-mods add halti                    # if you use halti: its doc, wrappers and card (1.1 KB into every request)
henki                                   # the REPL
henki run "why does the build fail?"    # one task, headless
henki run --json --approve auto --max-seconds 600 "fix the failing test"
echo "a long task" | henki run -
```

### The REPL

`henki` starts a line REPL on the terminal it was started from (without a terminal on stdin and on stderr it refuses: use `henki run`). Its banner names the model, the server and its dialect, the approval mode, the config file, the extensions (or that no module is on), any notices, and every allow rule for this project. A `base_url` that is not a loopback address (`localhost`, `127.x.x.x`, `[::1]`) gets a notice at every start: prompts, files and command output go to that host, unencrypted. What you send with Enter at the `> ` prompt is one message. The answer streams to stdout; the banner, the prompt, what you type, questions, call lines and notices go to stderr, so `henki > answers.txt` keeps only the answers.

- `/exit` or Ctrl-D on an empty line quits (Ctrl-D at a question ends the turn first, then quits). `/help` lists the commands, the modules' too. `/mods ARGS` runs the module manager, `henki-mods ARGS`, on this terminal (the one shipped with henki, never a module's command), and `/allow` (`/mods allow`) lists the allow rules and `/allow rm N` removes rule N. `/NAME TEXT` runs a module's command `cmd/NAME` on this terminal, with TEXT as one argument; the model is not told. A command can hand something back: a message, which henki shows whole and sends when you press Enter (`send? [Y/n]`), or a new henki on another session, a new one, or this one again (`/resume`, `/new` and `/reload` of the `sessions` module), which closes this session and starts henki again with the same arguments. When a command or the manager changed a trust file, henki lists it, sends nothing, and asks `restart on these changes? [y/N]`; n stops the REPL until you restart henki. After a change to the history binding (`/mods swap history ...`) that restart starts a new session: the old one stays in the store it was kept in, and henki says how to reach it. A trust file that changed before the command (an edit in another terminal, a `!cmd`) gets no one-key restart, `/reload` included: henki says so, and `henki --resume` continues this session. Any other line that starts with `/` is an unknown command and never reaches the model.
- `!CMD` runs CMD yourself with bash on this terminal, so an editor or a pager works, and what you type while it runs goes to the command. Your next message starts with `[the user ran in their terminal: CMD (exit N)]`. The command gets the terminal modes henki started with, and at the next prompt henki sets its own again, so a change made with `!stty` does not last.
- Ctrl-C while the model answers or a call runs cancels the turn: the calls get SIGINT, then SIGTERM, then SIGKILL, and the text so far stays on the screen. At a question Ctrl-C answers no; during `!CMD` it goes to the command; at the prompt it does not quit, it prints a hint.
- Ctrl-C, like any other stop, closes henki's connection to the model server. While llama-server generates, it frees its slot for the next request within milliseconds; while it still reads a long prompt, it frees it only after the prompt batch it is on, so the next request can wait several seconds (about 7 s for a long prompt on a small CPU-only machine).
- A process a call starts in the background keeps running between turns, and so does a `!CMD &` job. On Linux a cancel and the end of the session stop them all. On macOS henki stops what stays in a call's process group, but it cannot see a `!CMD &` job or a process that started a session of its own: such a process keeps running after henki exits (stop it yourself), and a `!CMD &` job does not hold back `always`.
- `max_steps` (model requests) bounds a turn. `--max-seconds` and `max_seconds` apply to `henki run` only.

### Typing and pasting

At the prompt and at a question henki reads the terminal itself, without the terminal's line mode and echo, and with bracketed paste on. It shows the line as it will be sent, with control and invisible characters escaped as everywhere else.

- Enter sends the line, also right after Esc, as Alt+Enter, and from a terminal that sends CR LF for it. Backspace removes one character (a whole one, also outside ASCII), Ctrl-U the whole line, and Ctrl-D on an empty line is the end of input. Arrow keys, other special keys and other control characters (Tab too) do nothing; Ctrl-C and Ctrl-\\ stay signals. Ctrl-Z does nothing at the prompt or a question; while henki works it suspends henki as usual. There is no history.
- A paste goes into the line whole, newlines included, whatever its size up to 1 MiB; past that the line is cut at 1 MiB, with a note. Pasting an error message or a log is fine: Enter sends it as one message.
- A line that holds a paste is always a message: never a `/` command or a `!CMD`, and at a question never an answer. A paste at a question counts as no, with a note: no call ever runs on pasted text, and the turn goes on as after any other no. A line emptied with Ctrl-U or Backspace holds no paste any more.
- Bracketed paste rests on the terminal marking where a paste ends. Most terminals take the escape character out of pasted text; one that does not lets text that holds the end mark end the paste early, and what follows is read as if typed: a `!` line in it could run on its own. Paste such text only in a terminal that filters it. Ctrl-C in the middle of a long paste ends it too: what the terminal sends after the next prompt appears is typed text.
- A terminal without bracketed paste sends a paste as if it were typed, so henki goes by what is waiting to be read: a line is a command or an answer only when nothing else is waiting when its Enter comes, and an Enter with more input behind it is a newline in the line. A block the terminal sends at once is therefore one message; a single pasted `!` line with its Enter looks exactly like typing it, and runs.
- What you type while henki works (the answer streams, a call runs, a question is being drawn) is dropped when the next prompt or question appears, with a note that counts the bytes; henki drops it only once everything before the question (the answer, the call's display) has been written to the terminal. After Ctrl-C at a question, the first line you type before the turn ends is dropped as well, without a note. So typed-ahead text never answers a question, never runs as a `!CMD` and never becomes a message.
- henki wraps a long line itself before the last column, counting two columns for a character outside ASCII (so such a row may end early). Backspace draws again only the row it changes, so erasing after a long paste stays quick; Ctrl-U draws the line from its first row. If the window changes width while you type a line, its display may be off until the line is sent.
- The terminal is in henki's modes only while henki waits for a line. Once the line is sent or given up (Enter, Ctrl-D, Ctrl-C, TERM, HUP), and so before any call or `!CMD` runs, and when henki ends in any way it can see (also a closed stdout or a crash), the terminal has the modes it had when henki started, and bracketed paste is off. After a SIGKILL, run `reset`.
- With `TERM=dumb` or no TERM (an editor's shell buffer) henki writes no bracketed paste switches and no reset before a question; editing still moves the cursor, and a terminal that does not echo what you type itself (Emacs's shell buffer) shows the line twice.

### Approvals

With `approve = "ask"` (the default), `shell`, `write` and `edit` calls ask first, and so does a `read` outside the project (the nearest directory above the working directory that holds `.git`). The REPL shows the whole call first: every line of the command or the content, never cut, with control and invisible characters escaped and such lines marked `!| `; a `write` over a file that exists says `(replaces an existing file)`; when the display is taller than the screen, its last line says so. Then it asks `run? [y/N]`:

- `y` or `yes` runs the call once. Anything else, an empty line or Ctrl-C answers no, and the model is told `denied by the user`. Ctrl-D ends the turn, and then the REPL quits.
- `always` saves an allow rule for the command, after a second question (`y` or `yes`), and runs it. It is offered (`run? [y/N/always]`) only for a `shell` command that is one line of printable ASCII of at most 200 bytes, and only while no background process of the session runs (on macOS a `!CMD &` job does not count).

An answer is a line you type at the question. What you typed before it appeared is dropped, and a paste is never an answer (see Typing and pasting).

`henki run` never asks. In ask mode the first call that needs approval ends it with exit 3, naming the call (`(needs approval: $ CMD; ...)`); `henki --resume` (the latest session here) or `henki --resume=ID` continues that session in the REPL: tell the model to go on, and it asks before the call runs. `--approve auto` runs every call without asking.

A module can bring a policy binding, a program that decides every call in place of the above (`henki mods swap policy NAME`; `henki mods swap policy std` goes back). It may allow a call, ask, or deny it (the model is told why); henki shows each allow as `$ CMD  (allowed by policy NAME ID: REASON)`. henki still refuses every write to its own files before the binding is asked, asks you about any call the binding allows that reads or names them (a `read` of `config.toml`, a command with `~/.config/henki/` in it), and asks about every call once the binding fails. With a binding, henki reads no allow rules and refuses `approve = "auto"`; `henki --port policy=std` runs one session without it. See CONTRACTS.md `policy/1`.

A module can also bring an exec binding, a program henki puts in front of every shell call, such as a sandbox (`henki mods swap exec NAME`): each call runs as `PROGRAM BASH --noprofile --norc -c COMMAND`, and henki still times out, kills and sweeps it as its own. While it is bound the model gets the `shell` tool only (`read`, `write` and `edit` would run outside it), so `tools` must hold `shell`. Your `!cmd`, module commands, observers and the other bindings do not run through it. `henki --port exec=std` runs one session without it. See CONTRACTS.md `exec/1`.

### Allow rules

An allow rule lets one exact shell command run without asking in one project. The rules are in `allow` next to the config file (`~/.config/henki/allow`), a plain text file you can read, edit and empty:

```
# henki allow rules: ROOT<TAB>COMMAND, one per line; COMMAND then runs without asking in ROOT.
# List and remove them with henki mods allow [rm N] (/allow in henki); or edit this file.
/Users/me/src/app	git status
```

- ROOT is the project's root directory, or `*` for every project. COMMAND must equal the call's command byte for byte: `git status` never matches `git status --short`, `git status && x` or `git status; rm -rf ~`.
- A rule fixes the bytes of a command, not what it does: `git status` runs what the repository's `.git/config` names, and `make` runs the Makefile. That is why a rule holds only in its own project.
- henki adds a rule only after a confirmed `always`; a `*` rule comes only from your editor. The model's `write` and `edit` refuse the file. `always` does not create the config dir: without it (no config file, only `HENKI_MODEL` and `HENKI_BASE_URL`) the call runs once and no rule is saved.
- `henki mods allow [--config FILE]` (also `henki allow`, and `/allow` in the REPL) lists the rules with their numbers; `henki mods allow [--config FILE] rm N` (`/allow rm N`) removes one. N counts every rule line from 1, invalid ones included. `henki mods ARGS` is `henki-mods ARGS`, found next to henki or on your PATH; it reads no config.
- At every start in ask mode a notice counts the rules for this project (`allow: N rules apply to this root ...`), and the REPL lists each of them. In `auto` mode the file is not read.
- henki does not start when the file is a symlink, writable by others, owned by another user, over 64 KiB, or holds an invalid line; `henki mods allow` still lists and removes, and the error names the fix.

### Trust files

The config file, the allow file, `AGENTS.md` in the config dir, the bash henki runs and every extension file are trust files. henki stamps them when it reads them at start, and checks them before every call (again right before it runs, after you answer a question), after it, and before a turn ends. When one changed (a call edited it, a background process, another program), the turn ends with `error changed`: `henki run` exits 2, and the REPL takes no more messages until you restart henki; `henki --resume` continues the session with the files read again. Editing a trust file with `!CMD`, `/mods` or `/allow rm` stops the REPL the same way. henki's own write to the allow file (`always`) does not count.

### Extensions

An extension is a directory that teaches the model a command-line tool: `NAME/doc.md`, whose first line goes into the system prompt (the model reads the rest of it when it needs to, without asking), and an optional `NAME/bin/`, whose executables come first on the PATH of the calls. It may also hold `cmd/`, commands for you (`/NAME`, with an optional `cmd/NAME.md` of data), `on/`, observers (`on/EVENT`: henki runs each once per event of the session, such as `turn_end` or `call_start`, with the event as one JSON line on stdin, and waits up to 2 s for an `on/turn_start.wait` or `on/call_start.wait` before it goes on; nothing an observer does changes the session, see CONTRACTS.md `observe/1`), `port/`, port bindings, and `ctx/`, context sources (`ctx/NAME`: henki runs each once at every start, as a call runs but never through an `exec` binding, all within 2 s, and puts what it prints, at most 16 KiB of text a terminal shows, into the system prompt after `AGENTS.md`; a source that fails, runs long or prints more is skipped with a notice, never cut, see CONTRACTS.md `ext/2`); a module with any of them and no `bin/` is not in the system prompt. Besides those it may hold only `eval/`, `README*`, `LICENSE*`, `CHANGELOG*` and dot files, and every link in it must be relative and stay inside it. Extensions live in `~/.config/henki/ext`, or in the directories the `ext` key lists. An extension command with the name of a command on your PATH loads only when the `shadow` key lists it as `EXT/NAME`; the calls see your own PATH in `HENKI_USER_PATH`. Every entry of `bin/` must be a file you can run, or henki does not start; only a dot file that cannot run (Finder's `.DS_Store`, the `._NAME` files a copy from a Mac leaves) is no command there, and it is a trust file like the rest. `doc.md` and `cmd/*.md` are held to `AGENTS.md`'s rules (at most 16 KiB of UTF-8, no character a terminal does not show, writable only by you), and so must `on/` and `ctx/`, their files and the directories from those up to the module's be, since they run unasked; henki does not check who owns the other extension files or who may write them, so keep them writable only by you.

The official modules are extensions in this repository's `mods/`, off until you add one with `henki-mods add NAME` (from a checkout, `manager/henki-mods`), which shows what it installs and asks first. `halti` teaches the model halti's verbs, puts its card into `AGENTS.md`, and wraps `halti`, `pbpaste` and `security` so the calls cannot reach secret values through them (a seatbelt: a command named by its full path gets past it). On a Mac, where all three are on your PATH, henki starts with it on only once `config.toml` has `shadow = ["halti/halti", "halti/pbpaste", "halti/security"]`, as `profiles/config.mac.toml` does; the install screen prints that line when it is missing. `itse` is a doc only: itse's verbs and exit codes, run through its CLI. `sessions` gives you `/sessions`, `/resume [N|ID]`, `/new` and `/reload`; `prompts` gives you `/review`, `/explain`, `/commit-msg` and `/fix-test`, prepared prompts you can copy and extend; both cost no prompt bytes. `clip` gives the model `clip-get` and `clip-put` for the clipboard. `checkpoints` keeps a snapshot of the work tree before each `write`, `edit` and `shell` call (in git, refs under `refs/henki/checkpoints/` made without touching your index or HEAD) and gives you `/undo`, `/rewind N` and `/checkpoints`; `notify` shows a desktop notification when a turn ends or henki asks before a call; both cost no prompt bytes. `project` puts the project's `AGENTS.md` (or `CLAUDE.md`) at the git root into the system prompt at each start, with the git branch; henki shows its bytes at start.

### henki mods

`henki mods ARGS` runs the module manager, `henki-mods ARGS` (shipped next to henki; `/mods ARGS` in the REPL). It reads no henki config, only the config dir it changes: `~/.config/henki`, or under `/mods` the dir of the config henki runs on. A change shows what it does and asks first (`--yes` once you have read the screen), and inside henki asks to restart on it.

- `list` shows each module's state, version, source and what it adds to every request's system prompt. `add SRC` copies a module (official by `NAME`, from a directory or a git URL) into a read-only store, pinned by its commit and tree hash in `mods.lock`; `on NAME`, `off NAME` and `rm NAME` turn it on, off, or drop it (its store copy stays for a rollback). `verify` holds the store copies, the links and the cards to the lock, `verify --dir DIR` one module directory, and `lock` prints the lock.
- `swap PORT NAME` binds henki's `history`, `policy` or `exec` port to a module's `port/PORT` program, once it passes the port's conformance suite (for `exec`, seven fixed checks) and you said yes; `test --port PORT NAME` runs the suite alone, and `swap PORT std` goes back. `henki --port PORT=std` runs one session on std whatever is bound.
- `eval NAME` runs a module's eval tasks (`eval/TASK/` with `prompt.txt`, `setup`, `check`, and optionally `fake-bin/` and `task.toml`) through `henki run`, once with the module on and once with it off, each arm in a temporary copy of your config dir with no other module: 3 repeats, 13 when the arms end within one pass of each other, or `--repeats N`; `--model M` picks the model. `--suite general` runs the general suite instead, 12 everyday shell tasks, to show that a module costs the model nothing on tasks it is not for. Each arm's passes are printed and added to `~/.local/state/henki/eval.tsv`. The model's calls run without asking (`approve = "auto"`) in temporary directories and a module's `setup` and `check` run as you, so eval shows that and asks first. `eval --check DIR` checks a suite's layout only. See CONTRACTS.md `eval/1`.
- `allow [rm N]` lists henki's allow rules or removes one (see Allow rules).

### Writing a module

A module is an extension with an optional manifest, `.henki/module.toml`, that `henki-mods` shows on its install screen and checks first. The module template, `github.com/deorlovnis/henki-module-template` (to be published), is a working module with a guide from copying it to sharing your own. `henki-mods verify --dir DIR` checks a module as an install would (its `eval/` tasks too), `henki-mods eval ./DIR` measures it against the module off, `henki-mods add ./DIR` installs it from a directory, and `henki-mods add URL[@REF]#DIR` from a git repository, which is how you share one: henki-mods pins the commit and runs nothing from the repository, and its install screen marks such a module unreviewed.

### In an isolated environment

`profiles/config.box.toml` runs `henki run` with `dialect = "anthropic"`: the Anthropic Messages API through the environment's model gateway, with a stand-in key that the gateway replaces. An Anthropic session cannot be resumed after its system prompt (the global `AGENTS.md`, an extension's first doc line) or its tools changed, because the API refuses the replayed history; start a new session instead. When the model declines a request (`stop_reason` `refusal`), the run ends with `error provider`, and the text that streamed before the refusal is not kept in the session.

### Output, sessions and exit codes

- `henki run` prints the answer on stdout and one line per call and notice on stderr; a call line ends `, no output` when the command printed nothing. `--json` prints one JSON record per line instead, ending with a `result` record.
- Every session is journaled under `~/.local/state/henki`; `--resume[=ID]` continues one, and `--resume=ID@N` forks session ID at its line N (a `user` record) into a new session. A REPL that quits before its first message leaves no session.
- When a session ends, every process it started is stopped (on macOS, except a `!CMD &` job or a process that started a session of its own).
- Exit codes: 0 answered (or the REPL quit), 1 gave up (step limit, stuck, empty answer), 2 error, 3 needs approval, 124 out of time, 130 interrupted (or TERM, HUP or the terminal closing), 141 stdout closed or not read in time.
- The journal, the `--json` records, what henki sends to a model server and the extension layout are specified, with their versions and the stability rule, in [CONTRACTS.md](CONTRACTS.md).

## Build and check

Rust 1.92.0 (pinned in `rust-toolchain.toml`) and cargo.

```sh
cargo build --release
./check.sh               # formatting, lints, tests and the size budgets
./check.sh install-hook  # check the commits of every git push
```

`check.sh` holds henki to its budgets: at most 4,446 lines of non-test Rust (400 per file), at most 10 crates in the dependency tree, and a release binary of at most 1,065,536 bytes, including the static musl build on Linux when that target is installed. It also refuses a commit not yet pushed that has no `Signed-off-by:` line (`git commit -s`); [CONTRIBUTING.md](CONTRIBUTING.md) says why and lists the style rules.

`release/release.sh VERSION` builds, from a clean checkout, `dist/henki-VERSION-TARGET.tar.gz` for this host (static musl on Linux) and `dist/SHA256SUMS`. A tarball holds `bin/henki`, `bin/henki-mods`, the official modules in `share/henki/official` (off until added), the conformance suites in `share/henki/conform`, the general eval suite in `share/henki/eval/general`, the config profiles, LICENSE, README.md and CONTRACTS.md; unpack it anywhere and put its `bin/` on your PATH. `release/release.sh --formula VERSION TAP` writes the Homebrew formula from `SHA256SUMS`, and `release/release.sh --selftest` builds, unpacks and runs one tarball.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or <https://www.apache.org/licenses/LICENSE-2.0>).

Contributions are accepted under the same license, signed off under the Developer Certificate of Origin; see [CONTRIBUTING.md](CONTRIBUTING.md).
