#!/usr/bin/env bash
# Everything a change must pass that needs no human, in order, stopping at the first failure:
# no private references, a sign-off on every commit not yet pushed, the gates' self-test, formatting, lints, the
# tests, the module manager's tests and the official modules, the size budgets (lines of code, crates in the
# dependency tree, release binary bytes), then the release gates: the tests marked #[ignore = "release gate"]
# (timing and memory limits), built and run with the release profile.
# Run it before pushing. Bash on macOS and Linux.
#
# Each budget gate is a function of its input (gate_lines DIR, gate_crates TREE LOCK, gate_bytes
# FILE, gate_mods DIR, and refs and dco). `./check.sh selftest` runs every one against fixtures it makes: an
# over-budget fixture must fail with the gate's own message, a within-budget one must pass. The
# full check runs it right after the reference and sign-off checks, so a gate that stops failing
# fails the check.
#
# Environment:
#   CARGO_BUILD_JOBS  parallel jobs (default 2; release builds with LTO need the memory)
#   CARGO_TARGET_DIR  build output (default target/)
#   RUSTUP_HOME       a rustup home whose 1.92.0 toolchain has the static musl target; on Linux the
#                     check fails without it (rustup target add <arch>-unknown-linux-musl)
#   CHECK_FROM_HOOK   set by the pre-push hook: a missing pattern file fails instead of skipping
#
# `./check.sh refs [RANGE]` runs only the first step against the private references listed in
# .git/info/private-patterns: one extended regex per line, blank lines and # comments ignored.
# That file is never committed. Without it (or with no pattern in it) a run by hand prints a loud
# SKIPPED line and goes on, and a run from the pre-push hook fails. With no RANGE it searches
# the working tree (tracked and untracked files, minus ignored ones). With a RANGE (`A..B`,
# `HEAD`, or `B --not --remotes`) it searches every commit message in it and every line those
# commits add, so a reference added and later removed is still found. In a range a matched line
# is shown as "COMMIT: +LINE", so a pattern anchored with ^ only matches in files.
# `./check.sh dco [RANGE]` runs only the sign-off check: every commit in RANGE (default
# `HEAD --not --remotes`, the commits no remote has yet) must carry a `Signed-off-by:` trailer with its author's email, the
# Developer Certificate of Origin sign-off of CONTRIBUTING.md. A range with no commits passes.
# `./check.sh install-hook` makes every git push run the full check, then the reference and
# sign-off checks on the commits it pushes; it refuses to install without the pattern file.
set -euo pipefail
cd "$(dirname "$0")"
# A git hook runs with GIT_DIR set to the repository being pushed (`git push` from a linked work tree), and `git -c`
# config too: every git run under the check would use that repository, a test's `git init` in its scratch dir
# included. So none of git's repository variables reach the check: each git finds its repository from its own
# directory, and this one finds this checkout's.
# shellcheck disable=SC2046 # one variable name per word.
unset $(git rev-parse --local-env-vars 2>/dev/null)

max_lines=4544      # non-test lines in src/
max_file_lines=400  # per file; a module is one file
max_crates=10       # packages in the normal dependency tree, henki included
max_bytes=1065536   # release binary

step() { printf '\n== %s\n' "$*"; }
say() { echo "check.sh: $*" >&2; }
fail() {
  say "$*"
  exit 1
}

