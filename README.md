# henki

henki is a small terminal agent in early development. It gives your task to a model and lets the model act through four tools (`shell`, `read`, `write`, `edit`) until it answers. On your machine it is a line REPL that talks to a local model server (plain HTTP, OpenAI-compatible) and asks before a command runs; in an isolated Linux environment it runs headless, one static binary with no install step.

## Usage

```sh
mkdir -p ~/.config/henki && cp profiles/config.mac.toml ~/.config/henki/config.toml   # then set model and base_url
henki                                   # the REPL
henki run "why does the build fail?"    # one task, headless
henki run --json --approve auto --max-seconds 600 "fix the failing test"
echo "a long task" | henki run -
```

### The REPL

`henki` starts a line REPL on the terminal it was started from (without a terminal on stdin it refuses: use `henki run`). Its banner names the model, the server and its dialect, the approval mode, the config file, the extensions, any notices, and every allow rule for this project. Each line you type at the `> ` prompt is one message. The answer streams to stdout; the banner, the prompt, questions, call lines and notices go to stderr, so `henki > answers.txt` keeps only the answers.

- One line is one message, and henki takes one line at each prompt: the rest of a pasted block, or of what you typed ahead, is dropped. So a pasted line that starts with `!` runs only when it is the first line of the paste. For longer text, such as a log, save it to a file and name the file in your message, or use `henki run -`.
- `/exit` or Ctrl-D quits. `/help` lists the commands, `/allow` lists the allow rules and `/allow rm N` removes rule N. Any other line that starts with `/` is an unknown command and never reaches the model.
- `!CMD` runs CMD yourself with bash on this terminal, so an editor or a pager works, and what you type (also right after the `!CMD` line) goes to the command. Your next message starts with `[the user ran in their terminal: CMD (exit N)]`. Afterwards henki restores the terminal modes it started with, so a change made with `!stty` does not last.
- Ctrl-C while the model answers or a call runs cancels the turn: the calls get SIGINT, then SIGTERM, then SIGKILL, and the text so far stays on the screen. At a question Ctrl-C answers no; during `!CMD` it goes to the command; at the prompt it does not quit, it prints a hint.
- A process a call starts in the background keeps running between turns, and so does a `!CMD &` job. On Linux a cancel and the end of the session stop them all. On macOS henki stops what stays in a call's process group, but it cannot see a `!CMD &` job or a process that started a session of its own: such a process keeps running after henki exits (stop it yourself), and a `!CMD &` job does not hold back `always`.
- `max_steps` (model requests) bounds a turn. `--max-seconds` and `max_seconds` apply to `henki run` only.
- There is no line editor and no key binding beyond Ctrl-C and Ctrl-D; `rlwrap henki` adds editing and history.

### Approvals

With `approve = "ask"` (the default), `shell`, `write` and `edit` calls ask first, and so does a `read` outside the project (the nearest directory above the working directory that holds `.git`). The REPL shows the whole call first: every line of the command or the content, never cut, with control and invisible characters escaped and such lines marked `!| `; when the display is taller than the screen, its last line says so. Then it asks `run? [y/N]`:

- `y` or `yes` runs the call once. Anything else, an empty line or Ctrl-C answers no, and the model is told `denied by the user`.
- `always` saves an allow rule for the command, after a second question, and runs it. It is offered (`run? [y/N/always]`) only for a `shell` command that is one line of printable ASCII of at most 200 bytes, and only while no background process of the session runs (on macOS a `!CMD &` job does not count).

What you type while henki works never answers a question: input typed ahead is dropped before each one. After Ctrl-C at a question, henki is still reading the line it asked for, so the first line you type before the turn ends is dropped too, rather than kept as your next message.

`henki run` never asks. In ask mode the first call that needs approval ends it with exit 3, naming the call; `henki --resume=ID` continues that session in the REPL: tell the model to go on, and it asks before the call runs. `--approve auto` runs every call without asking.

### Allow rules

