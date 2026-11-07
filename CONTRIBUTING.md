# Contributing to henki

henki is small on purpose: every change is held to budgets that `./check.sh` enforces, and a change that does not pass it is not merged.

## Build and check

Rust 1.92.0 (pinned in `rust-toolchain.toml`, so rustup picks it) and cargo.

```sh
cargo build --release   # target/release/henki
cargo test              # the tests alone
./check.sh              # everything a change must pass, stopping at the first failure
```

`./check.sh` runs, in order:

1. **refs**: no private references in the tree. It reads patterns from `.git/info/private-patterns`, which is never committed; without that file it prints a `SKIPPED` warning and goes on.
2. **dco**: every commit that no remote has yet carries a `Signed-off-by:` trailer (below). `./check.sh dco [RANGE]` runs it alone.
3. **selftest**: every gate runs against fixtures it makes and must fail the bad ones with its own message. `./check.sh selftest` runs it alone.
4. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.
5. `cargo test`.
6. **budgets**: at most 3,600 lines of non-test Rust in `src/` (400 per file; comments, blank lines and `#[cfg(test)]` code do not count), at most 10 crates in the normal dependency tree (no proc-macro crate, no C build dependency), and a release binary of at most 1,000,000 bytes, on Linux also the static musl build (`rustup target add $(uname -m)-unknown-linux-musl`).
7. **release gates**: the tests marked `#[ignore = "release gate"]` (timing and memory limits), built with the release profile.

It needs bash, git and POSIX tools, on macOS or Linux. `CARGO_BUILD_JOBS` (default 2) and `CARGO_TARGET_DIR` work as in cargo. `./check.sh install-hook` makes every `git push` run it first.

## Sign off your commits (DCO)

henki is licensed under Apache-2.0 ([LICENSE](LICENSE)), and contributions come in under the same license. Instead of a contributor agreement, every commit carries a sign-off: a `Signed-off-by: Your Name <you@example.com>` line at the end of its message, with your real name and the email of the commit. It says you certify the Developer Certificate of Origin 1.1 below for that commit.

```sh
git commit -s                          # adds the line from user.name and user.email
git commit --amend -s --no-edit        # sign off the last commit
git rebase --signoff origin/main       # sign off every commit since origin/main
git merge --signoff BRANCH             # merges are commits too
```

The `dco` step of `./check.sh` refuses a commit without it. Only a trailer counts: the line must be in the last paragraph of the message, where `git commit -s` puts it.

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

## Style

- **Commits**: an imperative subject (`Refuse a symlinked allow file`), a short body that says why, signed off.
- **Messages to the user**: `PROBLEM; fix: WHAT TO DO`, naming the file or value at fault (`~/.config/henki/allow: a symlink; fix: put the file itself in its place`).
- **Comments** say why, not what; keep them dense and true when the code changes.
- **Lints** (`Cargo.toml`, `clippy.toml`) are the rules, not suggestions: product code never panics on purpose (no `unwrap`, `expect`, `panic!` or byte slicing of a `str`), only `main` ends the process, only `term` writes to stdout and stderr, files are created and read through `sys`, and `unsafe` lives in `src/sys.rs` alone with a `// SAFETY:` note on every block. Do not silence a lint with `allow` to get a change through.
- **Tests** go in `src/MODULE/tests.rs` (declared `#[cfg(test)] mod tests;`) or `tests/`; they do not count against the line budget, so write them freely.
- **Dependencies**: a new crate needs a reason that beats writing the code; the budget is 10 crates, henki included, and the tree is close to it.
- **Safety floors** (the trust-file checks, the allow rules, the limits) are never weakened to make a feature fit.
