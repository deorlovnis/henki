# henki

henki is a small terminal agent in early development. Nothing usable yet: `henki --version` and `henki --help` work, and anything else exits with code 2.

## Plans

henki is being built to work in two places:

- On your machine, a line REPL that talks to a local model server and acts through your shell and a few file tools. It will ask before it runs a command unless you have allowed that command.
- In an isolated Linux environment, a headless runner: `henki run TASK`, one static binary, no install step.

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