An allow rule lets one exact shell command run without asking in one project. The rules are in `allow` next to the config file (`~/.config/henki/allow`), a plain text file you can read, edit and empty:

```
# henki allow rules: ROOT<TAB>COMMAND, one per line; COMMAND then runs without asking in ROOT.
# List and remove them with /allow in henki or henki allow [rm N]; or edit this file.
/Users/me/src/app	git status
```

- ROOT is the project's root directory, or `*` for every project. COMMAND must equal the call's command byte for byte: `git status` never matches `git status --short`, `git status && x` or `git status; rm -rf ~`.
- A rule fixes the bytes of a command, not what it does: `git status` runs what the repository's `.git/config` names, and `make` runs the Makefile. That is why a rule holds only in its own project.
- henki adds a rule only after a confirmed `always`; a `*` rule comes only from your editor. The model's `write` and `edit` refuse the file.
- `/allow` and `henki allow [--config FILE]` list the rules with their numbers; `/allow rm N` and `henki allow [--config FILE] rm N` remove one. N counts every rule line from 1, invalid ones included.
- At every start in ask mode a notice counts the rules for this project (`allow: N rules apply to this root ...`), and the REPL lists each of them. In `auto` mode the file is not read.
- henki does not start when the file is a symlink, writable by others, owned by another user, over 64 KiB, or holds an invalid line; `henki allow` still lists and removes, and the error names the fix.

### Trust files

The config file, the allow file, `AGENTS.md` in the config dir, the bash henki runs and every extension file are trust files. henki stamps them when it reads them at start, and checks them before every call (again right before it runs, after you answer a question), after it, and before a turn ends. When one changed (a call edited it, a background process, another program), the turn ends with `error changed`: `henki run` exits 2, and the REPL takes no more messages until you restart henki; `henki --resume` continues the session with the files read again. Editing a trust file with `!CMD` stops the REPL the same way. henki's own writes to the allow file (`always`, `/allow rm`) do not count.

### Extensions

An extension is a directory that teaches the model a command-line tool: `NAME/doc.md`, whose first line goes into the system prompt (the model reads the rest of it when it needs to, without asking), and an optional `NAME/bin/`, whose executables come first on the PATH of the calls. Besides those it may hold only `README*`, `LICENSE*`, `CHANGELOG*` and dot files. Extensions live in `~/.config/henki/ext`, or in the directories the `ext` key lists. An extension command with the name of a command on your PATH loads only when the `shadow` key lists it as `EXT/NAME`; the calls see your own PATH in `HENKI_USER_PATH`.

### In an isolated environment

`profiles/config.box.toml` runs `henki run` with `dialect = "anthropic"`: the Anthropic Messages API through the environment's model gateway, with a stand-in key that the gateway replaces. An Anthropic session cannot be resumed after its system prompt (the global `AGENTS.md`, an extension's first doc line) or its tools changed, because the API refuses the replayed history; start a new session instead.

### Output, sessions and exit codes

- `henki run` prints the answer on stdout and one line per call and notice on stderr. `--json` prints one JSON record per line instead, ending with a `result` record.
- Every session is journaled under `~/.local/state/henki`; `--resume[=ID]` continues one. A REPL that quits before its first message leaves no session.
- When a session ends, every process it started is stopped (on macOS, except a `!CMD &` job or a process that started a session of its own).
- Exit codes: 0 answered (or the REPL quit), 1 gave up (step limit, stuck, empty answer), 2 error, 3 needs approval, 124 out of time, 130 interrupted (or TERM, HUP or the terminal closing), 141 stdout closed or not read in time.

## Build and check

Rust 1.92.0 (pinned in `rust-toolchain.toml`) and cargo.

```sh
cargo build --release
./check.sh               # formatting, lints, tests and the size budgets
./check.sh install-hook  # check the commits of every git push
```

`check.sh` holds henki to its budgets: at most 3,500 lines of non-test Rust (400 per file), at most 10 crates in the dependency tree, and a release binary of at most 1,000,000 bytes, including the static musl build on Linux when that target is installed.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/license/mit>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
