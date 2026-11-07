# henki

henki is a small terminal agent in early development. Today it has one command, `henki run`: it gives a task to a model behind an OpenAI-compatible server (plain HTTP, usually on your machine) and lets the model act through four tools (`shell`, `read`, `write`, `edit`) until it answers.

## Usage

```sh
mkdir -p ~/.config/henki && cp profiles/config.mac.toml ~/.config/henki/config.toml   # then set model and base_url
henki run "why does the build fail?"
henki run --json --approve auto --max-seconds 600 "fix the failing test"
echo "a long task" | henki run -
```

- Plain mode prints the answer on stdout and one line per call and notice on stderr. `--json` prints one JSON record per line instead, ending with a `result` record.
- With `approve = "ask"` (the default), a call that needs a human (`shell`, `write`, `edit`, or a `read` outside the project) ends the run with exit 3 and the call it wanted; `--approve auto` runs every call.
- Every session is journaled under `~/.local/state/henki`; `--resume[=ID]` continues one.
- `--max-steps` bounds the model requests, `--max-seconds` the wall time. When a run ends, every process it started is stopped.
- Exit codes: 0 answered, 1 gave up (step limit, stuck, empty answer), 2 error, 3 needs approval, 124 out of time, 130 interrupted, 141 stdout closed or not read in time (5 s after the end, or the `--max-seconds` budget).

## Plans

henki is being built to work in two places:

- On your machine, a line REPL that talks to a local model server and acts through your shell and a few file tools. It will ask before it runs a command unless you have allowed that command.
- In an isolated Linux environment, the headless runner above: one static binary, no install step.

Extensions will be plain directories that teach the model a CLI: a `doc.md` and, optionally, a `bin/` of commands.

## Build and check

Rust 1.92.0 (pinned in `rust-toolchain.toml`) and cargo.

```sh
cargo build --release
./check.sh               # formatting, lints, tests and the size budgets
./check.sh install-hook  # check the commits of every git push
```

`check.sh` holds henki to its budgets: at most 2,600 lines of non-test Rust (400 per file), at most 10 crates in the dependency tree, and a release binary of at most 1,000,000 bytes, including the static musl build on Linux when that target is installed.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/license/mit>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