refs() {
  local file args=() p found=0 rc log
  file="$(git rev-parse --git-path info/private-patterns)"
  if [ -f "$file" ]; then
    while IFS= read -r p || [ -n "$p" ]; do
      case "$p" in '' | '#'*) ;; *) args+=(-e "$p") ;; esac
    done <"$file"
  fi
  if [ "${#args[@]}" = 0 ]; then
    if [ -n "${CHECK_FROM_HOOK:-}" ]; then
      echo "check.sh: no patterns in $file; the pre-push hook does not pass without them" >&2
      return 1
    fi
    echo "check.sh: WARNING: no patterns in $file, so the private reference check was SKIPPED" >&2
    return 0
  fi
  # grep exits 1 for no match; anything above that is an error and fails the check.
  rc=0
  if [ -z "${1:-}" ]; then
    git grep --untracked -nIE "${args[@]}" || rc=$?
  else
    # Messages, then each added line, under the short hash of its commit. %x01 marks a commit.
    # shellcheck disable=SC2086 # RANGE is several git arguments on purpose.
    log="$(git log --no-color --no-ext-diff --no-textconv --no-renames -p --format='%x01%h%n%B' $1)" || return $?
    LC_ALL=C awk '/^\001/ { c = substr($0, 2); msg = 1; next } /^diff --git / { msg = 0 } msg || /^\+/ { print c ": " $0 }' <<<"$log" |
      grep -E "${args[@]}" || rc=$?
  fi
  case "$rc" in 0) found=1 ;; 1) ;; *) return "$rc" ;; esac
  if [ "$found" = 1 ]; then
    echo "check.sh: the lines above match $file" >&2
    return 1
  fi
}

# The sign-off gate over RANGE (git log arguments, default the commits on no remote). Git's own trailer parser
# decides, so only a trailer counts (the message's last paragraph, where `git commit -s` puts it), not the words
# anywhere in a body. The trailer must carry the commit author's email (CONTRIBUTING.md: "the email of the commit"),
# as DCO checkers do: a sign-off is the author's own, not anyone's. Merges too: a merge brings no code of its own,
# but the DCO is per commit, and `git merge --signoff` costs nothing. Commits a remote already has are history.
dco() {
  local log bad
  # %x01 marks a commit, then the author's email and the subject; the trailer lines, if any, follow it.
  # shellcheck disable=SC2086 # RANGE is several git arguments on purpose.
  log="$(git log --no-color --format='%x01%h%x02%ae%x02%s%n%(trailers:key=Signed-off-by)' ${1:-HEAD --not --remotes})" || return $?
  bad="$(LC_ALL=C awk -F '\002' '/^\001/ { if (c != "" && !s) print c; c = substr($1, 2) " " $3; ae = "<" tolower($2) ">"; s = 0; next }
    tolower($0) ~ /^signed-off-by: *[^ ]/ && index(tolower($0), ae) { s = 1 }
    END { if (c != "" && !s) print c }' <<<"$log")"
  if [ -n "$bad" ]; then
    printf '%s\n' "$bad"
    say "the commits above have no Signed-off-by: line with their author's email; fix: git rebase --signoff BASE (or git commit --amend -s --no-edit for the last one) as the author, see CONTRIBUTING.md"
    return 1
  fi
}

