#!/usr/bin/env bash
# Everything a change must pass that needs no human, in order, stopping at the first failure:
# no private references, formatting, lints, the tests, the size budgets (lines of code, crates
# in the dependency tree, release binary bytes), then the release gates: the tests marked
# #[ignore = "release gate"] (timing and memory limits), built and run with the release profile.
# Run it before pushing. Bash on macOS and Linux.
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
# `./check.sh install-hook` makes every git push run the full check, then the reference check on
# the commits it pushes; it refuses to install without the pattern file.
set -euo pipefail
cd "$(dirname "$0")"

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

case "${1:-}" in
  refs)
    refs "${2:-}"
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
# .git/info/private-patterns. Checks every pushed commit that no remote-tracking branch of this
# remote already has. Without the pattern file the push fails.
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
done
HOOK
    chmod +x "$hook"
    echo "installed $hook"
    exit
    ;;
  "") ;;
  *)
    echo "usage: ./check.sh [refs [RANGE] | install-hook]" >&2
    exit 2
    ;;
esac

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Pinned, so the size gate measures what this run built, whatever cargo's config says.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-$PWD/target}}"
target="$CARGO_TARGET_DIR"
max_lines=2600      # non-test lines in src/
max_file_lines=400  # per file; a module is one file
max_crates=10       # packages in the normal dependency tree, henki included
max_bytes=1000000   # release binary

step() { printf '\n== %s\n' "$*"; }
fail() {
  echo "check.sh: $*" >&2
  exit 1
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

step "no private references"
refs

step format
cargo fmt --check

step clippy
cargo clippy --locked --all-targets -- -D warnings

step tests
cargo test --locked

step "lines of code (src/, non-test)"
files=()
while IFS= read -r f; do files+=("$f"); done < <(find src -type f -name '*.rs' | LC_ALL=C sort)
[ "${#files[@]}" -gt 0 ] || fail "no Rust files in src/"
counts="$(rust_lines "${files[@]}")"
printf '%s\n' "$counts"
total="$(awk '{ t += $1 } END { print t + 0 }' <<<"$counts")"
echo "total $total (gate $max_lines; any file $max_file_lines)"
[ "$total" -le "$max_lines" ] || fail "$total lines of non-test Rust, over $max_lines"
big="$(awk -v max="$max_file_lines" '$1 > max' <<<"$counts")"
[ -z "$big" ] || fail "files over $max_file_lines lines: $big"

step "crates (normal dependency tree)"
tree="$(cargo tree --locked -e normal --prefix none --no-dedupe | LC_ALL=C sort -u)"
printf '%s\n' "$tree"
crates="$(grep -c . <<<"$tree")"
echo "$crates packages (gate $max_crates)"
[ "$crates" -le "$max_crates" ] || fail "$crates packages in the normal dependency tree, over $max_crates"
if grep -F '(proc-macro)' <<<"$tree"; then fail "a proc-macro crate is in the normal dependency tree"; fi
if grep -E '^name = "(cc|cmake|pkg-config)"$' Cargo.lock; then fail "a C build dependency is in Cargo.lock"; fi

# Builds a release binary (cargo build arguments) and holds its bytes to the gate. The old binary
# goes first, so a build that lands elsewhere fails instead of measuring a stale file.
size_gate() {
  local bin="$1" bytes
  shift
  rm -f "$bin"
  cargo build --locked --release "$@"
  [ -f "$bin" ] || fail "cargo built no $bin (is build.target set in a cargo config?)"
  bytes="$(wc -c <"$bin")"
  bytes=$((bytes))
  echo "$bin: $bytes bytes (gate $max_bytes)"
  [ "$bytes" -le "$max_bytes" ] || fail "$bin is $bytes bytes, over $max_bytes"
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