# Non-test lines of Rust, one "LINES FILE" row per file. Comments and blank lines don't count,
# nor anything under #[cfg(test)] (an inline module, or the file of a `mod x;` declared under it);
# string contents count as code. Tests in tests/ are not in src/ and so not counted. Byte-wise
# (LC_ALL=C) and POSIX awk only, so macOS and Linux count alike.
rust_lines() {
  # shellcheck disable=SC2016 # awk's own $ fields.
  LC_ALL=C awk '
    function flush() { if (file != "") print "C", n, file }
    FNR == 1 {
      flush(); file = FILENAME; n = 0; bc = 0; str = 0; raw = -1; skip = 0; pend = ""
      dir = file; sub(/\/[^\/]*$/, "", dir); stem = file; sub(/^.*\//, "", stem); sub(/\.rs$/, "", stem)
      sub_dir = (stem == "main" || stem == "lib" || stem == "mod") ? dir : dir "/" stem
    }
    {
      # 1. Strip comments; replace string contents with x (raw, byte and char literals too).
      line = $0; code = ""; i = 1; len = length(line)
      while (i <= len) {
        c = substr(line, i, 1); c2 = substr(line, i, 2)
        if (bc > 0) { if (c2 == "/*") { bc++; i += 2 } else if (c2 == "*/") { bc--; i += 2 } else i++; continue }
        if (raw >= 0) {
          if (c == "\"" && substr(line, i + 1, raw) == substr(hashes, 1, raw)) { code = code c; i += 1 + raw; raw = -1 } else { code = code "x"; i++ }
          continue
        }
        if (str) {
          if (c == "\\") { code = code "x"; i += 2; continue }
          if (c == "\"") { str = 0; code = code c } else code = code "x"
          i++; continue
        }
        if (c2 == "//") break
        if (c2 == "/*") { bc = 1; i += 2; continue }
        if (c == "\"") { str = 1; code = code c; i++; continue }
        prev = i > 1 ? substr(line, i - 1, 1) : ""
        if ((c == "r" || c == "b") && prev !~ /[A-Za-z0-9_]/ && match(substr(line, i), /^b?r#*"/)) {
          h = substr(line, i, RLENGTH); gsub(/[^#]/, "", h); raw = length(h); hashes = "################################"
          code = code "\""; i += RLENGTH; continue
        }
        if (c == q) {  # a char literal (not a lifetime): its contents could be a quote
          if (substr(line, i + 1, 1) == "\\") { e = index(substr(line, i + 2), q); if (e) { code = code q "x" q; i += 2 + e; continue } }
          else if (substr(line, i + 2, 1) == q) { code = code q "x" q; i += 3; continue }
        }
        code = code c; i++
      }
      # 2. Drop #[cfg(test)] items: the attribute, then up to the matching } or the ;.
      counted = 0; j = 1; len = length(code)
      while (j <= len) {
        c = substr(code, j, 1)
        if (skip == 0) {
          if (c == "#" && match(substr(code, j), /^#[ \t]*\[[ \t]*cfg[ \t]*\([ \t]*(all[ \t]*\([ \t]*)?test/) && substr(code, j + RLENGTH, 1) !~ /[A-Za-z0-9_]/) {
            k = index(substr(code, j), "]"); skip = 1; depth = 0; pend = ""; j += k ? k : len; continue
          }
          if (c !~ /[ \t]/) counted = 1
        } else if (skip == 1) {
          pend = pend c
          if (c == "[") depth++
          else if (c == "]") depth--
          else if (c == "{" && depth == 0) { skip = 2; depth = 1 }
          else if (c == ";" && depth == 0) {
            if (match(pend, /(^|[^A-Za-z0-9_])mod[ \t]+[A-Za-z_][A-Za-z0-9_]*[ \t]*;$/)) {
              m = substr(pend, RSTART, RLENGTH); sub(/^.*mod[ \t]+/, "", m); sub(/[ \t]*;$/, "", m)
              print "X", sub_dir "/" m ".rs"; print "X", sub_dir "/" m "/mod.rs"
            }
            skip = 0
          }
        } else if (c == "{") depth++
        else if (c == "}" && --depth == 0) skip = 0
        j++
      }
      if (skip == 1) pend = pend " "
      if (counted) n++
    }
    END { flush() }
  ' q="'" "$@" | LC_ALL=C awk '$1 == "X" { x[$2] = 1 } $1 == "C" { n[++k] = $2; f[k] = $3 } END { for (i = 1; i <= k; i++) if (!(f[i] in x)) print n[i], f[i] }'
}

# The lines gate over DIR: every DIR/**/*.rs counted by rust_lines, the total within max_lines and each file within
# max_file_lines.
gate_lines() {
  local files=() f counts total big
  while IFS= read -r f; do files+=("$f"); done < <(find "$1" -type f -name '*.rs' | LC_ALL=C sort)
  if [ "${#files[@]}" = 0 ]; then
    say "no Rust files in $1/"
    return 1
  fi
  counts="$(rust_lines "${files[@]}")"
  printf '%s\n' "$counts"
  total="$(awk '{ t += $1 } END { print t + 0 }' <<<"$counts")"
  echo "total $total (gate $max_lines; any file $max_file_lines)"
  if [ "$total" -gt "$max_lines" ]; then
    say "$total lines of non-test Rust, over $max_lines"
    return 1
  fi
  big="$(awk -v max="$max_file_lines" '$1 > max' <<<"$counts")"
  if [ -n "$big" ]; then
    say "files over $max_file_lines lines: $big"
    return 1
  fi
}

# The crates gate over TREE (cargo tree's packages, one per line) and LOCK (the text of Cargo.lock): at most
# max_crates packages, no proc-macro crate, no C build dependency.
gate_crates() {
  local n
  n="$(grep -c . <<<"$1")" || true
  echo "$n packages (gate $max_crates)"
  if [ "$n" -gt "$max_crates" ]; then
    say "$n packages in the normal dependency tree, over $max_crates"
    return 1
  fi
  if grep -F '(proc-macro)' <<<"$1"; then
    say "a proc-macro crate is in the normal dependency tree"
    return 1
  fi
  if grep -E '^name = "(cc|cmake|pkg-config)"$' <<<"$2"; then
    say "a C build dependency is in Cargo.lock"
    return 1
  fi
}

# The size gate: FILE holds at most max_bytes.
gate_bytes() {
  local bytes
  bytes="$(wc -c <"$1")" || return 1
  bytes=$((bytes))
  echo "$1: $bytes bytes (gate $max_bytes)"
  if [ "$bytes" -gt "$max_bytes" ]; then
    say "$1 is $bytes bytes, over $max_bytes"
    return 1
  fi
}

# expect WHAT WANT GATE ARGS...: runs one gate on a fixture and holds it to WANT, either `pass` or the text its failure
# message must contain.
expect() {
  local what="$1" want="$2" err rc=0 wanted="a pass"
  shift 2
  err="$("$@" 2>&1 >/dev/null)" || rc=$?
  if [ "$want" = pass ] && [ "$rc" = 0 ]; then return 0; fi
  if [ "$want" != pass ] && [ "$rc" != 0 ] && [[ "$err" == *"$want"* ]]; then return 0; fi
  [ "$want" = pass ] || wanted="a failure saying \"$want\""
  say "selftest: $what: expected $wanted, got exit $rc${err:+: $err}"
  return 1
}

# N counted lines of Rust into FILE.
code() {
  awk -v n="$2" 'BEGIN { for (i = 0; i < n; i++) print "const C: u8 = 0;" }' >"$1"
}

# N counted lines of Rust into DIR, spread over files of at most max_file_lines.
spread() {
  local n="$2" i=0
  while [ "$n" -gt 0 ]; do
    code "$1/f$i.rs" $((n < max_file_lines ? n : max_file_lines))
    n=$((n - max_file_lines)) i=$((i + 1))
  done
}

# An inline #[cfg(test)] item (nested braces, a brace in a string) put in the middle of FILE: the counter must skip
# its lines and count every line after it.
mid_test() {
  local n
  n="$(wc -l <"$1")"
  {
    head -n $((n / 2)) "$1"
    printf '#[cfg(test)]\nfn t() {\n    let s = "}";\n    if s.is_empty() {}\n}\n'
    tail -n +$((n / 2 + 1)) "$1"
  } >"$1.tmp" && mv "$1.tmp" "$1"
}

# N package lines, as cargo tree prints them.
packages() {
  awk -v n="$1" 'BEGIN { for (i = 1; i <= n; i++) print "p" i " v1.0.0" }'
}

# The mods gate over DIR: every DIR/*/ (the official modules) passes `henki-mods verify --dir`, which checks the
# layout henki loads and the manifest. No DIR, or an empty one, is no module and passes.
gate_mods() {
  local m n=0
  for m in "$1"/*/; do
    [ -d "$m" ] || continue
    if ! manager/henki-mods verify --dir "${m%/}"; then
      say "mods: ${m%/} does not pass henki-mods verify --dir"
      return 1
    fi
    n=$((n + 1))
  done
  echo "$n official modules"
}

# Every budget gate against fixtures made in a scratch directory, removed at the end.
selftest() {
  local t bad=0 clean
  t="$(mktemp -d)"
  scratch="$t"
  trap 'rm -rf "$scratch"' EXIT
  mkdir -p "$t/over" "$t/full" "$t/big" "$t/ok/x"
  spread "$t/over" $((max_lines + 1))
  spread "$t/full" "$max_lines"
  code "$t/big/f.rs" $((max_file_lines + 1))
  code "$t/ok/x.rs" "$max_file_lines"
  printf '// a comment\n\n  /* a block\n     comment */\n#[cfg(test)]\nmod tests;\n' >>"$t/ok/x.rs"
  code "$t/ok/x/tests.rs" 500
  # Lines after an inline test item still count (else `over` would pass), and its own lines do not (else `ok` would fail).
  mid_test "$t/over/f0.rs"
  mid_test "$t/ok/x.rs"
  expect "lines: $((max_lines + 1)) in files of at most $max_file_lines" "over $max_lines" gate_lines "$t/over" || bad=1
  expect "lines: exactly $max_lines in files of at most $max_file_lines" pass gate_lines "$t/full" || bad=1
  expect "lines: a file of $((max_file_lines + 1))" "files over $max_file_lines" gate_lines "$t/big" || bad=1
  expect "lines: $max_file_lines, comments, blank lines, an inline test item and a test module" pass gate_lines "$t/ok" || bad=1

  clean=$'[[package]]\nname = "henki"\nversion = "0.1.0"'
  expect "crates: $((max_crates + 1)) packages" "over $max_crates" gate_crates "$(packages $((max_crates + 1)))" "$clean" || bad=1
  expect "crates: a proc-macro crate" "proc-macro" gate_crates "$(packages 2)"$'\nderive v1.0.0 (proc-macro)' "$clean" || bad=1
  expect "crates: cc in the lock file" "C build dependency" gate_crates "$(packages 2)" $'[[package]]\nname = "cc"\nversion = "1.2.0"' || bad=1
  expect "crates: $max_crates packages" pass gate_crates "$(packages "$max_crates")" "$clean" || bad=1

  head -c $((max_bytes + 1)) /dev/zero >"$t/big.bin"
  head -c "$max_bytes" /dev/zero >"$t/ok.bin"
  expect "bytes: $((max_bytes + 1))" "over $max_bytes" gate_bytes "$t/big.bin" || bad=1
  expect "bytes: $max_bytes" pass gate_bytes "$t/ok.bin" || bad=1

  # A module that henki loads passes; one with an entry henki refuses fails, and so does a dir holding both.
  mkdir -p "$t/mods/good/bin" "$t/mods-bad/good" "$t/mods-bad/bad/lib" "$t/mods-none"
  printf 'good: a fixture module\n' >"$t/mods/good/doc.md"
  printf '#!/bin/sh\n' >"$t/mods/good/bin/good" && chmod 755 "$t/mods/good/bin/good"
  printf 'good: a fixture module\n' >"$t/mods-bad/good/doc.md"
  printf 'bad: a fixture module\n' >"$t/mods-bad/bad/doc.md"
  expect "mods: a module henki loads" pass gate_mods "$t/mods" || bad=1
  expect "mods: an unknown entry lib/" "does not pass henki-mods verify --dir" gate_mods "$t/mods-bad" || bad=1
  expect "mods: no modules" pass gate_mods "$t/mods-none" || bad=1

  # A scratch repository with its own pattern file (no GIT_DIR from a hook leads back here: see the top).
  # Commits there use a fixed identity and no signing or hooks, whatever the user's git config says.
  (
    commit() { git -c user.name=fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q --no-verify "$@"; }
    cd "$t" && git init -q repo && cd repo && mkdir -p .git/info || exit 1
    echo 'fixture-[0-9]+-marker' >.git/info/private-patterns
    echo 'a fixture-7-marker here' >a.txt && git add a.txt || exit 1
    rc=0
    expect "refs: a tracked file matches" "the lines above match" refs || rc=1
    echo 'nothing to see' >a.txt
    expect "refs: no match" pass refs || rc=1
    # A range, as the pre-push hook checks one: commit messages and every added line, even one removed later.
    git add a.txt && commit -m 'add a fixture-7-marker file' || exit 1
    expect "refs HEAD: a commit message matches" "the lines above match" refs HEAD || rc=1
    echo 'a fixture-8-marker line' >b.txt && git add b.txt && commit -m 'add b' || exit 1
    git rm -q b.txt && commit -m 'remove b' || exit 1
    expect "refs HEAD~2..HEAD: a line added, then removed" "the lines above match" refs HEAD~2..HEAD || rc=1
    expect "refs: the working tree after that" pass refs || rc=1
    echo 'still nothing' >c.txt && git add c.txt && commit -m 'add c' || exit 1
    expect "refs HEAD~1..HEAD: a clean commit" pass refs HEAD~1..HEAD || rc=1
    rm .git/info/private-patterns
    CHECK_FROM_HOOK=1 expect "refs: no pattern file, from the hook" "does not pass without them" refs || rc=1

    # Sign-offs, in a second repository whose "remote" is a remote-tracking ref set by hand.
    cd "$t" && git init -q dco && cd dco || exit 1
    commit --allow-empty -m 'pushed, unsigned' && git update-ref refs/remotes/origin/main HEAD || exit 1
    expect "dco: every commit on a remote" pass dco || rc=1
    commit --allow-empty -s -m 'signed' || exit 1
    expect "dco: a signed commit" pass dco || rc=1
    commit --allow-empty -m 'unsigned' || exit 1
    expect "dco: an unsigned commit" "no Signed-off-by: line" dco || rc=1
    expect "dco HEAD~1..HEAD: only the unsigned commit" "no Signed-off-by: line" dco HEAD~1..HEAD || rc=1
    expect "dco HEAD~2..HEAD~1: only the signed commit" pass dco HEAD~2..HEAD~1 || rc=1
    commit --allow-empty -m $'mid-body\n\nSigned-off-by: a b <a@b.invalid>\n\nis not a trailer' || exit 1
    expect "dco: the words in a body, not a trailer" "no Signed-off-by: line" dco HEAD~1..HEAD || rc=1
    commit --allow-empty -m $'signed by another\n\nSigned-off-by: Someone Else <else@example.invalid>' || exit 1
    expect "dco: a sign-off that is not the author's" "no Signed-off-by: line with their author's email" dco HEAD~1..HEAD || rc=1
    commit --allow-empty -m $'author in capitals, and a second sign-off\n\nSigned-off-by: Someone Else <else@example.invalid>\nSigned-off-by: Fixture <Fixture@Example.invalid>' || exit 1
    expect "dco: the author's among several, any case" pass dco HEAD~1..HEAD || rc=1
    exit "$rc"
  ) || bad=1

  rm -rf "$t"
  trap - EXIT
  if [ "$bad" != 0 ]; then
    say "selftest: a gate does not hold (above)"
    return 1
  fi
  echo "selftest: every gate fails its over-budget fixtures and passes the others"
}

case "${1:-}" in
  refs)
    refs "${2:-}"
    exit
    ;;
  dco)
    dco "${2:-}"
    exit
    ;;
  selftest)
    selftest
    exit
    ;;
  install-hook)
    file="$(git rev-parse --git-path info/private-patterns)"
    if ! grep -qvE '^(#|$)' "$file" 2>/dev/null; then
      echo "check.sh: no patterns in $file; write them first (the hook fails without them)" >&2
      exit 1
    fi
    hook="$(git rev-parse --git-path hooks)/pre-push"
    cat >"$hook" <<'HOOK'
#!/usr/bin/env bash
# Installed by `./check.sh install-hook`: run the full check on the working tree, then refuse a
# push whose commits (messages or added lines) match the private references in
# .git/info/private-patterns, or that lack a sign-off. Checks every pushed commit that no
# remote-tracking branch of this remote already has. Without the pattern file the push fails.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export CHECK_FROM_HOOK=1
./check.sh </dev/null
zero=0000000000000000000000000000000000000000
while read -r _ local_sha _ remote_sha; do
  [ "$local_sha" = "$zero" ] && continue
  range="$local_sha --not --remotes=$1"
  if [ "$remote_sha" != "$zero" ] && git cat-file -e "$remote_sha^{commit}" 2>/dev/null; then range="$range $remote_sha"; fi
  ./check.sh refs "$range"
  ./check.sh dco "$range"
done
HOOK
    chmod +x "$hook"
    echo "installed $hook"
    exit
    ;;
  "") ;;
  *)
    echo "usage: ./check.sh [refs [RANGE] | dco [RANGE] | selftest | install-hook]" >&2
    exit 2
    ;;
esac

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Pinned, so the size gate measures what this run built, whatever cargo's config says.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-$PWD/target}}"
target="$CARGO_TARGET_DIR"

step "no private references"
refs

step "signed off (commits on no remote)"
dco

step "gate self-test"
selftest

step format
cargo fmt --check

step clippy
cargo clippy --locked --all-targets -- -D warnings

step tests
cargo test --locked

# The module manager: its own suite (hermetic, with live prompt and eval checks against the henki the tests just
# built), then the official modules: their own tests, and the layout henki loads; then the general eval suite's layout.
step "mods (manager/tests.sh, manager/mods-tests.sh, mods/*/, eval/general/)"
MODS_TEST_HENKI="$target/debug/henki" manager/tests.sh
MODS_TEST_HENKI="$target/debug/henki" manager/mods-tests.sh
gate_mods mods
manager/henki-mods eval --check eval/general

step "lines of code (src/, non-test)"
gate_lines src

step "crates (normal dependency tree)"
tree="$(cargo tree --locked -e normal --prefix none --no-dedupe | LC_ALL=C sort -u)"
printf '%s\n' "$tree"
gate_crates "$tree" "$(<Cargo.lock)"

# Builds a release binary (cargo build arguments) and holds its bytes to the gate. The old binary
# goes first, so a build that lands elsewhere fails instead of measuring a stale file.
size_gate() {
  local bin="$1"
  shift
  rm -f "$bin"
  cargo build --locked --release "$@"
  [ -f "$bin" ] || fail "cargo built no $bin (is build.target set in a cargo config?)"
  gate_bytes "$bin"
}

step "release size"
size_gate "$target/release/henki"
# The static Linux build that goes into an isolated environment.
if [ "$(uname -s)" = Linux ]; then
  case "$(uname -m)" in
    aarch64 | arm64) musl=aarch64-unknown-linux-musl ;;
    x86_64) musl=x86_64-unknown-linux-musl ;;
    *) musl="" ;;
  esac
  if [ -z "$musl" ]; then
    echo "skipped: no static musl build for $(uname -m)"
  elif [ -d "$(rustc --print sysroot)/lib/rustlib/$musl/lib" ]; then
    size_gate "$target/$musl/release/henki" --target "$musl"
  else
    fail "the $musl target is not installed (rustup target add $musl, or set RUSTUP_HOME to a rustup home that has it)"
  fi
fi

# Limits that hold only for an optimized build (timings, memory), as tests marked
# #[ignore = "release gate"]; #[ignore] is used for nothing else, so none is skipped silently.
step "release gates (ignored tests, release profile)"
cargo test --locked --release -- --ignored

step "all checks passed"
