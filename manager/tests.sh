#!/usr/bin/env bash
# The tests of henki-mods. Hermetic: every case gets its own HOME, XDG dirs and TMPDIR under one scratch dir, git
# fixtures are repositories this script makes (cloned through file:// URLs), and nothing touches the network or the
# user's config. Bash 3.2 compatible like the manager, so it runs on a stock Mac too.
#
#   manager/tests.sh                    every case, once with bash and once with bash --posix
#   MODS_TEST_HENKI=PATH manager/tests.sh
#       also checks the prompt bytes `list` reports against a real `henki run --json` request, sent to a small
#       python3 stub that records the system prompt (check.sh passes the henki it built)
#
# Exit 0 when every case passes; each failure prints the case, the check and the manager's output.
set -u
export LC_ALL=C
cd "$(dirname "$0")" || exit 1
HERE=$(pwd -P)
MODS="$HERE/henki-mods"
HENKI_BIN=${MODS_TEST_HENKI:-}
# The manager's `henki port` for the conformance suites (test --port, swap); none without MODS_TEST_HENKI.
[ -z "$HENKI_BIN" ] || export HENKI_BIN
unset MODS_TEST_HENKI HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG HENKI_SESSION HENKI_NEXT GIT_DIR GIT_WORK_TREE
export GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/henki-mods-tests.XXXXXX") || exit 1
trap 'chmod -R u+w "$SCRATCH" 2>/dev/null; rm -rf "$SCRATCH"' EXIT
tab=$(printf '\t')
nl='
'
FAILS=0
CASES=0
RUN=bash # the interpreter the manager runs under: bash, then bash --posix

if command -v sha256sum >/dev/null 2>&1; then sha() { sha256sum | awk '{ print $1 }'; }; else sha() { shasum -a 256 | awk '{ print $1 }'; }; fi

# --- the harness -------------------------------------------------------------------------------------------------------

# A case: a fresh HOME (its config dir empty), XDG dirs that are not the defaults, and a work dir W for fixtures.
# XDG_CONFIG_HOME points elsewhere on purpose: henki reads ~/.config/henki, so the manager must too.
case_() {
  CASE="$1"
  CASES=$((CASES + 1))
  local d="$SCRATCH/$CASES"
  mkdir -p "$d/home" "$d/w" "$d/tmp"
  export HOME="$d/home" XDG_DATA_HOME="$d/data" XDG_CONFIG_HOME="$d/xdg-config" XDG_STATE_HOME="$d/state" TMPDIR="$d/tmp"
  W="$d/w"
  CONF="$HOME/.config/henki"
  STORE="$XDG_DATA_HOME/henki/mods"
  unset HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG HENKI_SESSION HENKI_NEXT
}

# Runs the manager; OUT, ERR, RC.
run() {
  RC=0
  OUT=$($RUN "$MODS" "$@" 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
}

fail() {
  FAILS=$((FAILS + 1))
  printf 'FAIL [%s] %s: %s\n  rc=%s\n  out: %s\n  err: %s\n' "$RUN" "$CASE" "$*" "${RC:-}" "${OUT:-}" "${ERR:-}" | sed '4,$s/^/       /' >&2
}
rc_is() { [ "$RC" = "$1" ] || fail "exit $RC, wanted $1"; }
out_has() { case "$OUT" in *"$1"*) ;; *) fail "stdout lacks: $1" ;; esac; }
err_has() { case "$ERR" in *"$1"*) ;; *) fail "stderr lacks: $1" ;; esac; }
out_lacks() { case "$OUT" in *"$1"*) fail "stdout has: $1" ;; esac; }
err_lacks() { case "$ERR" in *"$1"*) fail "stderr has: $1" ;; esac; }
check() { # DESCRIPTION COMMAND...
  local what="$1"
  shift
  "$@" || fail "$what"
}
# A refusal: exit 1, henki's "henki-mods: PROBLEM; fix: ..." form, and TEXT in it.
refused() {
  rc_is 1
  err_has "$1"
  case "${ERR##*"$nl"}" in "henki-mods: "*"; fix: "*) ;; *) fail "the last line is not a 'henki-mods: PROBLEM; fix: ...' message" ;; esac
}

# A module DIR named by its basename: doc.md (line 1 SUMMARY), bin/NAME (755), a manifest (plus EXTRA lines).
mkmod() {
  local dir="$1" n="${1##*/}" extra="${2:-}"
  mkdir -p "$dir/bin" "$dir/.henki"
  printf '# %s: says hello for tests\n\nThe body is read on demand.\n' "$n" >"$dir/doc.md"
  printf '#!/bin/sh\necho %s\n' "$n" >"$dir/bin/$n"
  chmod 755 "$dir/bin/$n"
  printf 'henki_module = 1\nname = "%s"\nversion = "0.1.0"\nlicense = "0BSD"\ncaps_net = "none"\n%b' "$n" "$extra" >"$dir/.henki/module.toml"
}

# A git repository at DIR from what is already in it; one commit.
mkrepo() {
  git -C "$1" init -q 2>/dev/null || git init -q "$1"
  commit "$1" "first"
}
commit() {
  git -C "$1" add -A 2>/dev/null && git -C "$1" -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q --no-verify -m "$2" 2>/dev/null
}

num() { printf '%s\n' "$1" | awk '{ s = $0; o = ""; while (length(s) > 3) { o = "," substr(s, length(s) - 2) o; s = substr(s, 1, length(s) - 3) } print s o }'; }
lockline() { awk -F '\t' -v n="$1" '$2 == n' "$CONF/mods.lock" 2>/dev/null; }
field() { lockline "$1" | cut -f "$2"; }

# --- the static check ---------------------------------------------------------------------------------------------------

# Bash 4+ only syntax, which a stock Mac's bash 3.2 does not run (there is no bash 3.2 here to run it under).
static_bash32() {
  case_ "static: no bash 4+ features"
  local f hits
  for f in "$MODS" "$HERE/henki-mods-dev" "$HERE/tests.sh"; do
    hits=$(command grep -nE '(^|[^[:alnum:]_])(mapfile|readarray|coproc)([^[:alnum:]_]|$)|(declare|local|typeset) +-[a-zA-Z]*[An]|\$\{[[:alnum:]_]+(,,?|\^\^?)[}[:alnum:]]|\$\{[[:alnum:]_]+@[QEPAa]\}|\|&|&>>|;;&|\[\[ +-v |\$\{[[:alnum:]_]+\[-[0-9]|EPOCH(SECONDS|REALTIME)|BASHPID|wait +-n|shopt +-s +(globstar|lastpipe|inherit_errexit)|\{[0-9]+\.\.[0-9]+\.\.' "$f" |
      command grep -vE '^[0-9]+:[[:space:]]*#|command grep -nE' || true)
    [ -z "$hits" ] || fail "$f uses bash 4+ syntax:$(printf '\n%s' "$hits")"
  done
  # GNU-only flags and tools, which a Mac lacks.
  for f in "$MODS" "$HERE/henki-mods-dev" "$HERE/tests.sh"; do
    hits=$(command grep -nE '(^|[^[:alnum:]_-])(sed -i|readlink -f|stat -c|grep -P|date -d|mv -T|cp -T|sort -V|realpath|tac|xargs -r)([^[:alnum:]_-]|$)|-printf' "$f" |
      command grep -vE '^[0-9]+:[[:space:]]*#|command grep -nE' || true)
    [ -z "$hits" ] || fail "$f uses a GNU-only tool or flag:$(printf '\n%s' "$hits")"
  done
  # Arrays: an empty one is "unbound" under set -u before bash 4.4, so the manager has none.
  hits=$(command grep -nE '^[^#]*([[:alnum:]_]+=\(|\$\{[[:alnum:]_]+\[)' "$MODS" || true)
  [ -z "$hits" ] || fail "henki-mods uses an array:$(printf '\n%s' "$hits")"
  # awk -v: macOS's awk refuses a value holding a newline and then runs nothing ("newline in string"), and every
  # awk expands backslash escapes in it; mawk and gawk here take both, so only a look at the text finds one.
  hits=$(command grep -nE '^[^#]*(^|[^[:alnum:]_])awk([[:space:]]+-F[[:space:]]*[^[:space:]]+)*[[:space:]]+-v' "$MODS" || true)
  [ -z "$hits" ] || fail "henki-mods passes data to awk with -v (use ENVIRON):$(printf '\n%s' "$hits")"
  bash -n "$MODS" || fail "bash -n henki-mods"
  # A bash 3.2 built for tests (HENKI_TEST_BASH32) parses what this bash cannot judge: a case pattern inside $(...)
  # without its opening parenthesis is a syntax error there.
  if [ -n "${HENKI_TEST_BASH32:-}" ]; then
    for f in "$MODS" "$HERE/henki-mods-dev" "$HERE/tests.sh" "$HERE/mods-tests.sh"; do
      "$HENKI_TEST_BASH32" -n "$f" || fail "bash 3.2 cannot parse $f"
    done
  fi
}

# The contracts henki-mods accepts in a manifest are the rows of CONTRACTS.md's index, no more and no fewer: a
# contract added there and not here would refuse every module that names it.
static_contracts() {
  case_ "static: CONTRACTS_KNOWN is the CONTRACTS.md index"
  local doc known
  doc=$(awk '/^## Index$/ { p = 1; next } /^## / { p = 0 } p && /^\| `/ { split($0, c, "`"); print c[2] }' "$HERE/../CONTRACTS.md" | sort)
  known=$(sed -n "s/^CONTRACTS_KNOWN='\(.*\)'$/\1/p" "$MODS" | tr ' ' '\n' | sort)
  [ -n "$doc" ] || fail "CONTRACTS.md has no index rows"
  [ "$doc" = "$known" ] || fail "henki-mods knows [$(echo $known)], CONTRACTS.md's index lists [$(echo $doc)]"
}

# CONTRACTS.md's section NAME, its lines joined by spaces.
doc_sec() { HM_S="## $1" awk '$0 == ENVIRON["HM_S"] { p = 1; next } /^## / { p = 0 } p' "$HERE/../CONTRACTS.md" | tr '\n' ' '; }
# The number of fields of the line format KIND that CONTRACTS.md's section KIND shows in its code block (`KIND F...`).
doc_fields() { HM_K="$1" awk '$1 == ENVIRON["HM_K"] && NF > 3 { print NF; exit }' "$HERE/../CONTRACTS.md"; }

# The other lists henki-mods shares with CONTRACTS.md (the names henki keeps, the observer events, the ports henki
# binds, the text limit), and the conform/1 files against the format.
static_formats() {
  case_ "static: henki-mods' names and the conform/1 suites as CONTRACTS.md has them"
  local ext2 obs port mine doc f line name inst req want save names="" rest
  mine() { sed -n "s/^$1='\(.*\)'.*$/\1/p" "$MODS" | tr ' ' '\n' | sort | tr '\n' ' '; }
  ext2=$(doc_sec ext/2)
  doc=$(printf '%s\n' "$ext2" | sed -n 's/.*CMD as for bin\/, not \([a-z, ]*\) NAME\/cmd.*/\1/p' | sed 's/, / /g; s/ or / /' | tr ' ' '\n' | sort | tr '\n' ' ')
  [ "$doc" = "$(mine RESERVED)" ] || fail "RESERVED is [$(mine RESERVED)], ext/2 keeps [$doc]"
  case "$ext2" in *"at most 16 KiB of UTF-8"*) [ "$(sed -n 's/^DOC_MAX=\([0-9]*\).*/\1/p' "$MODS")" = 16384 ] || fail "DOC_MAX is not 16 KiB" ;; *) fail "ext/2 lost its 16 KiB" ;; esac
  obs=$(doc_sec observe/1)
  doc=$( (HM_S="## observe/1" awk '$0 == ENVIRON["HM_S"] { p = 1; next } /^## / { p = 0 } p && /^\| `/ { split($0, c, "`"); print c[2] }' "$HERE/../CONTRACTS.md"
    printf '%s\n' "$obs" | sed -n 's/.*NAME\/on\/EVENT\.wait  *\([a-z_]*\) and \([a-z_]*\) only.*/\1.wait \2.wait/p' | tr ' ' '\n') | sort | tr '\n' ' ')
  [ "$doc" = "$(mine EVENTS)" ] || fail "EVENTS is [$(mine EVENTS)], observe/1 has [$doc]"
  port=$(doc_sec port/1)
  doc=$(printf '%s\n' "$port" | sed -n 's/.*This henki binds `\([a-z]*\)` and `\([a-z]*\)` that way.*/\1 \2/p' | tr ' ' '\n' | sort | tr '\n' ' ')
  [ "$doc" = "$(mine PORTS_KNOWN)" ] || fail "PORTS_KNOWN is [$(mine PORTS_KNOWN)], port/1 binds [$doc]"
  for f in "$HERE"/conform/*.tsv; do
    names=""
    while IFS= read -r line; do
      case "$line" in '' | '#'*) continue ;; esac
      IFS="$tab" read -r name inst req want save rest <<EOF
$line
EOF
      [ -z "$rest" ] && [ -n "$want" ] || fail "${f##*/}: $name: not 4 or 5 fields"
      case "$inst" in a | b | a! | +[0-9] | +[0-9][0-9]) ;; *) fail "${f##*/}: $name: INST $inst" ;; esac
      case "$req" in hello | '{"method":"'*'","params":'*'}') ;; *) [ "$inst" = a! ] || fail "${f##*/}: $name: REQUEST $req" ;; esac
      case "$save" in '' | [A-Za-z]*=[a-z]*) ;; *) fail "${f##*/}: $name: SAVE $save" ;; esac
      case " $names " in *" $name "*) fail "${f##*/}: $name twice" ;; esac
      names="$names $name"
    done <"$f"
  done
  [ "$(doc_fields bind/1)" = 6 ] && [ "$(doc_fields eval/1)" = 10 ] && [ "$(doc_fields lock/1)" = 11 ] || fail "doc_fields reads $(doc_fields bind/1) $(doc_fields eval/1) $(doc_fields lock/1)"
}

# --- the cases ----------------------------------------------------------------------------------------------------------

t_add_path() {
  case_ "add from a path, --yes"
  mkmod "$W/hello"
  run add "$W/hello" --yes
  rc_is 0
  out_has "hello 0.1.0"
  out_has "license  0BSD"
  out_has "network  none (declared, NOT enforced)"
  out_has "bin/hello (sh, 2 lines)"
  out_has "restart henki (or /exit and start again) to load the change"
  out_lacks "UNREVIEWED"
  local l hd store
  l=$(lockline hello)
  hd=$(field hello 8)
  store="$STORE/hello/$(printf '%s' "${hd#hd1:}" | cut -c1-12)"
  check "11 tab-separated fields, MASK empty" [ "$(printf '%s' "$l" | awk -F '\t' '{ print NF ":" $11 }')" = "11:" ]
  check "lock/1 hello on 0.1.0 path SOURCE - " [ "$(printf '%s' "$l" | cut -f 1-7)" = "lock/1${tab}hello${tab}on${tab}0.1.0${tab}path${tab}$W/hello${tab}-" ]
  check "ADDED is UTC minutes" sh -c "printf '%s' '$(field hello 9)' | command grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z$'"
  check "HOW is yes" [ "$(field hello 10)" = yes ]
  check "the link points at the store copy" [ "$(readlink "$CONF/ext/hello")" = "$store" ]
  check "no file under XDG_CONFIG_HOME" [ ! -e "$XDG_CONFIG_HOME" ]
  check "store files are read-only" [ -z "$(find "$store" ! -type l -perm -200 -print)" ]
  check "store dirs are read-only" [ ! -w "$store/bin" ] || [ "$(id -u)" = 0 ]
  check "no scratch dir left in the store" [ -z "$(find "$STORE" -name '.add.*' -print)" ]
  check "no busy dir left" [ ! -e "$CONF/mods.busy" ]
  run verify
  rc_is 0
  out_has "verify: ok (1 modules)"
  # A path with #SUBDIR, and the same module twice.
  case_ "add DIR#SUBDIR from a path"
  mkmod "$W/repo/mods/hi"
  run add "$W/repo#mods/hi" --yes
  rc_is 0
  check "lock names the dir" [ "$(field hi 6)" = "$W/repo/mods/hi" ]
  run add "$W/repo/mods/hi" --yes
  refused "hi is already added"
}

t_ask() {
  case_ "add asks on the terminal, default no"
  mkmod "$W/hello"
  printf '\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run add "$W/hello"
  refused "not installed (the answer was no)"
  out_has "hello 0.1.0"
  check "nothing linked" [ ! -e "$CONF/ext/hello" ]
  check "no lock line" [ -z "$(lockline hello)" ]
  check "no store copy" [ -z "$(find "$STORE" -mindepth 1 -maxdepth 1 ! -name '.*' -print 2>/dev/null)" ]
  printf 'y\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run add "$W/hello"
  rc_is 0
  check "HOW is tty" [ "$(field hello 10)" = tty ]
  check "on" [ "$(field hello 3)" = on ] && [ -L "$CONF/ext/hello" ]
  case_ "add without a terminal and without --yes"
  mkmod "$W/hello"
  HENKI_MODS_TTY="$W/none" run add "$W/hello"
  refused "no terminal to ask on"
  check "nothing linked" [ ! -e "$CONF/ext/hello" ]
}

t_git() {
  case_ "add from a git URL with @REF and #SUBDIR"
  local r="$W/repo" c1 c2 store
  mkmod "$r/mods/hello"
  printf 'first\n' >"$r/README.md"
  mkrepo "$r"
  git -C "$r" tag v1
  c1=$(git -C "$r" rev-parse HEAD)
  printf '# hello: version two\n' >"$r/mods/hello/doc.md"
  commit "$r" second
  git -C "$r" branch feature/x
  c2=$(git -C "$r" rev-parse HEAD)
  run add "file://$r@v1#mods/hello" --yes
  rc_is 0
  out_has "UNREVIEWED"
  out_has "commit $c1"
  check "SOURCE is URL#SUBDIR" [ "$(field hello 6)" = "file://$r#mods/hello" ]
  check "COMMIT is the tag's commit" [ "$(field hello 7)" = "$c1" ]
  check "KIND url" [ "$(field hello 5)" = url ]
  store="$STORE/hello/$(printf '%s' "$c1" | cut -c1-12)"
  check "store ID is the commit's first 12 hex" [ "$(readlink "$CONF/ext/hello")" = "$store" ]
  check "the copy is the tagged content" command grep -q 'says hello for tests' "$store/doc.md"
  check "nothing outside SUBDIR" [ ! -e "$store/README.md" ] && [ ! -e "$store/mods" ]
  run rm hello
  rc_is 0
  run add "file://$r#mods/hello" --yes
  rc_is 0
  check "no REF: the default branch" [ "$(field hello 7)" = "$c2" ]
  run rm hello
  run add "file://$r@feature/x#mods/hello/" --yes
  rc_is 0
  check "a branch with a slash" [ "$(field hello 7)" = "$c2" ]
  run rm hello
  run add "file://$r@$(printf '%s' "$c1" | cut -c1-10)#mods/hello" --yes
  rc_is 0
  check "a short commit" [ "$(field hello 7)" = "$c1" ]
  run rm hello
  run add "file://$r@nope#mods/hello" --yes
  refused "has no nope to pin"
  run add "file://$r@v1#mods/absent" --yes
  refused "has no directory mods/absent"
  run add "file://$W/no-such-repo" --yes
  refused "git clone"
  # A whole repository is one module; its name comes from the URL.
  case_ "add a whole repository by URL"
  mkdir -p "$W/henki-tool.git"
  mkmod "$W/henki-tool.git/tool"
  mv "$W/henki-tool.git/tool/"* "$W/henki-tool.git/tool/.henki" "$W/henki-tool.git/"
  rmdir "$W/henki-tool.git/tool"
  mv "$W/henki-tool.git/bin/tool" "$W/henki-tool.git/bin/henki-tool"
  sed 's/^name = .*//' "$W/henki-tool.git/.henki/module.toml" >"$W/m" && mv "$W/m" "$W/henki-tool.git/.henki/module.toml"
  mkrepo "$W/henki-tool.git"
  run add "file://$W/henki-tool.git" --yes
  refused "no name"
  rm -rf "$W/henki-tool.git/.henki"
  commit "$W/henki-tool.git" "drop the manifest"
  run add "file://$W/henki-tool.git" --yes
  rc_is 0
  out_has "manifest none (unmanaged"
  check "unmanaged name from the URL, .git dropped" [ -L "$CONF/ext/henki-tool" ]
  check "VERSION -" [ "$(field henki-tool 4)" = - ]
  run list
  out_has "(unmanaged)"
}

t_raw_blobs() {
  case_ "the copy is the raw blobs: 755 kept, links kept, no filters"
  local r="$W/repo" s
  mkmod "$r/tool"
  ln -s tool "$r/tool/bin/tool-alias"
  ln -s ../doc.md "$r/tool/bin/.doc-link"
  printf 'plain\n' >"$r/tool/README.notes"
  chmod 644 "$r/tool/README.notes"
  mkrepo "$r"
  # A clean filter in the user's git config must not touch the copy (no checkout runs it).
  git config --global filter.evil.clean 'echo FILTERED' 2>/dev/null || true
  run add "file://$r#tool" --yes
  rc_is 0
  s=$(readlink "$CONF/ext/tool")
  check "bin/tool keeps its x bit" [ -n "$(find "$s/bin/tool" -perm -100 -print)" ]
  check "README.notes has no x bit" [ -z "$(find "$s/README.notes" -perm -100 -print)" ]
  check "bin/tool-alias is a link to tool" [ -L "$s/bin/tool-alias" ] && [ "$(readlink "$s/bin/tool-alias")" = tool ]
  check "the dot link is kept" [ "$(readlink "$s/bin/.doc-link")" = ../doc.md ]
  check "the blob bytes" [ "$(cat "$s/bin/tool")" = "$(printf '#!/bin/sh\necho tool')" ]
  check "hd1 in the lock is the store's" [ "$(field tool 8)" = "$(hd1_by_hand "$s")" ]
  out_has "files    6 (2 links)"
}

# hd1 the slow way, for comparison: a find, a sort, one sha per entry, by the definition.
hd1_by_hand() {
  local d="$1" p mode h
  (cd "$d" && find . -path ./.git -prune -o \( -type f -o -type l \) -print) | sed 's|^\./||' | sort | while IFS= read -r p; do
    if [ -L "$d/$p" ]; then
      mode=link
      h=$(printf '%s' "$(readlink "$d/$p")" | sha)
    else
      if [ -n "$(find "$d/$p" \( -perm -100 -o -perm -010 -o -perm -001 \) -print)" ]; then mode=755; else mode=644; fi
      h=$(sha <"$d/$p")
    fi
    printf '%s %s %s\n' "$mode" "$h" "$p"
  done | sha | sed 's/^/hd1:/'
}

t_hd1() {
  case_ "hd1 known answer"
  local m="$W/kat"
  mkdir -p "$m/bin" "$m/.henki"
  printf 'doc line\n' >"$m/doc.md"
  printf '#!/bin/sh\n' >"$m/bin/run"
  chmod 750 "$m/bin/run"
  printf 'henki_module = 1\nname = "kat"\nversion = "1"\nlicense = "MIT"\n' >"$m/.henki/module.toml"
  ln -s doc.md "$m/README.md"
  mkdir -p "$m/.git" && printf 'excluded\n' >"$m/.git/HEAD"
  # By the definition, sorted bytewise by hand: ".henki/..." < "README.md" < "bin/run" < "doc.md".
  local want
  want="hd1:$(
    printf '644 %s .henki/module.toml\n' "$(printf 'henki_module = 1\nname = "kat"\nversion = "1"\nlicense = "MIT"\n' | sha)"
    printf 'link %s README.md\n' "$(printf 'doc.md' | sha)"
    printf '755 %s bin/run\n' "$(printf '#!/bin/sh\n' | sha)"
    printf '644 %s doc.md\n' "$(printf 'doc line\n' | sha)"
  )"
  want="hd1:$(printf '%s\n' "${want#hd1:}" | sha)"
  run verify --dir "$m"
  rc_is 0
  out_has "$want"
  run add "$m" --yes
  rc_is 0
  check "the lock has the known answer" [ "$(field kat 8)" = "$want" ]
  check "store ID is the hd1's first 12 hex" [ "$(readlink "$CONF/ext/kat")" = "$STORE/kat/$(printf '%s' "${want#hd1:}" | cut -c1-12)" ]
  check "the hand-rolled hash agrees" [ "$(hd1_by_hand "$(readlink "$CONF/ext/kat")")" = "$want" ]
}

t_refusals() {
  case_ "refusals"
  local m="$W/m"
  mkmod "$W/Bad"
  sed 's/^name = .*/name = "Bad_Name"/' "$W/Bad/.henki/module.toml" >"$W/t" && mv "$W/t" "$W/Bad/.henki/module.toml"
  run add "$W/Bad" --yes
  refused "name: a name is 1 to 32 of a-z"
  rm -rf "$W/Bad/.henki"
  run add "$W/Bad" --yes
  refused "the name \"Bad\""

  mkmod "$m" 'colour = "red"\n'
  run add "$m" --yes
  refused "unknown key colour"
  run verify --dir "$m"
  refused "unknown key colour"

  rm -rf "$m" && mkmod "$m" 'os = ["plan9"]\n'
  run add "$m" --yes
  refused 'os "plan9" is not macos or linux'
  local other=linux
  [ "$(uname -s)" != Linux ] || other=macos
  rm -rf "$m" && mkmod "$m" "os = [\"$other\"]\n"
  run add "$m" --yes
  refused "runs on $other only"
  run verify --dir "$m"
  rc_is 0

  rm -rf "$m" && mkmod "$m" 'henki_module = 2\nrequires = ["henki >= 0.2"]\n'
  sed '1d' "$m/.henki/module.toml" >"$W/t" && mv "$W/t" "$m/.henki/module.toml"
  mkdir -p "$m/cmd"
  run add "$m" --yes
  refused "henki_module = 2 needs henki >= 0.2"

  rm -rf "$m" && mkmod "$m"
  mkdir -p "$m/ctx" && printf 'x\n' >"$m/ctx/x"
  run add "$m" --yes
  refused "ctx/: a later module layout (context)"
  rm -rf "$m/ctx" && mkdir "$m/lib" && printf 'x\n' >"$m/lib/x"
  run add "$m" --yes
  refused "lib: unknown entry"

  rm -rf "$m" && mkmod "$m"
  ln -s ../../outside "$m/bin/escape"
  run add "$m" --yes
  refused "leaves the module"
  rm "$m/bin/escape" && ln -s /bin/sh "$m/bin/escape"
  run add "$m" --yes
  refused "is absolute, leaves the module"
  rm "$m/bin/escape" && ln -s ../bin/../../m/doc.md "$m/README.x"
  run add "$m" --yes
  refused "leaves the module"
  # A link to . makes the words lie: README.x/README.x/../.. counts as one level down, and is one up.
  local down="" up="" i
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do down="${down}README.x/" up="${up}../"; done
  rm "$m/README.x" && ln -s . "$m/README.x" && ln -s "$down${up}etc/hostname" "$m/README.host"
  check "the fixture reaches out" [ "$(cat "$m/README.host")" = "$(cat /etc/hostname)" ]
  run verify --dir "$m"
  refused "README.host: a symlink to README.x/README.x/"
  err_has "goes through another link"
  run add "$m" --yes
  refused "goes through another link"
  check "nothing installed" [ ! -e "$CONF/mods.lock" ]
  # Links to links, and through real directories, are fine.
  rm "$m/README.host" && ln -s README.x "$m/README.y" && ln -s bin/../doc.md "$m/README.z" && ln -s README.y/doc.md "$m/README.w"
  run verify --dir "$m"
  refused "README.w: a symlink to README.y/doc.md"
  rm "$m/README.w"
  run verify --dir "$m"
  rc_is 0
  rm "$m/README.x" "$m/README.y" "$m/README.z"

  printf 'not runnable\n' >"$m/bin/data"
  run add "$m" --yes
  refused "a command is a file you can run"
  rm "$m/bin/data"
  rm "$m/doc.md"
  run add "$m" --yes
  refused "no doc.md"
  printf '#\n' >"$m/doc.md"
  run add "$m" --yes
  refused "line 1 must be a summary of 1 to 160 bytes"
  awk 'BEGIN { for (i = 0; i < 161; i++) printf "x"; print "" }' >"$m/doc.md"
  run add "$m" --yes
  refused "line 1 must be a summary"
  if command -v iconv >/dev/null 2>&1; then
    printf 'zero\342\200\213width\n' >"$m/doc.md"
    run add "$m" --yes
    refused "holds U+200B"
  fi
  printf 'ok\n' >"$m/doc.md"

  rm -rf "$m" && mkmod "$m"
  mkdir "$m/empty"
  run verify --dir "$m"
  refused "empty: unknown entry"
  rmdir "$m/empty" && mkdir "$m/bin/sub"
  run verify --dir "$m"
  refused "bin/sub: a directory"
  rmdir "$m/bin/sub"
  mv "$m/.henki/module.toml" "$m/manifest" && ln -s ../manifest "$m/.henki/module.toml"
  run verify --dir "$m"
  refused "module.toml: not a regular file"

  rm -rf "$m" && mkmod "$m" 'ports = ["history"]\n'
  run add "$m" --yes
  refused "ports must name exactly the programs in port/"
  rm -rf "$m" && mkmod "$m" 'ports = ["context"]\n'
  run add "$m" --yes
  refused "port context is not one this henki binds (history policy)"
  rm -rf "$m" && mkmod "$m" && mkdir "$m/port" && printf '#!/bin/sh\n' >"$m/port/history"
  run add "$m" --yes
  refused "port/history: port/ holds a program you can run per port"
  chmod 755 "$m/port/history" && mv "$m/port/history" "$m/port/context"
  run add "$m" --yes
  refused "port/context: port/ holds a program you can run per port, named history policy"
  rm -rf "$m" && mkmod "$m" 'contracts = ["ext/1", "context/1"]\n'
  run add "$m" --yes
  refused "contract context/1 is not one this henki ships"
  rm -rf "$m" && mkmod "$m" 'caps_net = "lots"\n'
  sed '/^caps_net = "none"/d' "$m/.henki/module.toml" >"$W/t" && mv "$W/t" "$m/.henki/module.toml"
  run add "$m" --yes
  refused 'caps_net "lots"'
  rm -rf "$m" && mkmod "$m" 'license = "MIT"\n'
  run add "$m" --yes
  refused "duplicate key license"
  rm -rf "$m" && mkmod "$m" 'summary = "a \\"quote\\""\n'
  run add "$m" --yes
  refused "escape"
  rm -rf "$m" && mkmod "$m" 'shadow = ["nope"]\n'
  run add "$m" --yes
  refused "a shadow entry is not EXT/COMMAND"
  rm -rf "$m" && mkmod "$m" 'requires = ["itse >= 1"]\n'
  run add "$m" --yes
  refused 'requires "itse >= 1" is not "henki >= VERSION"'
  rm -rf "$m" && mkmod "$m"
  printf 'x\n' >"$m/.henki/notes"
  run add "$m" --yes
  refused ".henki holds module.toml, card.md and template"

  # Nothing of a refused add stays behind.
  check "no link after refusals" [ ! -e "$CONF/ext" ] || [ -z "$(ls "$CONF/ext")" ]
  check "no lock after refusals" [ ! -e "$CONF/mods.lock" ]
  check "no store copy after refusals" [ -z "$(find "$STORE" -mindepth 1 -maxdepth 1 -print 2>/dev/null)" ]

  # A dir in ext/ that henki-mods did not make.
  rm -rf "$m" && mkmod "$m"
  mkdir -p "$CONF/ext/m"
  run add "$m" --yes
  refused "exists and henki-mods did not make it"
  rmdir "$CONF/ext/m"

  # Usage errors exit 2.
  run frobnicate
  rc_is 2
  run on
  rc_is 2
  run add
  rc_is 2
  run add "$m" --force
  rc_is 2
  run on nothing-here
  refused "nothing-here is not added"
}

t_gitattributes() {
  case_ "refuse .gitattributes export-subst (and kin)"
  local r="$W/repo" a
  mkmod "$r/tool"
  printf '* -diff\n' >"$r/.gitattributes"
  printf 'doc.md export-subst\n' >"$r/tool/.gitattributes"
  mkrepo "$r"
  run add "file://$r#tool" --yes
  refused "tool/.gitattributes sets export-subst"
  check "nothing installed" [ ! -e "$CONF/mods.lock" ]
  for a in 'bin/* filter=lfs' '*.md text eol=lf' '# export-subst only in a comment' 'tool/x ident' '[attr]mine export-ignore' \
    '*.md crlf' '*.md -crlf' '*.md crlf=input' '*.md working-tree-encoding=UTF-16'; do
    printf '%s\n' "$a" >"$r/tool/.gitattributes"
    commit "$r" "attrs: $a"
    run add "file://$r#tool" --yes
    case "$a" in
      '#'*) rc_is 0 && run rm tool ;;
      *) refused "sets export-subst, export-ignore, ident, filter, text, eol, crlf or working-tree-encoding" ;;
    esac
  done
  # Attributes of a directory above the module apply to it.
  printf '\n' >"$r/tool/.gitattributes"
  printf 'tool/doc.md export-subst\n' >"$r/.gitattributes"
  commit "$r" "root attrs"
  run add "file://$r#tool" --yes
  refused ".gitattributes sets export-subst"
}

# Only https, http, ssh, git and file reach git; a remote helper (ext::CMD runs CMD at clone time, before the screen
# and the question) is refused by the parser, and git is held to the list too, whatever the config says.
t_transport() {
  case_ "git transports: a remote helper never runs"
  local pw="$W/pwned" u r="$W/repo"
  # ext enabled every way the environment can: the parser must refuse before git sees it.
  for u in "ext::sh -c touch% $pw;% :% a@b:c" "ext::sh -c touch% $pw" "fd::7" "EXT::sh -c touch% $pw" "testing::x@y:z"; do
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.ext.allow GIT_CONFIG_VALUE_0=always run add "$u" --yes
    refused "a git remote helper"
    check "$u ran nothing" [ ! -e "$pw" ]
  done
  for u in "foo://h/x" "HTTPS://h/x" "ftp://h/x" "ssh://-oProxyCommand=touch% $pw/x" "ssh://u@-oProxyCommand=x/y" "a@-oProxyCommand=x:y" "a@b@c:d" "a b@c:d"; do
    run add "$u" --yes
    refused "fix: use an https, http, ssh, git or file URL"
  done
  check "nothing ran" [ ! -e "$pw" ]
  # Past the parser, git is pinned: an insteadOf rewrite of an https URL into ext:: fails, with ext allowed in
  # ~/.gitconfig, in GIT_CONFIG_COUNT and in GIT_CONFIG_PARAMETERS, and GIT_ALLOW_PROTOCOL naming ext.
  git config --global protocol.ext.allow always
  git config --global url."ext::sh -c touch% $pw% #".insteadOf "https://mods.invalid/"
  GIT_ALLOW_PROTOCOL=ext:https GIT_CONFIG_PARAMETERS="'protocol.ext.allow'='always'" \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.ext.allow GIT_CONFIG_VALUE_0=always run add "https://mods.invalid/x" --yes
  refused "git clone https://mods.invalid/x failed"
  err_has "transport 'ext' not allowed"
  check "the rewritten helper ran nothing" [ ! -e "$pw" ]
  # The allowed ones still work: file:// here, and USER@HOST:PATH goes to ssh (the user's GIT_SSH_COMMAND, kept).
  git config --global --unset-all url."ext::sh -c touch% $pw% #".insteadOf
  mkmod "$r/hello"
  mkrepo "$r"
  run add "file://$r#hello" --yes
  rc_is 0
  GIT_SSH_COMMAND="touch $W/ssh-ran; false" run add "git@mods.invalid:o/r.git" --yes
  refused "git clone git@mods.invalid:o/r.git failed"
  check "scp form goes to ssh" [ -e "$W/ssh-ran" ]
}

# A tree whose names are one name where case does not count. On a Mac a link README and a directory readme/ in one
# tree put readme/x where the link points, outside the copy; refused before anything is written.
t_case_clash() {
  case_ "git trees with names that differ only in case"
  local r="$W/repo" blob link sub tree out="$W/outside"
  mkdir -p "$out" "$r"
  git init -q "$r"
  blob=$(printf '# m: case\n' | git -C "$r" hash-object -w --stdin)
  link=$(printf '%s' "$out" | git -C "$r" hash-object -w --stdin)
  sub=$(printf '100644 blob %s\tREADME.x\n' "$blob" | git -C "$r" mktree)
  tree=$(printf '120000 blob %s\tREADME\n100644 blob %s\tdoc.md\n040000 tree %s\treadme\n' "$link" "$blob" "$sub" | git -C "$r" mktree)
  git -C "$r" update-ref refs/heads/main "$(git -C "$r" commit-tree -m clash "$tree")"
  git -C "$r" symbolic-ref HEAD refs/heads/main
  check "git itself takes the tree" git -C "$r" fsck --strict --no-progress
  run add "file://$r" --yes
  refused "README and readme are one name where case does not count"
  check "nothing written outside" [ -z "$(ls -A "$out")" ]
  check "nothing installed" [ ! -e "$CONF/mods.lock" ]
  # Two files, and a directory against a directory.
  tree=$(printf '100644 blob %s\tDoc.md\n100644 blob %s\tdoc.md\n' "$blob" "$blob" | git -C "$r" mktree)
  git -C "$r" update-ref refs/heads/main "$(git -C "$r" commit-tree -m files "$tree")"
  run add "file://$r" --yes
  refused "Doc.md and doc.md are one name"
  tree=$(printf '100644 blob %s\tdoc.md\n040000 tree %s\tREADME.d\n040000 tree %s\treadme.D\n' "$blob" "$sub" "$sub" | git -C "$r" mktree)
  git -C "$r" update-ref refs/heads/main "$(git -C "$r" commit-tree -m dirs "$tree")"
  run add "file://$r" --yes
  refused "README.d and readme.D are one name"
}

# Data with a backslash reaches awk intact (ENVIRON, not -v): a data dir named with \t in it. (A newline in a -v
# value, which macOS's awk refuses, is caught by the static check: the awks here take it.)
t_awk_data() {
  case_ "awk data: backslashes and newlines"
  export XDG_DATA_HOME="$XDG_DATA_HOME/back\\tslash"
  STORE="$XDG_DATA_HOME/henki/mods"
  mkdir -p "$CONF"
  printf '# mine\n' >"$CONF/AGENTS.md"
  mkmod "$W/carded"
  printf 'The card.\n' >"$W/carded/.henki/card.md"
  run add "$W/carded" --yes
  rc_is 0
  check "the card is written through a path with a backslash" command grep -qx 'The card.' "$CONF/AGENTS.md"
  run verify
  rc_is 0
}

t_lock_newer() {
  case_ "a newer lock is refused"
  mkdir -p "$CONF"
  printf 'lock/2\tx\ton\n' >"$CONF/mods.lock"
  local v
  for v in list lock verify "on x" "off x" "rm x"; do
    # shellcheck disable=SC2086 # the verb and its argument
    run $v
    refused "this lock is newer than this henki-mods; fix: upgrade henki"
  done
  mkmod "$W/hello"
  run add "$W/hello" --yes
  refused "this lock is newer"
  check "the lock is untouched" [ "$(cat "$CONF/mods.lock")" = "$(printf 'lock/2\tx\ton')" ]
  printf 'lock/1\tx\ton\n' >"$CONF/mods.lock"
  run list
  refused "not a lock/1 line"
}

t_onoff() {
  case_ "on, off, rm flip links and lock lines"
  mkmod "$W/hello"
  mkmod "$W/bye"
  run add "$W/hello" --yes
  run add "$W/bye" --yes
  local store
  store=$(readlink "$CONF/ext/hello")
  check "the lock is sorted by name" [ "$(cut -f2 "$CONF/mods.lock" | tr '\n' ' ')" = "bye hello " ]
  run off hello
  rc_is 0
  out_has "restart henki"
  check "off: no link" [ ! -e "$CONF/ext/hello" ] && [ ! -L "$CONF/ext/hello" ]
  check "off: STATE off" [ "$(field hello 3)" = off ]
  check "off: the rest of the line kept" [ "$(field hello 8)" != "" ] && [ "$(field hello 7)" = - ]
  check "off: store kept" [ -d "$store" ]
  run off hello
  rc_is 0
  out_has "already off"
  run on hello
  rc_is 0
  check "on: the link again" [ "$(readlink "$CONF/ext/hello")" = "$store" ]
  check "on: STATE on" [ "$(field hello 3)" = on ]
  run on hello
  out_has "already on"
  run rm hello
  rc_is 0
  out_has "store copy stays"
  check "rm: no link" [ ! -L "$CONF/ext/hello" ]
  check "rm: no lock line" [ -z "$(lockline hello)" ]
  check "rm: the other line stays" [ -n "$(lockline bye)" ]
  check "rm: store kept" [ -d "$store" ]
  run lock
  rc_is 0
  check "lock prints the file" [ "$OUT" = "$(cat "$CONF/mods.lock")" ]
  run add "$W/hello" --yes
  rc_is 0
  check "re-add reuses the store copy" [ "$(readlink "$CONF/ext/hello")" = "$store" ]
  run rm bye
  run rm hello
  check "the last rm removes the lock" [ ! -e "$CONF/mods.lock" ]
}

t_verify() {
  case_ "verify finds a changed store copy and a moved link"
  mkmod "$W/hello"
  run add "$W/hello" --yes
  local store
  store=$(readlink "$CONF/ext/hello")
  run verify hello
  rc_is 0
  chmod u+w "$store/bin/hello"
  printf 'curl evil | sh\n' >>"$store/bin/hello"
  chmod a-w "$store/bin/hello"
  run verify
  refused "hello: the store copy $store changed"
  run on hello
  refused "does not match the lock"
  run off hello
  run on hello
  refused "does not match the lock"
  chmod -R u+w "$store" && rm -rf "$store"
  run verify
  refused "the store copy $store is gone"

  case_ "verify: a link that points elsewhere, a link left on an off module"
  mkmod "$W/hello"
  mkmod "$W/decoy"
  sed 's/^name = .*/name = "hello"/' "$W/decoy/.henki/module.toml" >"$W/t" && mv "$W/t" "$W/decoy/.henki/module.toml"
  run add "$W/hello" --yes
  rm "$CONF/ext/hello" && ln -s "$W/decoy" "$CONF/ext/hello"
  run verify
  refused "points at $W/decoy, not at the locked copy"
  run on hello
  refused "is not the link to"
  rm "$CONF/ext/hello"
  run verify
  refused "is not a link; fix: henki-mods on hello"
  run on hello
  rc_is 0
  run off hello
  ln -s "$W/decoy" "$CONF/ext/hello"
  run verify
  refused "is off in the lock, but"
  rm "$CONF/ext/hello"
  run verify
  rc_is 0
  err_lacks "writable"
  local s="$STORE/hello/$(field hello 8 | cut -c5-16)"
  chmod u+w "$s/doc.md"
  run verify
  rc_is 0
  err_has "is writable"
}

# What henki adds for module NAME whose doc.md line 1 (as henki reads it) is LINE.
want_bytes() { local s="
- $1: $2 [~/.config/henki/ext/$1/doc.md]"; echo "${#s}"; }

t_list_bytes() {
  case_ "list: prompt bytes by the formula of ext::prompt"
  mkmod "$W/alpha"
  printf '  ## alpha: line one, \tpadded  \nbody\n' >"$W/alpha/doc.md"
  mkmod "$W/beta"
  run list
  rc_is 0
  out_has "on: nothing (0 B of prompt)"
  run add "$W/alpha" --yes
  run add "$W/beta" --yes
  local a b header=77
  a=$(want_bytes alpha "alpha: line one, $(printf '\t')padded")
  b=$(want_bytes beta "beta: says hello for tests")
  run list
  rc_is 0
  out_has "alpha            on    0.1.0      $a B"
  out_has "on: $((header + a + b)) B of prompt (extensions $((header + a + b)) B with the 77 B header, cards 0 B)"
  run off beta
  run list
  out_has "beta             off   0.1.0      $b B"
  out_has "on: $((header + a)) B of prompt"
  # A hand-made extension in the ext dir counts too: henki loads it.
  mkdir -p "$CONF/ext/manual"
  printf 'manual one\n' >"$CONF/ext/manual/doc.md"
  run list
  out_has "manual           on    -          $(want_bytes manual 'manual one') B  not added by henki-mods"
  out_has "on: $((header + a + $(want_bytes manual 'manual one'))) B of prompt"
  # The screen of the next add says what it adds and the total after.
  run on beta
  rm -rf "$CONF/ext/manual"
  mkmod "$W/gamma"
  run add "$W/gamma" --yes
  out_has "prompt   +$(want_bytes gamma 'gamma: says hello for tests') B on every request (total on after: $((header + a + b + $(want_bytes gamma 'gamma: says hello for tests'))) B)"
}

t_cards() {
  case_ "cards: the block in AGENTS.md"
  mkdir -p "$CONF"
  printf '# my rules\n\nBe brief.\n' >"$CONF/AGENTS.md"
  cp "$CONF/AGENTS.md" "$W/original"
  mkmod "$W/one"
  printf 'Card one.\n' >"$W/one/.henki/card.md"
  mkmod "$W/two"
  printf 'Card two, no newline' >"$W/two/.henki/card.md"
  mkmod "$W/plain"
  run add "$W/two" --yes
  rc_is 0
  out_has "card     +20 B"
  run add "$W/one" --yes
  run add "$W/plain" --yes
  check "the block after the user's text" [ "$(cat "$CONF/AGENTS.md")" = "$(printf '# my rules\n\nBe brief.\n\n# >>> mods cards\nCard one.\n\nCard two, no newline\n# <<< mods cards')" ]
  run list
  out_has "card 10 B"
  local with=$(($(printf '# my rules\n\nBe brief.\n\n# >>> mods cards\nCard one.\n\nCard two, no newline\n# <<< mods cards' | wc -c)))
  local without=$(($(printf '# my rules\n\nBe brief.' | wc -c)))
  out_has "cards $((with - without)) B)"
  run off one
  check "off removes its card" [ "$(cat "$CONF/AGENTS.md")" = "$(printf '# my rules\n\nBe brief.\n\n# >>> mods cards\nCard two, no newline\n# <<< mods cards')" ]
  run verify
  rc_is 0
  # A block edited by hand is found, and the next change writes it again.
  printf 'extra\n' >>"$CONF/AGENTS.md"
  sed 's/^Card two, no newline$/Card two, edited/' "$CONF/AGENTS.md" >"$W/t" && cat "$W/t" >"$CONF/AGENTS.md"
  run verify
  refused "is not the cards of the modules on"
  run on one
  run verify
  rc_is 0
  check "text after the block stays" [ "$(tail -n 1 "$CONF/AGENTS.md")" = extra ]
  printf '# my rules\n\nBe brief.\n\n# >>> mods cards\nCard one.\n\nCard two, no newline\n# <<< mods cards\n' >"$CONF/AGENTS.md"
  run rm one
  run rm two
  check "the user's file comes back byte for byte" cmp -s "$CONF/AGENTS.md" "$W/original"

  case_ "cards: an AGENTS.md made only for cards goes with the last card"
  mkmod "$W/one"
  printf 'Card one.\n' >"$W/one/.henki/card.md"
  run add "$W/one" --yes
  check "AGENTS.md is the block" [ "$(cat "$CONF/AGENTS.md")" = "$(printf '# >>> mods cards\nCard one.\n# <<< mods cards')" ]
  run off one
  check "and goes" [ ! -e "$CONF/AGENTS.md" ]

  case_ "cards: the 4 KiB limit"
  mkmod "$W/big"
  awk 'BEGIN { for (i = 0; i < 4097; i++) printf "x" }' >"$W/big/.henki/card.md"
  run add "$W/big" --yes
  refused "4,097 bytes, over henki's AGENTS.md limit of 4,096"
  mkdir -p "$CONF"
  awk 'BEGIN { for (i = 0; i < 40; i++) { for (j = 0; j < 99; j++) printf "y"; print "" } }' >"$CONF/AGENTS.md"
  awk 'BEGIN { for (i = 0; i < 99; i++) printf "z"; print "" }' >"$W/big/.henki/card.md"
  run add "$W/big" --yes
  refused "the cards would make ~/.config/henki/AGENTS.md 4,135 bytes, over henki's limit of 4,096"
  check "AGENTS.md untouched" [ "$(wc -c <"$CONF/AGENTS.md" | tr -d ' ')" = 4000 ]
  check "nothing installed" [ ! -e "$CONF/mods.lock" ]
  printf 'zzz\n' >"$W/big/.henki/card.md"
  run add "$W/big" --yes
  rc_is 0
  check "under the limit is fine" [ "$(wc -c <"$CONF/AGENTS.md" | tr -d ' ')" = 4039 ]
  run verify
  rc_is 0
  printf '# >>> mods cards\n' >>"$CONF/AGENTS.md"
  run off big
  refused "a second mods cards block"
  mkmod "$W/bad"
  printf 'one\n# <<< mods cards\n' >"$W/bad/.henki/card.md"
  run verify --dir "$W/bad"
  refused "holds a mods cards marker line"
  printf 'has a CR\r\n' >"$W/bad/.henki/card.md"
  run verify --dir "$W/bad"
  refused "card.md: holds U+000D"
}

t_shadow() {
  case_ "shadow: the line to add, and verify warns while it is missing"
  local bin="$W/userbin"
  mkdir -p "$bin"
  printf '#!/bin/sh\n' >"$bin/pbtool" && chmod 755 "$bin/pbtool"
  mkmod "$W/wrap" 'shadow = ["wrap/other"]\n'
  printf '#!/bin/sh\n' >"$W/wrap/bin/pbtool" && chmod 755 "$W/wrap/bin/pbtool"
  mkdir -p "$CONF"
  printf 'model = "m"\nshadow = ["halti/halti"]  # kept\n' >"$CONF/config.toml"
  PATH="$bin:$PATH" run add "$W/wrap" --yes
  rc_is 0
  out_has 'shadows  wrap/other wrap/pbtool'
  out_has 'shadow = ["halti/halti", "wrap/other", "wrap/pbtool"]'
  PATH="$bin:$PATH" run verify
  rc_is 0
  err_has 'warning: wrap: henki does not start until ~/.config/henki/config.toml has: shadow = ['
  # Several entries, one still missing, under an awk that refuses a newline in a -v value as macOS's does.
  local shim="$W/bwk" awk_bin
  awk_bin=$(command -v awk)
  mkdir -p "$shim"
  printf '#!/bin/sh\nfor a in "$@"; do case "$a" in *"\n"*) [ "$v" = 1 ] && echo "awk: newline in string" >&2 && exit 2 ;; esac; v=0; [ "$a" = -v ] && v=1; done\nexec %s "$@"\n' "$awk_bin" >"$shim/awk"
  chmod 755 "$shim/awk"
  printf 'model = "m"\nshadow = ["halti/halti", "wrap/other"]\n' >"$CONF/config.toml"
  PATH="$shim:$bin:$PATH" run verify
  rc_is 0
  err_has 'warning: wrap: henki does not start until ~/.config/henki/config.toml has: shadow = ["halti/halti", "wrap/other", "wrap/pbtool"]'
  err_lacks "newline in string"
  printf 'model = "m"\nshadow = ["halti/halti", "wrap/other", "wrap/pbtool"]\n' >"$CONF/config.toml"
  PATH="$shim:$bin:$PATH" run verify
  rc_is 0
  err_lacks "warning"
  err_lacks "newline in string"
  printf 'ext = ["/somewhere/else"]\n' >>"$CONF/config.toml"
  run list
  err_has "sets ext without ~/.config/henki/ext"
}

t_official() {
  case_ "official modules, --version"
  local share="$W/share" bin="$W/bin"
  mkmod "$share/official/halti"
  mkmod "$share/official/itse"
  HENKI_SHARE="$share" run list
  out_has "official, not added: halti itse"
  HENKI_SHARE="$share" run add halti --yes
  rc_is 0
  out_has "source   official ($share/official/halti)"
  check "KIND official, SOURCE the name" [ "$(field halti 5):$(field halti 6)" = official:halti ]
  HENKI_SHARE="$share" run list
  out_has "official, not added: itse"
  run add nothere --yes
  refused "no official module nothere"
  # Shipped next to a henki: its version, and the tarball's share/ dir without HENKI_SHARE.
  mkdir -p "$bin/bin" "$bin/share/henki"
  cp "$MODS" "$bin/bin/henki-mods"
  cp -R "$share/official" "$bin/share/henki/official"
  printf '#!/bin/sh\necho "henki 0.1.0 (abc, x86_64-linux)"\n' >"$bin/bin/henki"
  chmod 755 "$bin/bin/henki"
  RC=0
  OUT=$($RUN "$bin/bin/henki-mods" --version 2>&1) || RC=$?
  check "--version from the henki next to it" [ "$OUT" = "henki-mods 0.1.0" ]
  run --version
  check "--version without one" [ "$OUT" = "henki-mods dev" ]
  ln -s "$bin/bin/henki-mods" "$W/link-to-mods"
  OUT=$($RUN "$W/link-to-mods" add itse --yes 2>&1) || RC=$?
  rc_is 0
  check "found through a link" [ "$(field itse 5)" = official ]
  mkmod "$W/future" 'requires = ["henki >= 0.2"]\n'
  RC=0
  ERR=$($RUN "$bin/bin/henki-mods" add "$W/future" --yes 2>&1 >/dev/null) || RC=$?
  refused "future needs henki >= 0.2, and this henki is 0.1.0"
  rm -rf "$W/future" && mkmod "$W/future" 'requires = ["henki >= 0.1"]\n'
  RC=0
  $RUN "$bin/bin/henki-mods" add "$W/future" --yes >/dev/null 2>&1 || RC=$?
  rc_is 0
}

# The official halti module from this checkout: its install screen, its card, and its wrappers, which refuse what
# hands out secret values and run the user's own command for the rest. USER holds stand-ins for halti, pbpaste and
# security, as on a Mac.
t_halti() {
  case_ "the official halti module and its wrappers"
  local user="$W/user" b="$CONF/ext/halti/bin" v line card
  mkdir -p "$user"
  for v in halti pbpaste security; do
    printf '#!/bin/sh\necho "real %s: $*"\n' "$v" >"$user/$v"
    chmod 755 "$user/$v"
  done
  line='shadow = ["halti/halti", "halti/pbpaste", "halti/security"]'
  PATH="$user:$PATH" run add halti --yes
  rc_is 0
  out_has "source   official ($(cd "$HERE/../mods/halti" && pwd -P))"
  out_has "license  Apache-2.0"
  out_has "bin/halti (sh, "
  out_has "bin/pbpaste (sh, "
  out_has "bin/security (sh, "
  out_has "needs    halti (found)"
  out_has "config   henki starts only once ~/.config/henki/config.toml has: $line"
  out_has "add to ~/.config/henki/config.toml: $line"
  card="$HERE/../mods/halti/.henki/card.md"
  out_has "card     +$(num "$(wc -c <"$card" | tr -d ' ')") B"
  check "AGENTS.md is the card in its block" [ "$(cat "$CONF/AGENTS.md")" = "$(printf '# >>> mods cards\n%s\n# <<< mods cards' "$(cat "$card")")" ]
  run verify
  rc_is 0
  err_has "$line"
  # The wrappers, with HENKI_USER_PATH as henki sets it for a call.
  wrap() {
    RC=0
    OUT=$(HENKI_USER_PATH="$user:/usr/bin:/bin" PATH="$b:$user:/usr/bin:/bin" "$@" 2>"$SCRATCH/err") || RC=$?
    ERR=$(cat "$SCRATCH/err")
  }
  for v in set with copy update rotate; do
    wrap halti "$v" own/github
    rc_is 126
    err_has "fix: ask the user to run !halti $v"
  done
  wrap halti --version
  check "halti --version runs the real halti" [ "$OUT" = "real halti: --version" ]
  wrap halti
  check "bare halti runs the real halti" [ "$OUT" = "real halti: " ]
  wrap halti ls own
  check "halti ls runs the real halti" [ "$OUT" = "real halti: ls own" ]
  for v in "-s github" "--secret github" "-sgithub" "--secret=github"; do
    # shellcheck disable=SC2086 # two words on purpose
    wrap halti run $v -- gh api user
    rc_is 126
    err_has "'halti run -s' puts secret values"
  done
  wrap halti sh own -- grep -s x
  check "-s after -- is the command's" [ "$OUT" = "real halti: sh own -- grep -s x" ]
  wrap halti run own -- echo -s
  check "-s after -- in halti run is the command's" [ "$OUT" = "real halti: run own -- echo -s" ]
  wrap pbpaste
  rc_is 126
  err_has "pbpaste reads the clipboard"
  for v in "find-generic-password -s x -w" "-q find-internet-password -s x" "-p prompt dump-keychain -d" "export -k login" \
    "set-key-partition-list -S apple:" "add-generic-password -a a -s s -w pw" "unlock-keychain -p pw" "-i" "-qi" ""; do
    # shellcheck disable=SC2086 # words on purpose
    wrap security $v
    rc_is 126
    err_has "henki: "
    check "security $v runs nothing" [ -z "$OUT" ]
  done
  wrap security -p find-generic-password list-keychains
  check "-p takes the next word" [ "$OUT" = "real security: -p find-generic-password list-keychains" ]
  wrap security -h
  check "security -h runs the real one" [ "$OUT" = "real security: -h" ]
  wrap security find-certificate -a -c name
  check "find-certificate runs the real one" [ "$OUT" = "real security: find-certificate -a -c name" ]
  # henki sets HENKI_USER_PATH even when empty: then there is no real command, and the wrapper says so.
  RC=0
  ERR=$(HENKI_USER_PATH="" PATH="$b:$user:/usr/bin:/bin" halti ls 2>&1 >/dev/null) || RC=$?
  rc_is 127
  err_has "halti is not installed on the user's PATH"
  # Run outside henki: the PATH without the wrapper's own dir, by the link and by the store copy alike.
  for v in "$b" "$(cd "$b" && pwd -P)"; do
    RC=0
    OUT=$(env -u HENKI_USER_PATH PATH="$v:$user:/usr/bin:/bin" halti ls 2>&1) || RC=$?
    check "outside henki, $v: the real halti" [ "$OUT" = "real halti: ls" ]
    OUT=$(env -u HENKI_USER_PATH PATH="$v:/usr/bin:/bin" halti ls 2>&1) || RC=$?
    rc_is 127
  done
}

# A real henki with the halti module on and a fresh HOME: it refuses to start without the shadow line the install
# screen prints, and starts with the Mac profile, which has it; the doc line and the card reach the model.
t_halti_live() {
  case_ "henki starts with the official halti module (henki run --json)"
  live_ok || return
  local d="$W/stub" user="$W/user" work="$W/work" v
  mkdir -p "$user" "$work" "$CONF"
  for v in halti pbpaste security; do
    printf '#!/bin/sh\necho "real %s: $*"\n' "$v" >"$user/$v"
    chmod 755 "$user/$v"
  done
  stub_up "$d" || return
  printf 'model = "m"\nbase_url = "http://127.0.0.1:%s/v1"\n' "$STUB_PORT" >"$CONF/config.toml"
  chmod 600 "$CONF/config.toml"
  PATH="$user:$PATH" run add halti --yes
  rc_is 0
  henki_run() { (cd "$work" && PATH="$user:$PATH" env -u XDG_CONFIG_HOME "$HENKI_BIN" run --json hi >"$d/run.out" 2>&1); }
  RC=0
  henki_run || RC=$?
  check "henki refuses without the shadow line" [ "$RC" != 0 ]
  check "henki says what shadows what: $(cat "$d/run.out")" command grep -qF 'bin/halti shadows' "$d/run.out"
  check "henki says which line to add" command grep -qF 'fix: add shadow = [' "$d/run.out"
  check "no request without it" [ ! -e "$d/system" ]
  sed "s|^base_url = .*|base_url = \"http://127.0.0.1:$STUB_PORT/v1\"|" "$HERE/../profiles/config.mac.toml" >"$CONF/config.toml"
  chmod 600 "$CONF/config.toml"
  henki_run || fail "henki run with the Mac profile failed: $(cat "$d/run.out")"
  check "the doc line reaches the model" command grep -qF -e "- halti: $(sed -n '1s/^# //p' "$HERE/../mods/halti/doc.md") [~/.config/henki/ext/halti/doc.md]" "$d/system"
  check "the card reaches the model" command grep -qF -e "$(sed -n 1p "$HERE/../mods/halti/.henki/card.md")" "$d/system"
  run verify
  rc_is 0
  err_lacks "shadow"
  kill "$STUB_PID" 2>/dev/null
  wait "$STUB_PID" 2>/dev/null
}

# A stub model server for the live checks in D: it answers every request "ok" and appends the system prompt's length
# in bytes to D/sizes, and writes the prompt itself to D/system. Sets STUB_PORT and STUB_PID; exit 1 when it does not
# start.
stub_up() {
  local d="$1"
  mkdir -p "$d"
  cat >"$d/stub.py" <<'PY'
import http.server, json, os, socketserver, sys
d = sys.argv[1]
def chunk(delta, finish=None):
    return "data: " + json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "object": "chat.completion.chunk"}) + "\n\n"
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        system = [m["content"] for m in body["messages"] if m["role"] == "system"][0]
        with open(os.path.join(d, "sizes"), "a") as f:
            f.write("%d\n" % len(system.encode()))
        with open(os.path.join(d, "system"), "w") as f:
            f.write(system)
        sse = (chunk({"role": "assistant", "content": "ok"}) + chunk({}, "stop") + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(sse)))
        self.end_headers()
        self.wfile.write(sse)
    def log_message(self, *a):
        pass
s = socketserver.TCPServer(("127.0.0.1", 0), H)
open(os.path.join(d, "port"), "w").write(str(s.server_address[1]))
s.serve_forever()
PY
  stub_start "$d"
}

# Starts DIR/stub.py, which writes the port it listens on into DIR/port; STUB_PID and STUB_PORT.
stub_start() {
  local d="$1" i=0
  python3 -I "$d/stub.py" "$d" &
  STUB_PID=$!
  while [ ! -s "$d/port" ] && [ $i -lt 100 ]; do
    sleep 0.1 2>/dev/null || sleep 1
    i=$((i + 1))
  done
  STUB_PORT=$(cat "$d/port" 2>/dev/null)
  [ -n "$STUB_PORT" ] && return 0
  kill "$STUB_PID" 2>/dev/null
  fail "the stub server did not start"
  return 1
}

# Whether the live checks can run; says why not.
live_ok() {
  if [ -z "$HENKI_BIN" ]; then
    echo "tests.sh: the live check \"$CASE\" was SKIPPED (set MODS_TEST_HENKI to a henki binary)" >&2
    return 1
  fi
  command -v python3 >/dev/null 2>&1 && return 0
  fail "python3 is not on PATH, and the live check needs it for its stub server"
  return 1
}

# The live check: the system prompt of real henki requests, without modules and with them, sent to a stub that
# records its length in bytes.
t_live() {
  case_ "list's bytes are what henki really adds (henki run --json)"
  live_ok || return
  local d="$W/stub" base with work="$W/work"
  mkdir -p "$work" "$CONF"
  stub_up "$d" || return
  printf 'model = "m"\nbase_url = "http://127.0.0.1:%s/v1"\n' "$STUB_PORT" >"$CONF/config.toml"
  chmod 600 "$CONF/config.toml"
  live_run() { (cd "$work" && env -u XDG_CONFIG_HOME "$HENKI_BIN" run --json hi >"$d/run.out" 2>&1) || fail "henki run failed: $(cat "$d/run.out")"; }
  live_run
  base=$(tail -n 1 "$d/sizes")
  mkmod "$W/alpha"
  printf '## alpha: one line\n' >"$W/alpha/doc.md"
  mkmod "$W/beta"
  printf 'A card for beta.\n\nTwo paragraphs.\n' >"$W/beta/.henki/card.md"
  run add "$W/alpha" --yes
  run add "$W/beta" --yes
  run list
  live_run
  with=$(tail -n 1 "$d/sizes")
  out_has "on: $(num $((with - base))) B of prompt"
  run off beta
  run list
  live_run
  with=$(tail -n 1 "$d/sizes")
  out_has "on: $(num $((with - base))) B of prompt"
  kill "$STUB_PID" 2>/dev/null
  wait "$STUB_PID" 2>/dev/null
}

# ext/2: user commands in cmd/ (with their .md data), eval/, the whole-doc text rules, links inside the module, and the
# prompt rule (a module of commands without bin/ costs no prompt bytes), as henki's ext.rs loads them.
t_ext2() {
  case_ "ext/2: cmd/, eval/, texts, links, prompt bytes"
  local m="$W/kit"
  mkmod "$m"
  rm -rf "$m/bin"
  mkdir -p "$m/cmd"
  printf '#!/bin/sh\nexec "$HENKI_MODULE/.henki/template" "$@"\n' >"$m/cmd/go"
  printf '#!/bin/sh\necho template\n' >"$m/.henki/template"
  chmod 755 "$m/cmd/go" "$m/.henki/template"
  ln -s ../.henki/template "$m/cmd/review"
  printf 'Review the change.\n' >"$m/cmd/review.md"
  printf 'x\n' >"$m/cmd/.DS_Store"
  mktask "$m/eval" task-1 "a task" 'true'
  run verify --dir "$m"
  rc_is 0
  out_has "0 B of prompt"
  run add "$m" --yes
  rc_is 0
  out_has "prompt   +0 B on every request (total on after: 0 B)"
  run list
  out_has "on: 1 module(s), 0 B of prompt"
  # With bin/ the doc is for the model again.
  mkmod "$W/both"
  mkdir -p "$W/both/cmd" && cp "$m/cmd/go" "$W/both/cmd/go"
  run verify --dir "$W/both"
  out_lacks " 0 B of prompt"
  for bad in mods exit help allow; do
    rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/cmd" && cp "$m/cmd/go" "$W/r/cmd/$bad"
    run verify --dir "$W/r"
    refused "cmd/$bad: a name henki keeps for its own command"
  done
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/cmd" && printf 'x\n' >"$W/r/cmd/notes.txt"
  run verify --dir "$W/r"
  refused "cmd/notes.txt: a command is a file you can run"
  printf 'Review \342\200\256 this.\n' >"$W/r/cmd/notes.md" && rm "$W/r/cmd/notes.txt"
  run verify --dir "$W/r"
  refused "cmd/notes.md: holds U+202E, which a terminal does not show"
  head -c 16385 /dev/zero | tr '\0' x >"$W/r/cmd/notes.md"
  run verify --dir "$W/r"
  refused "cmd/notes.md: 16,385 bytes, over 16,384"
  rm -rf "$W/r" && mkmod "$W/r"
  printf 'r: a summary\n\nbody \342\200\256 here\n' >"$W/r/doc.md"
  run verify --dir "$W/r"
  refused "doc.md: holds U+202E"
  printf 'r: a summary\r\n' >"$W/r/doc.md"
  run verify --dir "$W/r"
  refused "doc.md: holds U+000D"
  { printf 'r: a summary\n'; head -c 16384 /dev/zero | tr '\0' x; } >"$W/r/doc.md"
  run verify --dir "$W/r"
  refused "doc.md: 16,397 bytes, over 16,384"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/cmd"
  printf '#!/bin/sh\n' >"$W/elsewhere" && chmod 755 "$W/elsewhere"
  ln -s ../../elsewhere "$W/r/cmd/out"
  run verify --dir "$W/r"
  refused "leaves the module"
}

# on/: observers named for an event (a waited one for turn_start and call_start), runnable, links inside; a module of
# observers costs no prompt bytes.
t_observe() {
  case_ "observe/1: on/ names, modes, prompt bytes"
  local m="$W/watch" bad
  mkmod "$m" 'contracts = ["ext/2", "observe/1"]\n'
  rm -rf "$m/bin"
  mkdir -p "$m/on"
  printf '#!/bin/sh\ncat >/dev/null\n' >"$m/on/turn_end"
  printf '#!/bin/sh\n' >"$m/on/call_start.wait"
  chmod 755 "$m/on/turn_end" "$m/on/call_start.wait"
  ln -s turn_end "$m/on/question"
  printf 'x\n' >"$m/on/.DS_Store"
  run verify --dir "$m"
  rc_is 0
  out_has "0 B of prompt"
  printf '#!/bin/sh\necho hi\n' >"$m/.henki/template" && chmod 755 "$m/.henki/template"
  mkdir -p "$m/cmd" "$m/port" && ln -s ../.henki/template "$m/cmd/hi"
  printf '#!/bin/sh\n' >"$m/port/policy" && chmod 755 "$m/port/policy"
  printf 'ports = ["policy"]\n' >>"$m/.henki/module.toml"
  run add "$m" --yes
  rc_is 0
  out_has "prompt   +0 B on every request"
  # The screen names every program it may run, an observer as run unasked, and no module looks as if it ran nothing.
  out_has "  files    "
  out_has "  runs as you, unsandboxed:$nl    cmd/hi -> ../.henki/template (sh, 2 lines): when you type /hi$nl"
  out_has "    on/call_start.wait (sh, 1 lines): unasked, at every call_start event; henki waits for it, up to 2 s$nl"
  out_has "    on/question -> turn_end (sh, 2 lines): unasked, at every question event$nl"
  out_has "    on/turn_end (sh, 2 lines): unasked, at every turn_end event$nl"
  out_has "    port/policy (sh, 1 lines): as henki's policy port, once henki-mods swap policy binds it$nl"
  out_lacks ".DS_Store"
  for bad in call_end.wait turn_end.wait stop .hidden; do
    rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/on" && printf '#!/bin/sh\n' >"$W/r/on/$bad" && chmod 755 "$W/r/on/$bad"
    run verify --dir "$W/r"
    refused "on/$bad: not an event; on/ holds session_start turn_start turn_start.wait call_start call_start.wait call_end turn_end question session_end record"
  done
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/on" && printf '#!/bin/sh\n' >"$W/r/on/record"
  run verify --dir "$W/r"
  refused "on/record: an observer is a file you can run"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/on/sub"
  run verify --dir "$W/r"
  refused "on/sub: a directory"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/on"
  ln -s ../../elsewhere "$W/r/on/record"
  run verify --dir "$W/r"
  refused "leaves the module"
}

# After a change the manager writes `restart` to the hand-off file a henki gives it (/mods), and prints the restart
# line without one; listing writes nothing.
t_restart() {
  case_ "restart to HENKI_NEXT"
  local next="$W/next"
  mkmod "$W/one"
  : >"$next"
  HENKI_NEXT=$next run add "$W/one" --yes
  rc_is 0
  out_lacks "restart henki"
  check "add hands back restart" [ "$(cat "$next")" = restart ]
  for verb in "off one" "on one" "rm one"; do
    : >"$next"
    HENKI_NEXT=$next run $verb
    rc_is 0
    check "$verb hands back restart" [ "$(cat "$next")" = restart ]
  done
  : >"$next"
  HENKI_NEXT=$next run list
  check "list hands back nothing" [ ! -s "$next" ]
  printf '/src\tmake\n' >"$CONF/allow"
  HENKI_NEXT=$next run allow rm 1
  rc_is 0
  check "allow rm hands back restart" [ "$(cat "$next")" = restart ]
  run allow
  check "allow lists, nothing more" [ "$OUT" = "$CONF/allow" ]
  mkmod "$W/two"
  ln -s "$W/elsewhere" "$W/link"
  HENKI_NEXT=$W/link run add "$W/two" --yes
  out_has "restart henki (or /exit and start again) to load the change"
  check "a link is not written through" [ ! -e "$W/elsewhere" ]
  run off two
  out_has "restart henki (or /exit and start again) to load the change"
}

# henki's allow file, as henki listed and removed rules before the manager did.
# FILE holds exactly the bytes of the printf FORMAT (for \t, \r, \033 and \377); WANT is a file of the same.
bytes_are() { # DESCRIPTION FILE FORMAT
  printf "$3" >"$SCRATCH/want"
  cmp -s "$2" "$SCRATCH/want" || fail "$1: $(od -c <"$2" | head -n 5)"
}

t_allow() {
  case_ "allow: list and rm"
  local f="$CONF/allow"
  mkdir -p "$CONF"
  printf '# my rules\n/src/app\tgit status\n\n/src/app\tmake test\n%s/proj\tls -la\n%s\tpwd\n*\tmake\n' "$HOME" "$HOME" >"$f"
  chmod 600 "$f"
  run allow
  rc_is 0
  check "the list" [ "$OUT" = "$f${nl}1  /src/app  git status${nl}2  /src/app  make test${nl}3  ~/proj  ls -la${nl}4  ~  pwd${nl}5  *  make" ]
  run allow rm 1
  rc_is 0
  check "the removed rule" [ "$OUT" = "removed rule 1: /src/app  git status" ]
  bytes_are "everything else byte for byte" "$f" "# my rules\n\n/src/app\tmake test\n$HOME/proj\tls -la\n$HOME\tpwd\n*\tmake\n"
  check "0600" [ "$(ls -l "$f" | cut -c1-10)" = "-rw-------" ]
  check "no temp file left" [ "$(ls -A "$CONF")" = allow ]
  run allow rm 004
  rc_is 0
  check "leading zeros" [ "$OUT" = "removed rule 4: *  make" ]
  rm "$f"
  run allow
  rc_is 0
  check "no file: the path alone" [ "$OUT" = "$f" ]
  run allow rm 1
  refused "no rule 1 in $f"

  case_ "allow: invalid lines are listed, numbered and removable, in any mode"
  f="$CONF/allow"
  mkdir -p "$CONF"
  printf '# c\n/a\tok1\nbad line\n\n# c2\n/a\033[31m\tls\n/b\377\tls\r\n/a\tls \303\244\n/a\342\200\213b\tls\n/a\tok2' >"$f"
  chmod 666 "$f"
  run allow
  rc_is 0
  check "invalid lines, escaped" [ "$OUT" = "$f${nl}1  /a  ok1${nl}2  !bad line  (invalid: no TAB between ROOT and COMMAND)${nl}3  !/a\\x1b[31m${tab}ls  (invalid: ROOT is not * or an absolute path)${nl}4  !/b$(printf '\357\277\275')${tab}ls  (invalid: ROOT is not * or an absolute path)${nl}5  !/a${tab}ls $(printf '\303\244')  (invalid: COMMAND is not one printable-ASCII line of 1 to 200 bytes)${nl}6  !/a\\u{200b}b${tab}ls  (invalid: ROOT is not * or an absolute path)${nl}7  /a  ok2" ]
  run allow rm 2
  rc_is 0
  check "an invalid rule goes" [ "$OUT" = "removed rule 2: !bad line  (invalid: no TAB between ROOT and COMMAND)" ]
  run allow rm 3
  rc_is 0
  bytes_are "the CR goes with its line" "$f" '# c\n/a\tok1\n\n# c2\n/a\033[31m\tls\n/a\tls \303\244\n/a\342\200\213b\tls\n/a\tok2'
  run allow rm 5
  rc_is 0
  bytes_are "a last line without a newline takes the one before it" "$f" '# c\n/a\tok1\n\n# c2\n/a\033[31m\tls\n/a\tls \303\244\n/a\342\200\213b\tls'
  printf '/a\tone' >"$f"
  run allow rm 1
  rc_is 0
  bytes_are "the only line" "$f" ''

  case_ "allow: refusals and usage"
  f="$CONF/allow"
  mkdir -p "$CONF"
  printf '/src/app\tgit status\n' >"$f"
  for n in 9 0 000; do
    run allow rm "$n"
    refused "no rule $((10#$n)) in $f"
  done
  for args in "rm x" "rm" "rm -1" "rm 1 2" "clear" "--config"; do
    run allow $args
    rc_is 2
    err_has "usage: henki-mods"
  done
  bytes_are "nothing removed" "$f" '/src/app\tgit status\n'
  mkfifo "$W/fifo"
  mv "$W/fifo" "$f"
  run allow
  rc_is 0
  check "a FIFO is named, never opened" [ "$OUT" = "$f${nl}cannot read it: not a regular file" ]
  run allow rm 1
  refused "$f: not a regular file"
  rm "$f"
  awk 'BEGIN { for (i = 0; i < 6554; i++) print "#234567890" }' >"$f"
  run allow
  check "over 64 KiB" [ "$OUT" = "$f${nl}cannot read it: over 65,536 bytes" ]
  HOME="" run allow
  refused "HOME is not set"

  case_ "allow: --config FILE, HENKI_CONFIG, a linked config dir"
  mkdir -p "$CONF" "$W/alt" "$W/real"
  printf '/src/app\techo home\n' >"$CONF/allow"
  printf '/src/app\techo alt\n' >"$W/alt/allow"
  run allow --config "$W/alt/henki.toml"
  check "beside --config" [ "$OUT" = "$W/alt/allow${nl}1  /src/app  echo alt" ]
  OUT=$(cd "$W" && $RUN "$MODS" allow --config alt/henki.toml rm 1 2>&1) || fail "relative --config"
  check "relative --config, from the working directory" [ "$OUT" = "removed rule 1: /src/app  echo alt" ]
  bytes_are "removed there" "$W/alt/allow" ''
  bytes_are "the default file untouched" "$CONF/allow" '/src/app\techo home\n'
  HENKI_CONFIG="$W/alt" run allow
  check "HENKI_CONFIG names the config dir" [ "$OUT" = "$W/alt/allow" ]
  HENKI_CONFIG="alt" run allow
  refused "HENKI_CONFIG is not an absolute path"
  printf '/x\tone\n/x\ttwo\n' >"$W/real/allow"
  ln -s "$W/real" "$W/linked"
  HENKI_CONFIG="$W/linked" run allow rm 1
  rc_is 0
  check "the link stays" [ -L "$W/linked" ]
  bytes_are "the file behind it" "$W/real/allow" '/x\ttwo\n'
  check "no temp file left" [ "$(ls -A "$W/real")" = allow ]
}

# A module NAME in W that binds history: the bash binding of the conformance suite as port/history.
mkport() {
  mkmod "$W/$1" 'ports = ["history"]\ncontracts = ["history/1"]\n'
  mkdir -p "$W/$1/port"
  cp "$HERE/../tests/fixtures/ports/toy-history" "$W/$1/port/history"
  chmod 755 "$W/$1/port/history"
}

t_ports() {
  case_ "test --port, swap, bind/1, rm refused, verify, list"
  run test --port history std
  rc_is 0
  out_has "std passes this suite in henki's own tests"
  run test --port context std
  refused "context is not a port this henki binds (history policy)"
  mkport tree
  run add "$W/tree" --yes
  rc_is 0
  run list
  out_has "history: std"
  if [ -z "${HENKI_BIN:-}" ]; then return 0; fi
  run test --port history tree
  rc_is 0
  out_has "ok H01"
  out_has "ok eof"
  out_has "history/1: 25 checks, 25 passed"
  HENKI_MODS_TTY=/nonexistent/tty run swap history tree
  refused "no terminal to ask on"
  check "no link without a yes" [ ! -L "$CONF/port/history" ]
  run swap history tree --yes
  rc_is 0
  out_has "history: std -> tree 0.1.0 ("
  out_has "conformance: passed (history/1, 25 checks) · it runs with your rights and stores every session record"
  out_has "henki --port history=std --resume reaches them"
  local store id
  store=$(readlink "$CONF/ext/tree")
  id=${store##*/}
  check "the link to the store copy's port/history" [ "$(readlink "$CONF/port/history")" = "$store/port/history" ]
  check "a bind/1 line after the lock/1 lines" [ "$(tail -n 1 "$CONF/mods.lock" | cut -f 1-4)" = "$(printf 'bind/1\thistory\ttree\t%s' "$id")" ]
  check "bind/1 and lock/1 lines have CONTRACTS.md's fields" [ "$(awk -F '\t' '{ print $1, NF }' "$CONF/mods.lock" | sort -u | tr '\n' ' ')" = "bind/1 $(doc_fields bind/1) lock/1 $(doc_fields lock/1) " ]
  check "how: yes" [ "$(tail -n 1 "$CONF/mods.lock" | cut -f 6)" = yes ]
  check "no temp link left" [ "$(ls -A "$CONF/port")" = history ]
  run list
  out_has "history: tree 0.1.0 (bound)"
  run verify
  rc_is 0
  mkmod "$W/other"
  run add "$W/other" --yes
  check "lock/1 lines stay sorted before bind/1" [ "$(cut -f 1,2 "$CONF/mods.lock" | tr '\t\n' ': ')" = "lock/1:other lock/1:tree bind/1:history " ]
  run rm tree
  refused "tree holds the history binding; fix: henki-mods swap history std"
  run off tree
  rc_is 0
  check "off keeps the binding" [ -L "$CONF/port/history" ]
  ln -sfn "$W/tree/port/history" "$CONF/port/history"
  run verify
  refused "is not the link to $store/port/history; fix: henki-mods swap history tree"
  run swap history std
  rc_is 0
  out_has "history: std"
  check "std: no link, no bind/1 line" [ ! -L "$CONF/port/history" ] && [ -z "$(command grep '^bind/1' "$CONF/mods.lock")" ]
  run rm tree
  rc_is 0
  # A binding that fails the suite is not swapped to; one that has no port/history neither.
  mkport bad
  printf '#!/bin/sh\nexit 0\n' >"$W/bad/port/history"
  run add "$W/bad" --yes
  run swap history bad --yes
  refused "bad does not pass the history/1 suite (above)"
  out_has "FAIL H01"
  check "no link after a failed suite" [ ! -L "$CONF/port/history" ]
  run swap history other --yes
  refused "other has no port/history"
  printf 'bind/1\thistory\tx\n' >>"$CONF/mods.lock"
  run list
  refused "not a bind/1 line; fix: henki-mods swap history std"
  run swap history std
  refused "not a bind/1 line"
  # A port a later henki binds, in a line its manager wrote: newer, not broken (swap context std cannot be named here).
  awk '!/^bind\/1/' "$CONF/mods.lock" >"$W/lock" && cp "$W/lock" "$CONF/mods.lock"
  printf 'bind/1\tcontext\tfoo\tabcabcabcabc\t2026-11-08T10:00Z\tyes\n' >>"$CONF/mods.lock"
  run list
  refused "this lock is newer than this henki-mods (it binds context); fix: upgrade henki"
  printf 'bind/1\tcontext\tfoo\n' >"$CONF/mods.lock"
  run list
  refused "not a bind/1 line; fix: remove the line"
  rm "$CONF/mods.lock"
  # No henki to run the suite: the command stops there, and no module is blamed.
  local hb="$HENKI_BIN" path="$PATH"
  HENKI_BIN='' PATH=/usr/bin:/bin
  run test --port history /bin/true
  HENKI_BIN=$hb PATH=$path
  refused "no henki to run; fix: run it as henki mods, or put henki on PATH"
  err_lacks "checks"
}

# port_toy (examples/, built beside the henki under test) through test --port: it passes, and each fault fails the
# check named for it.
t_port_toy() {
  case_ "test --port: port_toy and its faults"
  local toy f want
  toy="$(dirname "${HENKI_BIN:-/nonexistent/henki}")/examples/port_toy"
  [ -x "$toy" ] || return 0
  run test --port history "$toy"
  rc_is 0
  out_has "history/1: 25 checks, 25 passed"
  for f in "--exit-at create:H04" "--garbage-at read:H10" "--huge-at list:H15" "--error-at discard:H22" "--no-durable:H20" "--chatty:H01"; do
    want=${f##*:}
    printf '#!/bin/sh\nexec %s history %s\n' "$toy" "${f%:*}" >"$W/faulty"
    chmod 755 "$W/faulty"
    run test --port history "$W/faulty"
    rc_is 1
    out_has "FAIL $want: got"
  done
}

# A module NAME in W that binds policy: the bash binding of the policy suite as port/policy.
mkpolicy() {
  mkmod "$W/$1" 'ports = ["policy"]\ncontracts = ["policy/1"]\n'
  mkdir -p "$W/$1/port"
  cp "$HERE/../tests/fixtures/ports/toy-policy" "$W/$1/port/policy"
  chmod 755 "$W/$1/port/policy"
}

t_policy() {
  case_ "policy: test --port, swap with approve = auto, port_toy's faults"
  run test --port policy std
  rc_is 0
  mkpolicy rules
  run add "$W/rules" --yes
  rc_is 0
  if [ -z "${HENKI_BIN:-}" ]; then return 0; fi
  run test --port policy rules
  rc_is 0
  out_has "ok Q12"
  out_has "policy/1: 13 checks, 13 passed"
  printf 'approve = "auto"\n' >"$CONF/config.toml"
  run swap policy rules --yes
  rc_is 0
  out_has "policy: std -> rules 0.1.0 ("
  out_has "conformance: passed (policy/1, 13 checks) · it runs with your rights and decides every call before it runs; protected paths stay refused"
  out_has "henki refuses approve = auto with a policy binding; set approve = \"ask\" in ~/.config/henki/config.toml"
  check "the link to the store copy's port/policy" [ "$(readlink "$CONF/port/policy")" = "$(readlink "$CONF/ext/rules")/port/policy" ]
  run list
  out_has "policy: rules 0.1.0 (bound)"
  run rm rules
  refused "rules holds the policy binding; fix: henki-mods swap policy std"
  run swap policy std
  rc_is 0
  rm -f "$CONF/config.toml"
  local toy f want
  toy="$(dirname "$HENKI_BIN")/examples/port_toy"
  [ -x "$toy" ] || return 0
  for f in allow ask ask-always deny; do
    printf '#!/bin/sh\nexec %s policy %s\n' "$toy" "$f" >"$W/toy-$f"
    chmod 755 "$W/toy-$f"
    run test --port policy "$W/toy-$f"
    rc_is 0
    out_has "policy/1: 13 checks, 13 passed"
  done
  for f in "maybe:Q04" "allow --exit-at decide:Q04" "allow --hang-at decide#2:Q05" "allow --garbage-at record:Q12" "allow --chatty:Q01"; do
    want=${f##*:}
    printf '#!/bin/sh\nexec %s policy %s\n' "$toy" "${f%:*}" >"$W/faulty"
    chmod 755 "$W/faulty"
    run test --port policy "$W/faulty"
    rc_is 1
    out_has "FAIL $want: got"
  done
}

# A stub model server for the eval checks in D, a small agent: a message holding `CALL: CMD` gets a shell call of CMD
# and then the call's output as the answer, one holding BYTES the text of eval_bytes, and any other "yes" when the
# system prompt lists the module probe, else "no". Each request's model goes to D/models.
eval_stub_up() {
  local d="$1"
  mkdir -p "$d"
  cat >"$d/stub.py" <<'PY'
import http.server, json, os, socketserver, sys
d = sys.argv[1]
TEXT = 'café "q" back\\slash\ttab\nline two ​ \U000e0001 end'
def chunk(delta, finish=None):
    return "data: " + json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "object": "chat.completion.chunk"}) + "\n\n"
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with open(os.path.join(d, "models"), "a") as f:
            f.write(body["model"] + "\n")
        system = [m["content"] for m in body["messages"] if m["role"] == "system"][0]
        last = body["messages"][-1]
        content, finish = last.get("content") or "", "stop"
        if last["role"] == "tool":
            delta = {"role": "assistant", "content": "got " + content}
        elif "CALL: " in content:
            cmd = content.split("CALL: ", 1)[1].split("\n", 1)[0]
            call = {"index": 0, "id": "c1", "type": "function", "function": {"name": "shell", "arguments": json.dumps({"command": cmd})}}
            delta, finish = {"role": "assistant", "tool_calls": [call]}, "tool_calls"
        elif "BYTES" in content:
            delta = {"role": "assistant", "content": TEXT}
        else:
            delta = {"role": "assistant", "content": "yes" if "\n- probe: " in system else "no"}
        sse = (chunk(delta) + chunk({}, finish) + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(sse)))
        self.end_headers()
        self.wfile.write(sse)
    def log_message(self, *a):
        pass
s = socketserver.TCPServer(("127.0.0.1", 0), H)
open(os.path.join(d, "port"), "w").write(str(s.server_address[1]))
s.serve_forever()
PY
  stub_start "$d"
}

# A task T in DIR: prompt PROMPT, a setup that writes a marker, and the check CHECK (sh).
mktask() { # DIR T PROMPT CHECK
  mkdir -p "$1/$2"
  printf '%s\n' "$3" >"$1/$2/prompt.txt"
  printf '#!/bin/sh\nprintf "made by setup\\n" >marker\n' >"$1/$2/setup"
  printf '#!/bin/sh\n%s\n' "$4" >"$1/$2/check"
  chmod 755 "$1/$2/setup" "$1/$2/check"
}

# eval/1's layout (eval --check, and a module's eval/ in verify and add) and eval's refusals before any run.
t_eval() {
  case_ "eval --check: the general suite and the layout rules"
  local e="$W/ev" t
  run eval --check "$HERE/../eval/general"
  rc_is 0
  out_has "ok: eval/1, 12 tasks"
  mktask "$e" one "Say yes." '[ "$(cat)" = yes ]'
  printf 'about the tasks\n' >"$e/README.md" && printf 'x\n' >"$e/.hidden" && printf 'x\n' >"$e/one/.notes"
  mkdir -p "$e/one/fake-bin" && printf '#!/bin/sh\n' >"$e/one/fake-bin/jq" && chmod 755 "$e/one/fake-bin/jq"
  printf '# limits\nmax_steps = 10000\nmax_seconds = 9999\n' >"$e/one/task.toml"
  run eval --check "$e"
  rc_is 0
  out_has "ok: eval/1, 1 tasks"
  # Each broken copy of the task, alone in its dir, and the refusal that names it.
  bad() { # WHAT TEXT CHANGE
    rm -rf "$W/bad" && mkdir -p "$W/bad" && cp -R "$e/one" "$W/bad/one" && (cd "$W/bad/one" && eval "$3")
    run eval --check "$W/bad"
    refused "$2"
  }
  bad "no check" "one: no check" 'rm check'
  bad "a setup that cannot run" "setup: a program" 'chmod 644 setup'
  bad "an empty prompt" "0 bytes; fix: write a prompt of 1 to 4,096 bytes" ': >prompt.txt'
  bad "a prompt over 4 KiB" "4,097 bytes" 'head -c 4097 /dev/zero | tr "\000" x >prompt.txt'
  bad "U+202E in the prompt" "U+202E" 'printf "say \342\200\256 yes\n" >prompt.txt'
  bad "an unknown entry" "notes: unknown entry" 'printf x >notes'
  bad "a fake-bin entry that cannot run" "fake-bin/ holds programs" 'chmod 644 fake-bin/jq'
  bad "fake-bin a file" "fake-bin: not a directory" 'rm -r fake-bin && printf x >fake-bin'
  bad "an unknown task.toml key" "x: task.toml takes" 'printf "x = 1\n" >task.toml'
  bad "max_steps 0" "max_steps: task.toml takes" 'printf "max_steps = 0\n" >task.toml'
  bad "max_seconds a string" "max_seconds: task.toml takes" 'printf "max_seconds = \"3\"\n" >task.toml'
  bad "a task.toml line that is no assignment" "task.toml:1: expected key = value" 'printf "max_steps\n" >task.toml'
  rm -rf "$W/bad" && mkdir -p "$W/bad/Bad" && cp -R "$e/one/." "$W/bad/Bad/"
  run eval --check "$W/bad"
  refused "a task is a directory, and its name as a module's"
  rm -rf "$W/bad" && mkdir -p "$W/bad"
  run eval --check "$W/bad"
  refused "no task; fix:"

  case_ "a module's eval/ is held to eval/1 when it is added"
  mkmod "$W/probe"
  mkdir -p "$W/probe/eval" && cp -R "$e/one" "$W/probe/eval/one"
  run verify --dir "$W/probe"
  rc_is 0
  chmod 644 "$W/probe/eval/one/check"
  run verify --dir "$W/probe"
  refused "eval/one/check: a program"
  run add "$W/probe" --yes
  refused "eval/one/check: a program"
  chmod 755 "$W/probe/eval/one/check"
  run add "$W/probe" --yes
  rc_is 0

  case_ "eval: what it refuses before any run"
  printf '#!/bin/sh\necho "henki 0.2.0 (x)"\n' >"$W/henki" && chmod 755 "$W/henki"
  mkmod "$W/probe"
  mkmod "$W/plain"
  mkdir -p "$W/probe/eval" && cp -R "$e/one" "$W/probe/eval/one"
  run add "$W/probe" --yes
  run eval
  rc_is 2
  HENKI_BIN="$W/henki" run eval nope --yes
  refused "nope is not added"
  HENKI_BIN="$W/henki" run eval probe --repeats 0 --yes
  refused "--repeats 0: not a number from 1 to 99"
  HENKI_BIN="$W/henki" run eval probe --repeats 100 --yes
  refused "--repeats 100"
  HENKI_BIN="$W/henki" HENKI_DEPTH=1 run eval probe --yes
  refused "this runs inside henki (HENKI_DEPTH=1)"
  HENKI_BIN="$W/henki" run eval "$W/plain" --yes
  refused "plain has no eval/ tasks; fix: henki-mods eval $W/plain --suite general"
  HENKI_BIN="$W/henki" run eval probe --suite nope --yes
  refused "nope is not added"
  HENKI_BIN="$W/henki" HENKI_MODS_TTY="$W/no-tty" run eval probe
  refused "no terminal to ask on"
  out_has "eval: probe 0.1.0 (hd1:"
  out_has "each task's setup and check run as you, and henki with approve = auto"
  printf 'n\n' >"$W/answer"
  HENKI_BIN="$W/henki" HENKI_MODS_TTY="$W/answer" run eval probe
  refused "not run (the answer was no)"
  [ ! -e "$XDG_STATE_HOME/henki/eval.tsv" ] || fail "a refused eval wrote eval.tsv"
}

# Live: eval against a real henki and the eval stub, the arms told apart by what the model sees.
t_eval_live() {
  case_ "eval: two arms through henki run, the result lines, the repeats rule"
  live_ok || return
  local d="$W/stub" m="$W/probe" s="$W/suite" lines
  eval_stub_up "$d" || return
  mkdir -p "$CONF"
  printf 'model = "m"\nbase_url = "http://127.0.0.1:%s/v1"\next = ["~/elsewhere"]\n' "$STUB_PORT" >"$CONF/config.toml"
  chmod 600 "$CONF/config.toml"
  printf 'Be brief.\n' >"$CONF/AGENTS.md"
  mkmod "$m"
  printf '# probe: answers for tests\n' >"$m/doc.md"
  # ask: only the on arm's prompt lists probe; it also sees setup's marker and EVAL_DIR.
  mktask "$m/eval" ask "Is probe there?" '[ "$(cat)" = yes ] && [ -f marker ] && [ -f "$EVAL_DIR/prompt.txt" ]'
  # fake: a call runs fake-bin/fakething first on PATH, within max_steps.
  mktask "$m/eval" fake "CALL: fakething" 'command grep -q FAKE-OK'
  mkdir -p "$m/eval/fake/fake-bin" && printf '#!/bin/sh\necho FAKE-OK\n' >"$m/eval/fake/fake-bin/fakething"
  chmod 755 "$m/eval/fake/fake-bin/fakething" && printf 'max_steps = 3\n' >"$m/eval/fake/task.toml"
  # bytes: the check gets the answer's exact bytes (escapes, a surrogate pair, a tab, no added newline).
  mktask "$m/eval" bytes "BYTES" 'cmp - "$EVAL_DIR/.want"'
  printf 'caf\303\251 "q" back\\slash\ttab\nline two \342\200\213 \363\240\200\201 end' >"$m/eval/bytes/.want"
  run add "$m" --yes
  rc_is 0
  run eval probe --yes
  rc_is 0
  out_has "ask              on  pass  (ok, 1 steps,"
  out_has "ask              off fail  (ok, 1 steps,"
  out_has "fake             off pass  (ok, 2 steps,"
  out_has "bytes            off pass"
  out_lacks "10 more"
  check "eval/1 lines have CONTRACTS.md's fields" [ "$(awk -F '\t' '{ print NF }' "$XDG_STATE_HOME/henki/eval.tsv" | sort -u)" = "$(doc_fields eval/1)" ]
  lines=$(cat "$XDG_STATE_HOME/henki/eval.tsv")
  case "$lines" in "eval/1${tab}probe${tab}0.1.0${tab}$(field probe 8)${tab}m${tab}probe${tab}on${tab}9${tab}9${tab}"*"${nl}eval/1${tab}probe${tab}0.1.0${tab}"*"${tab}off${tab}6${tab}9${tab}"*) ;; *)
    fail "eval.tsv holds: $lines"
    ;;
  esac
  out_has "probe: on 9 of 9 passed, off 6 of 9; added to $XDG_STATE_HOME/henki/eval.tsv"
  # The arms within one pass of each other after 3 runs: 10 more, 13 each; --model names the model.
  mkdir -p "$s" && cp -R "$m/eval/fake" "$s/fake"
  : >"$d/models"
  run eval probe --suite "$s" --model other --yes
  rc_is 0
  out_has "the arms are within one pass of each other after 3 repeats: 10 more"
  [ "$(sort -u "$d/models")" = other ] || fail "the stub saw the models $(sort -u "$d/models" | tr '\n' ' ')"
  case "$(tail -n 2 "$XDG_STATE_HOME/henki/eval.tsv")" in *"${tab}other${tab}$s${tab}on${tab}13${tab}13${tab}"*"${tab}off${tab}13${tab}13${tab}"*) ;; *)
    fail "eval.tsv ends with: $(tail -n 2 "$XDG_STATE_HOME/henki/eval.tsv")"
    ;;
  esac
  # The general suite: every setup runs (none fails), one run per task and arm.
  run eval probe --suite general --repeats 1 --yes
  rc_is 0
  case "$(tail -n 1 "$XDG_STATE_HOME/henki/eval.tsv")" in *"${tab}general${tab}off${tab}"*"${tab}12${tab}"*) ;; *)
    fail "eval.tsv ends with: $(tail -n 1 "$XDG_STATE_HOME/henki/eval.tsv")"
    ;;
  esac
  kill "$STUB_PID" 2>/dev/null
  wait "$STUB_PID" 2>/dev/null
}

# The general suite's checks are deterministic and right: after each task's setup, a right solution and answer pass,
# and a near miss fails.
t_general() {
  case_ "the general suite: each check passes a right solution and fails a near miss"
  local g="$HERE/../eval/general" t n=0
  gen() { # TASK RIGHT_SOLUTION RIGHT_ANSWER WRONG_SOLUTION WRONG_ANSWER
    local which w sol ans
    n=$((n + 1))
    for which in right wrong; do
      w="$W/$1.$which"
      mkdir -p "$w"
      (cd "$w" && EVAL_DIR="$g/$1" "$g/$1/setup") </dev/null >"$W/setup.log" 2>&1 || {
        fail "$1: setup failed: $(cat "$W/setup.log")"
        return
      }
      if [ "$which" = right ]; then sol=$2 ans=$3; else sol=$4 ans=$5; fi
      (cd "$w" && eval "$sol") || fail "$1: the $which solution failed"
      if printf '%s\n' "$ans" | (cd "$w" && EVAL_DIR="$g/$1" "$g/$1/check") >/dev/null 2>&1; then
        [ "$which" = right ] || fail "$1: the check passed a near miss"
      else
        [ "$which" = wrong ] || fail "$1: the check failed the right answer"
      fi
    done
  }
  gen count-lines ': ' 'There are 19 such lines (status 404).' ': ' '23'
  gen rename 'cd photos && for f in *.JPG; do mv "$f" "${f%.JPG}.jpg"; done' done 'cd photos && for f in *JPG*; do mv "$f" "$(echo "$f" | tr A-Z a-z)"; done' done
  gen fix-script "printf '#!/bin/sh\necho \$((\$1 + \$2))\n' >sum.sh" fixed "printf '#!/bin/sh\necho 5\n' >sum.sh" fixed
  gen largest-file ': ' 'data/c/d/e/huge.bin' ': ' 'data/.cache'
  gen json-field '[ "$("$g/json-field/fake-bin/jq" .server.port config.json 2>&1; echo $?)" = "jq: command not found${nl}127" ]' '8443' ': ' '8443 (and 9443 for TLS)'
  gen git-log '[ "$(git log --diff-filter=A --format=%an -- parser.c)" = "Ada Lovelace" ]' 'Ada Lovelace' ': ' 'Alan Turing'
  gen write-script "printf '#!/bin/sh\nwc -w <\"\$1\"\n' >count-words && chmod 755 count-words" done "printf '#!/bin/sh\nwc -w \"\$1\"\n' >count-words && chmod 755 count-words" done
  gen config-value "sed 's/^timeout = 60\$/timeout = 30/' app.ini >x && mv x app.ini" done "sed 's/^timeout = .*/timeout = 30/' app.ini >x && mv x app.ini" done
  gen find-todo '[ "$(command grep -rn TODO src | cut -d: -f1,2)" = src/lib/parse.sh:14 ]' 'src/lib/parse.sh:14' ': ' 'src/lib/parse.sh:13'
  gen dir-tree 'mkdir -p site/css site/js site/img/icons && : >site/index.html' done 'mkdir -p site/css site/js site/img/icons && echo hi >site/index.html' done
  gen compare-files '[ "$(diff old.txt new.txt | sed -n 1p)" = 7c7 ]' '7: color = navy' ': ' '7: color = teal'
  gen explain-error '! ./run.sh >/dev/null 2>&1' 'settings.conf' 'touch settings.conf' 'settings.conf'
  t=$(cd "$g" && ls | wc -l | tr -d ' ')
  [ "$n" = "$t" ] || fail "the general suite has $t tasks, and this case covers $n"
}

# --- the signed index ----------------------------------------------------------------------------------------------------

# The index fixtures are signed with ssh-keygen: OpenSSH's when it is on PATH, else the test stand-in (python3 and
# openssl; testdata/ssh-keygen), which reads and writes the same files. Three keys for the whole run.
if ! command -v ssh-keygen >/dev/null 2>&1; then
  PATH="$HERE/testdata:$PATH"
  echo "ssh-keygen: the test stand-in (python3 + openssl)"
fi
KEYS="$SCRATCH/keys"
mkdir -p "$KEYS"
for k in k1 k2 k3; do ssh-keygen -q -t ed25519 -N '' -C "$k" -f "$KEYS/$k" </dev/null >/dev/null || exit 1; done
# KEY's allowed_signers line, and its blob.
signer() { printf 'henki-index namespaces="git" %s\n' "$(cut -d ' ' -f 1,2 "$KEYS/$1.pub")"; }
blob() { cut -d ' ' -f 2 "$KEYS/$1.pub"; }

# A commit of everything in index DIR: signed by KEY (k1 by default), or unsigned with KEY none.
ixc() { # DIR MESSAGE [KEY]
  git -C "$1" add -A
  if [ "${3:-k1}" = none ]; then
    git -C "$1" -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q --allow-empty -m "$2"
  else
    git -C "$1" -c gpg.format=ssh -c user.signingkey="$KEYS/${3:-k1}" -c core.hooksPath=/dev/null commit -q --allow-empty -S -m "$2"
  fi
}

# An index at IX (W/index by default): .signers with k1, an empty removed.toml, one signed root commit on main.
mkindex() {
  IX="${1:-$W/index}"
  mkdir -p "$IX/mods"
  signer k1 >"$IX/.signers"
  printf '# removed modules\n' >"$IX/removed.toml"
  git init -q -b main "$IX"
  ixc "$IX" root
}

# The default index's anchors in W/share (HENKI_SHARE): IX's URL, KEY's line (k1), the root commit as the checkpoint,
# and no refused key.
mkshare() { # [KEY]
  mkdir -p "$W/share/index"
  printf 'file://%s\n' "$IX" >"$W/share/index/deorlovnis.url"
  signer "${1:-k1}" >"$W/share/index/deorlovnis.signers"
  git -C "$IX" rev-list --max-parents=0 HEAD >"$W/share/index/deorlovnis.checkpoint"
  printf '# none\n' >"$W/share/index/refused.keys"
  export HENKI_SHARE="$W/share"
}

# KEY = VALUE (raw) in place of FILE's KEY line, or added.
setkey() { # FILE KEY VALUE
  HM_K="$2" HM_V="$3" awk '{ k = $0; sub(/ *=.*/, "", k) } k == ENVIRON["HM_K"] { if (!d++) print ENVIRON["HM_K"] " = " ENVIRON["HM_V"]; next } { print } END { if (!d) print ENVIRON["HM_K"] " = " ENVIRON["HM_V"] }' "$1" >"$1.new" && mv "$1.new" "$1"
}

# FILE without its KEY line.
unkey() { HM_K="$2" awk '{ k = $0; sub(/ *=.*/, "", k) } k != ENVIRON["HM_K"]' "$1" >"$1.new" && mv "$1.new" "$1"; }

# A module repository W/NAME at VERSION (a commit more), with EXTRA manifest lines.
mkmodrepo() { # NAME VERSION [EXTRA]
  [ -d "$W/$1/.git" ] || mkmod "$W/$1" "${3:-}"
  setkey "$W/$1/.henki/module.toml" version "\"$2\""
  printf '%s\n' "$2" >"$W/$1/CHANGELOG.md"
  mkrepo "$W/$1"
}

# IX's entry for module repository W/NAME at its HEAD as VERSION: the package file (latest VERSION) and the version
# file the author tool derives, reviewed unless REVIEW is "no".
ixentry() { # NAME VERSION [REVIEW]
  local d="$W/$1"
  mkdir -p "$IX/mods/$1"
  [ -f "$IX/mods/$1.toml" ] || printf 'index = 1\nname = "%s"\nkind = "git"\nrepo = "file://%s"\npath = ""\nsummary = "%s does things for tests"\nmaintainer = "someone"\nlicense = "0BSD"\nlatest = "%s"\nadded = "2026-11-08"\n' "$1" "$d" "$1" "$2" >"$IX/mods/$1.toml"
  setkey "$IX/mods/$1.toml" latest "\"$2\""
  {
    echo 'index = 1'
    HENKI_INDEX_ALLOW_FILE=1 $RUN "$HERE/henki-mods-dev" entry derive "file://$d" "" "$(git -C "$d" rev-parse HEAD)"
    [ "${3:-}" = no ] || printf 'reviewed = "2026-11-08"\nreviewer = "someone"\n'
  } >"$IX/mods/$1/$2.toml"
}

ixv() { git --git-dir="$XDG_DATA_HOME/henki/index/${1:-deorlovnis}.git" rev-parse -q --verify refs/henki/verified 2>/dev/null; }

t_index_sync() {
  case_ "index: placeholder, first sync, bad commits, rotation, rollback, grafts, refused keys, offline, age"
  mkdir -p "$W/share/index"
  printf 'https://example.invalid/index\n' >"$W/share/index/deorlovnis.url"
  printf '# no key yet\n' >"$W/share/index/deorlovnis.signers"
  export HENKI_SHARE="$W/share"
  run search
  refused "index deorlovnis is not live yet"
  run add nosuch --yes
  refused "no official module nosuch, and index deorlovnis is not live yet"
  run add sessions --yes
  rc_is 0
  out_lacks "index"
  mkindex
  mkshare
  run sync
  rc_is 0
  out_has "index deorlovnis: $(git -C "$IX" rev-parse HEAD | cut -c 1-12), 1 new commits verified (1 epochs)"
  out_has "index deorlovnis: henki-index @ "
  out_has "(verified, today; key SHA256:"
  out_has "anchor: $W/share/index/deorlovnis.signers"
  check "verified is main" [ "$(ixv)" = "$(git -C "$IX" rev-parse HEAD)" ]
  local v0 v1
  v0=$(ixv)
  # An unsigned commit, and one signed by a key .signers does not list: refused, the verified commit kept.
  ixc "$IX" unsigned none
  run sync
  rc_is 0
  err_has "is not signed (the .signers of its first parent, minus refused keys); refused"
  err_has "keeps using $(printf '%s' "$v0" | cut -c 1-12)"
  check "still the old commit" [ "$(ixv)" = "$v0" ]
  git -C "$IX" reset -q --hard HEAD~1
  ixc "$IX" "other key" k2
  run sync
  err_has "is signed by a key that is not in the signers list"
  check "still the old commit" [ "$(ixv)" = "$v0" ]
  git -C "$IX" reset -q --hard HEAD~1
  # An OpenPGP signature never counts, even one the person's own gpg calls good (a stand-in that says so to all).
  printf '%s\n' '#!/bin/sh' 'cat >/dev/null' 'case " $* " in *" -bsau "*)' \
    "  echo '[GNUPG:] SIG_CREATED D 22 8 00 1700000000 0123456789ABCDEF0123456789ABCDEF01234567' >&2" \
    "  printf -- '-----BEGIN PGP SIGNATURE-----\\n\\nZm9yZ2Vk\\n-----END PGP SIGNATURE-----\\n' ;;" \
    "*) printf '[GNUPG:] NEWSIG\\n[GNUPG:] GOODSIG 0123456789ABCDEF forger\\n[GNUPG:] VALIDSIG 0123456789ABCDEF0123456789ABCDEF01234567 2026-11-08 1700000000 0 4 0 22 8 00 0123456789ABCDEF0123456789ABCDEF01234567\\n[GNUPG:] TRUST_FULLY 0 pgp\\n' ;;" \
    'esac' >"$W/gpg"
  chmod 755 "$W/gpg"
  git config --global gpg.program "$W/gpg"
  git -C "$IX" -c user.signingkey=forger -c core.hooksPath=/dev/null commit -q --allow-empty -S -m pgp
  check "the stand-in makes git call the commit good" [ "$(git -C "$IX" log -1 --format=%G?)" = G ]
  run sync
  err_has "is not signed (the .signers of its first parent, minus refused keys); refused"
  check "still the old commit" [ "$(ixv)" = "$v0" ]
  git config --global --unset gpg.program
  git -C "$IX" reset -q --hard HEAD~1
  # A rotation: k1 signs .signers = k2, then k2 signs; a later commit by k1 is refused.
  signer k2 >"$IX/.signers"
  ixc "$IX" rotate k1
  ixc "$IX" "by k2" k2
  run sync
  rc_is 0
  out_has "2 new commits verified (2 epochs)"
  v1=$(ixv)
  check "verified moved" [ "$v1" = "$(git -C "$IX" rev-parse HEAD)" ]
  ixc "$IX" "by the old key" k1
  run sync
  err_has "is signed by a key that is not in the signers list"
  check "still v1" [ "$(ixv)" = "$v1" ]
  # A rewritten main that no longer holds the verified commit: no rollback.
  git -C "$IX" reset -q --hard "$v0"
  run sync
  err_has "main's first-parent line no longer holds the verified commit $v1"
  check "still v1" [ "$(ixv)" = "$v1" ]
  # The graft: k1, rotated out at v1's parent, signs X on the pre-rotation root and merges v1 as a second parent.
  git -C "$IX" -c gpg.format=ssh -c user.signingkey="$KEYS/k1" merge -q --no-ff -S -m graft "$v1" >/dev/null 2>&1 ||
    fail "the graft merge failed"
  check "v1 is the merge's second parent" [ "$(git -C "$IX" rev-parse HEAD^2)" = "$v1" ]
  run sync
  err_has "no longer holds the verified commit"
  check "still v1" [ "$(ixv)" = "$v1" ]
  # A first sync of that history with the checkpoint at v1: refused; without one it would verify (why one ships).
  printf '%s\n' "$v1" >"$W/share/index/deorlovnis.checkpoint"
  rm -rf "$XDG_DATA_HOME/henki/index"
  run sync
  refused "does not hold the checkpoint $v1"
  rm -f "$W/share/index/deorlovnis.checkpoint"
  rm -rf "$XDG_DATA_HOME/henki/index"
  run sync
  rc_is 0
  err_has "no checkpoint to start from"
  # The anchors beside the program are what a release ships: there, a missing checkpoint refuses a first sync.
  rm -rf "$XDG_DATA_HOME/henki/index"
  mkdir -p "$W/bin/index"
  cp "$MODS" "$W/bin/henki-mods"
  cp "$W/share/index/deorlovnis.url" "$W/share/index/deorlovnis.signers" "$W/share/index/refused.keys" "$W/bin/index/"
  RC=0
  OUT=$(unset HENKI_SHARE && $RUN "$W/bin/henki-mods" sync 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
  refused "index deorlovnis: no checkpoint beside henki-mods"
  check "nothing verified" [ -z "$(ixv)" ]
  # A key in refused.keys never verifies, even where .signers lists it.
  rm -rf "$XDG_DATA_HOME/henki/index"
  blob k1 >"$W/share/index/refused.keys"
  run sync
  refused "(a root commit: the anchor's keys, minus refused keys); refused"
  printf '# none\n' >"$W/share/index/refused.keys"
  run sync
  rc_is 0
  # Offline: the URL is gone, the verified commit stays in use; an edited ref is refused before a read.
  printf 'file://%s/gone\n' "$W" >"$W/share/index/deorlovnis.url"
  run search
  rc_is 0
  out_has "index deorlovnis: offline; using"
  ixc "$IX" "unsigned at the head" none
  git --git-dir="$XDG_DATA_HOME/henki/index/deorlovnis.git" fetch -q "$IX" "+main:refs/henki/verified" 2>/dev/null
  run search
  refused "is not signed"
  rm -rf "$XDG_DATA_HOME/henki/index"
  # A verified commit over 30 days old: a warning on every index screen.
  mkindex "$W/old"
  printf 'file://%s\n' "$IX" >"$W/share/index/deorlovnis.url"
  GIT_COMMITTER_DATE="2001-01-01T00:00:00Z" ixc "$IX" old
  run search
  rc_is 0
  err_has "days old; a frozen index stops showing yanks; fix: henki-mods sync, or report it"
  run list
  err_has "days old"
}

# 5,001 commits on main's first-parent line are refused before any signature is checked.
t_index_long() {
  case_ "index: more than 5,000 commits"
  mkindex
  mkshare
  local i=0 root
  root=$(git -C "$IX" rev-parse HEAD)
  {
    while [ $i -lt 5001 ]; do
      i=$((i + 1))
      printf 'commit refs/heads/main\ncommitter f <f@x> %s +0000\ndata 1\nx\n' "$((1700000000 + i))"
      [ $i != 1 ] || printf 'from %s\n' "$root"
      printf '\n'
    done
  } | git -C "$IX" fast-import --quiet || fail "fast-import"
  run sync
  refused "main's first-parent line has more than 5,000 commits; refused"
}

# An index W/index with module repositories hello (0.1.0 and 0.2.0) and lean (0.1.0), synced from W/share.
ix_two() {
  mkindex
  mkmodrepo hello 0.1.0
  ixentry hello 0.1.0
  mkmodrepo hello 0.2.0
  ixentry hello 0.2.0
  mkmodrepo lean 0.1.0
  ixentry lean 0.1.0
  ixc "$IX" entries
  mkshare
}

t_index_add() {
  case_ "index: search, info, add NAME (tiers, hd1, yanked, os, NAME@INDEX, reserved names, unreviewed, a URL's tier)"
  ix_two
  run search
  rc_is 0
  out_has "hello            reviewed                 0.2.0"
  out_has "lean             reviewed                 0.1.0"
  run search LEAN
  out_has "lean "
  out_lacks "hello "
  run info hello
  rc_is 0
  out_has "hello 0.2.0 · reviewed by someone (signed index henki-index, 2026-11-08)"
  out_has "hello 0.1.0 · reviewed by someone"
  out_has "install: henki-mods add hello"
  run add hello --yes
  rc_is 0
  out_has "source   index deorlovnis: hello 0.2.0 (file://$W/hello @ $(git -C "$W/hello" rev-parse HEAD))"
  out_has "review   reviewed by someone (signed index henki-index, 2026-11-08)"
  check "kind index:deorlovnis, source hello, version 0.2.0" [ "$(field hello 5):$(field hello 6):$(field hello 4)" = "index:deorlovnis:hello:0.2.0" ]
  run list
  out_has "index deorlovnis: hello"
  run verify
  rc_is 0
  # A version file whose hd1 is not the commit's files: refused.
  local hd other=macos
  hd=$(sed -n 's/^hd1 = //p' "$IX/mods/lean/0.1.0.toml")
  setkey "$IX/mods/lean/0.1.0.toml" hd1 '"hd1:0000000000000000000000000000000000000000000000000000000000000000"'
  ixc "$IX" "a wrong hd1"
  run add lean --yes
  refused "are not the reviewed ones"
  check "not added" [ -z "$(lockline lean)" ]
  setkey "$IX/mods/lean/0.1.0.toml" hd1 "$hd"
  # A yanked version is never installed; --version takes an older one.
  run rm hello
  setkey "$IX/mods/hello/0.1.0.toml" yanked '"buggy: loses data (2026-11-08)"'
  ixc "$IX" "yank 0.1.0"
  run add hello --version 0.1.0 --yes
  refused "hello 0.1.0 was yanked (buggy: loses data (2026-11-08)); not installed"
  # A latest version for another OS gives way to the newest older one that runs here.
  unkey "$IX/mods/hello/0.1.0.toml" yanked
  [ "$(uname -s)" != Darwin ] || other=linux
  printf 'os = ["%s"]\n' "$other" >>"$W/hello/.henki/module.toml"
  mkmodrepo hello 0.3.0
  ixentry hello 0.3.0
  ixc "$IX" "0.3.0 for $other"
  run add hello --yes
  rc_is 0
  out_has "hello 0.3.0 runs on $other only; looking at older versions"
  out_has "source   index deorlovnis: hello 0.2.0"
  run rm hello
  # A version nobody reviewed: red, and named so.
  mkmodrepo fresh 0.1.0
  ixentry fresh 0.1.0 no
  ixc "$IX" "an unreviewed entry"
  run add fresh --yes
  rc_is 0
  out_has "UNREVIEWED: listed in henki-index, but no reviewer has read this version"
  # A version this henki-mods cannot read is counted as such, not hidden.
  mkmodrepo odd 0.1.0
  ixentry odd 0.1.0
  setkey "$IX/mods/odd/0.1.0.toml" newkey '"x"'
  ixc "$IX" "an entry in a later format"
  run add odd --yes
  refused "no version of odd in index deorlovnis can be installed here (0 yanked, 0 not for this henki or system, 1 this henki-mods cannot read: the warnings above)"
  run add odd --version 0.1.0 --yes
  refused "odd 0.1.0 in index deorlovnis cannot be read by this henki-mods (the warning above); fix: upgrade henki"
  # A URL install of files an index lists gets their review; others stay red.
  run add "file://$W/lean" --yes
  rc_is 0
  out_has "review   reviewed by someone (signed index henki-index, 2026-11-08) (the same files as lean 0.1.0 of index deorlovnis)"
  out_lacks "UNREVIEWED"
  # A second index, the person's own: a name both list needs NAME@INDEX; it never provides a reserved or official name.
  mkdir -p "$CONF/index"
  local main="$IX"
  mkindex "$W/extra"
  mkdir -p "$IX/mods/hello" "$IX/mods/sessions"
  cp "$main/mods/hello.toml" "$IX/mods/"
  cp "$main/mods/hello/0.2.0.toml" "$IX/mods/hello/"
  setkey "$IX/mods/hello.toml" latest '"0.2.0"'
  printf 'index = 1\nname = "sessions"\nkind = "git"\nrepo = "file://%s"\npath = ""\nsummary = "not the official one"\nmaintainer = "someone"\nlicense = "0BSD"\nlatest = "0.2.0"\nadded = "2026-11-08"\n' "$W/hello" >"$IX/mods/sessions.toml"
  cp "$main/mods/hello/0.2.0.toml" "$IX/mods/sessions/0.2.0.toml"
  ixc "$IX" "extra entries"
  printf 'index/1\textra\tfile://%s\n' "$IX" >"$CONF/mods.index"
  signer k1 >"$CONF/index/extra.signers"
  IX=$main
  run add hello --yes
  refused "hello is listed by more than one index: deorlovnis extra; fix: name one, like henki-mods add hello@extra"
  run search
  rc_is 0
  out_has "index extra: henki-index @ "
  out_has "anchor: $CONF/index/extra.signers"
  out_lacks "not the official one"
  run add hello@extra --yes
  rc_is 0
  check "kind index:extra" [ "$(field hello 5)" = index:extra ]
  run add sessions@extra --yes
  refused "no index you use lists sessions (in index extra)"
  # A commit the module repository no longer has.
  git -C "$W/hello" reset -q --hard HEAD~2
  git -C "$W/hello" -c commit.gpgsign=false commit -q --allow-empty -m rewritten
  git -C "$W/hello" reflog expire --expire=now --all && git -C "$W/hello" gc -q --prune=now 2>/dev/null
  run rm hello
  run add hello@deorlovnis --yes
  refused "is gone from file://$W/hello (rewritten history?); fix: tell the module's maintainer"
  # doctor: HENKI_SHARE in the warning form.
  run doctor
  out_has "WARN  HENKI_SHARE is set: the default index's anchor comes from $W/share/index"
  # A git too old for SSH signatures: with an index synced, a path install and on are refused, and the message says
  # so; with none synced, a path install works.
  mkdir -p "$W/oldgit"
  printf '#!/bin/sh\n[ "$1" != --version ] || { echo "git version 2.30.0"; exit 0; }\nexec "%s" "$@"\n' "$(command -v git)" >"$W/oldgit/git"
  chmod 755 "$W/oldgit/git"
  mkmod "$W/plain"
  PATH="$W/oldgit:$PATH" run add "$W/plain" --yes
  refused "(this git is 2.30.0), and index deorlovnis is synced here, so installs, on and swap are refused; fix: upgrade git, or rm -rf"
  err_lacks "still work"
  run off lean
  rc_is 0
  PATH="$W/oldgit:$PATH" run on lean
  refused "so installs, on and swap are refused"
  rm -rf "$XDG_DATA_HOME/henki/index"
  PATH="$W/oldgit:$PATH" run add "$W/plain" --yes
  rc_is 0
}

t_index_yank() {
  case_ "index: yanks at sync and at every install (malicious, buggy, ports, other indexes, pins)"
  ix_two
  run add hello --yes
  rc_is 0
  run add lean --yes
  rc_is 0
  mkmod "$W/byurl"
  mkrepo "$W/byurl"
  run add "file://$W/byurl" --yes
  rc_is 0
  local hstore
  hstore=$(readlink "$CONF/ext/hello")
  # hello binds history, lean policy (as a swap leaves them).
  mkdir -p "$CONF/port"
  ln -s "$hstore/bin/hello" "$CONF/port/history"
  ln -s "$(readlink "$CONF/ext/lean")/bin/lean" "$CONF/port/policy"
  printf 'bind/1\thistory\thello\t%s\t2026-11-08T10:00Z\tyes\nbind/1\tpolicy\tlean\t%s\t2026-11-08T10:00Z\tyes\n' "${hstore##*/}" "$(readlink "$CONF/ext/lean" | sed 's|.*/||')" >>"$CONF/mods.lock"
  # A pin does not keep a malicious module on.
  awk -F '\t' 'BEGIN { OFS = "\t" } $2 == "hello" { $11 = "pin" } { print }' "$CONF/mods.lock" >"$W/l" && cp "$W/l" "$CONF/mods.lock"
  setkey "$IX/mods/hello/0.2.0.toml" yanked '"malicious: steals keys (2026-11-08)"'
  setkey "$IX/mods/lean/0.1.0.toml" yanked '"malicious: phones home (2026-11-08)"'
  printf 'byurl = "buggy: a test (2026-11-08)"\n' >>"$IX/removed.toml"
  ixc "$IX" yanks
  run sync
  rc_is 0
  out_has "hello 0.2.0 was yanked as malicious (steals keys (2026-11-08)) by index deorlovnis: turned off; its files are kept at"
  out_has "history: henki will not start until you choose: henki-mods swap history std (or another package) for good, or henki --port history=std for one session"
  out_has "policy: std again (it was lean's)"
  check "hello off, flagged, pinned" [ "$(field hello 3):$(field hello 11)" = "off:pin,yanked=malicious" ]
  check "no ext link" [ ! -L "$CONF/ext/hello" ]
  check "the history marker" [ "$(readlink "$CONF/port/history")" = yanked ]
  check "bind/1 history yanked" [ "$(awk -F '\t' '$1 == "bind/1" && $2 == "history" { print $6 }' "$CONF/mods.lock")" = yanked ]
  check "policy unbound" [ ! -L "$CONF/port/policy" ] && [ -z "$(awk -F '\t' '$2 == "policy"' "$CONF/mods.lock")" ]
  check "the URL install is untouched (a removal acts on index lines only)" [ "$(field byurl 3):$(field byurl 11)" = "on:ref=HEAD" ]
  run sync
  out_lacks "was yanked as malicious"
  run on hello
  refused "hello 0.2.0 was yanked as malicious; it stays off; fix: henki-mods rm hello"
  run list
  out_has "history: STOPPED: hello was yanked as malicious"
  run verify
  refused "history is stopped: hello was yanked as malicious"
  if [ -n "${HENKI_BIN:-}" ]; then
    RC=0
    OUT=$(cd "$W" && HENKI_MODEL=m HENKI_BASE_URL=http://127.0.0.1:9 "$HENKI_BIN" run x 2>&1) || RC=$?
    case "$OUT" in *"henki mods swap history std"*) ;; *) fail "henki started with the marker: $OUT" ;; esac
  fi
  run swap history std
  rc_is 0
  run verify
  rc_is 0
  # A swap never binds a malicious module again (a module with the port: below).
  run swap history hello --yes
  refused "hello 0.2.0 was yanked as malicious; it stays off; fix: henki-mods swap history std, or to another package"
  check "history stays std" [ ! -L "$CONF/port/history" ]
  run swap policy lean --yes
  refused "lean 0.1.0 was yanked as malicious; it stays off; fix: henki-mods swap policy std, or to another package"
  check "policy stays std" [ ! -L "$CONF/port/policy" ] && [ -z "$(awk -F '\t' '$2 == "policy"' "$CONF/mods.lock")" ]
  # A buggy yank is said once and changes nothing. Another index's yank of the same files, and its removal of a
  # name, act on nothing it did not provide.
  mkdir -p "$CONF/index"
  local main="$IX"
  mkindex "$W/extra"
  mkdir -p "$IX/mods/lean"
  cp "$main/mods/lean.toml" "$IX/mods/"
  cp "$main/mods/lean/0.1.0.toml" "$IX/mods/lean/"
  printf 'hello = "malicious: not its own (2026-11-08)"\n' >>"$IX/removed.toml"
  ixc "$IX" "extra yanks"
  printf 'index/1\textra\tfile://%s\n' "$IX" >"$CONF/mods.index"
  signer k1 >"$CONF/index/extra.signers"
  IX=$main
  run rm lean
  run rm hello
  unkey "$IX/mods/lean/0.1.0.toml" yanked
  unkey "$IX/mods/hello/0.2.0.toml" yanked
  ixc "$IX" "unyanked"
  run add hello@deorlovnis --yes
  rc_is 0
  out_has "source   index deorlovnis: hello 0.2.0"
  run add "file://$W/lean" --yes
  rc_is 0
  setkey "$IX/mods/hello/0.2.0.toml" yanked '"buggy: slow (2026-11-08)"'
  ixc "$IX" "buggy"
  run sync
  rc_is 0
  out_has "hello 0.2.0 was yanked as buggy (slow (2026-11-08)) by index deorlovnis; it stays as it is; fix: henki-mods rm hello"
  out_lacks "lean"
  run sync
  out_lacks "was yanked"
  check "lean by URL stays on" [ "$(field lean 3)" = on ]
  check "hello stays on, flagged" [ "$(field hello 3):$(field hello 11)" = on:yanked=buggy ]
  # A malicious hd1 in the default index refuses a URL install.
  run rm lean
  setkey "$IX/mods/lean/0.1.0.toml" yanked '"malicious: phones home (2026-11-08)"'
  ixc "$IX" "lean malicious"
  run sync
  run add "file://$W/lean" --yes
  refused "was yanked as malicious (phones home (2026-11-08)) by index deorlovnis; not installed"
  run on hello
  rc_is 0
  out_has "yanked   as buggy (slow (2026-11-08)) by index deorlovnis"
  # A version file this henki-mods cannot read (a key it does not know) is skipped, and its yank still applies.
  setkey "$IX/mods/hello/0.2.0.toml" yanked '"malicious: steals keys (2026-11-08)"'
  setkey "$IX/mods/hello/0.2.0.toml" newkey '"x"'
  ixc "$IX" "a yank in a later format"
  run sync
  rc_is 0
  err_has "unknown key newkey"
  out_has "hello 0.2.0 was yanked as malicious (steals keys (2026-11-08)) by index deorlovnis: turned off"
  check "hello off, flagged" [ "$(field hello 3):$(field hello 11)" = "off:yanked=malicious" ]
  # A history module bound, yanked as malicious, then swapped to again: refused, flag or not; the port stays std.
  mkport tree
  mkrepo "$W/tree"
  ixentry tree 0.1.0
  setkey "$IX/mods/tree/0.1.0.toml" conform '"history/1 25/25"'
  ixc "$IX" tree
  run add tree --yes
  rc_is 0
  if [ -n "${HENKI_BIN:-}" ]; then
    run swap history tree --yes
    rc_is 0
  fi
  setkey "$IX/mods/tree/0.1.0.toml" yanked '"malicious: forges sessions (2026-11-08)"'
  ixc "$IX" "tree malicious"
  run sync
  rc_is 0
  out_has "tree 0.1.0 was yanked as malicious"
  run swap history std
  rc_is 0
  run swap history tree --yes
  refused "tree 0.1.0 was yanked as malicious; it stays off; fix: henki-mods swap history std, or to another package"
  awk -F '\t' 'BEGIN { OFS = "\t" } $2 == "tree" { $11 = "" } { print }' "$CONF/mods.lock" >"$W/l" && cp "$W/l" "$CONF/mods.lock"
  run swap history tree --yes
  refused "tree 0.1.0 was yanked as malicious (forges sessions (2026-11-08)) by index deorlovnis"
  check "history stays std" [ ! -L "$CONF/port/history" ] && [ -z "$(awk -F '\t' '$1 == "bind/1"' "$CONF/mods.lock")" ]
}

t_index_guard() {
  case_ "inside a tool call; the history file; lock flags and kinds; entry derive and lint; test --port DIR"
  mkmod "$W/hello"
  printf 'y\n' >"$W/answer"
  local v
  for v in "add $W/hello --yes" "on hello" "swap history std"; do
    # shellcheck disable=SC2086 # the verb and its arguments
    HENKI_SESSION=20261108T100000Z-abcd HENKI_MODS_TTY="$W/answer" run $v
    refused "this changes what henki runs, and it runs inside a tool call; fix: run it at the prompt with /mods"
  done
  check "nothing added" [ ! -f "$CONF/mods.lock" ]
  HENKI_SESSION=20261108T100000Z-abcd HENKI_NEXT="$W/next" run add "$W/hello" --yes
  rc_is 0
  HENKI_SESSION=20261108T100000Z-abcd run off hello
  rc_is 0
  local line
  line=$(lockline hello)
  run rm hello
  rc_is 0
  check "rm leaves a hist/1 line with the line it removed" [ "$(cut -f 1,3- "$CONF/mods.history")" = "$(printf 'hist/1\trm\t%s' "$line")" ]
  check "hist/1: a UTC time" sh -c "cut -f 2 '$CONF/mods.history' | command grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}Z$'"
  run add "$W/hello" --yes
  awk -F '\t' 'BEGIN { OFS = "\t" } { $5 = "skill"; $11 = "pin,ref=main,skill=hd1:ab12,mask=cmd/x" } { print }' "$CONF/mods.lock" >"$W/l" && cp "$W/l" "$CONF/mods.lock"
  run lock
  rc_is 0
  awk -F '\t' 'BEGIN { OFS = "\t" } { $11 = "pin,later" } { print }' "$CONF/mods.lock" >"$W/l" && cp "$W/l" "$CONF/mods.lock"
  run lock
  refused "this lock is newer than this henki-mods (a flag it does not know); fix: upgrade henki"
  rm "$CONF/mods.lock"
  # entry derive: what the module decides; entry lint: the index's rules, also through henki-mods.
  mkmod "$W/derived" 'ports = ["history"]\ncontracts = ["history/1"]\nrequires = ["henki >= 0.2"]\n'
  mkdir -p "$W/derived/port" "$W/derived/cmd" "$W/derived/on"
  printf '#!/bin/sh\n' >"$W/derived/port/history"
  printf '#!/bin/sh\n' >"$W/derived/cmd/zz"
  printf '#!/bin/sh\n' >"$W/derived/cmd/aa"
  printf 'data\n' >"$W/derived/cmd/aa.md"
  printf '#!/bin/sh\n' >"$W/derived/on/session_end"
  chmod 755 "$W/derived/port/history" "$W/derived/cmd/zz" "$W/derived/cmd/aa" "$W/derived/on/session_end"
  mkrepo "$W/derived"
  RC=0
  OUT=$($RUN "$HERE/henki-mods-dev" entry derive "file://$W/derived" "" "$(git -C "$W/derived" rev-parse HEAD)" 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
  rc_is 0
  out_has "$(printf 'version = "0.1.0"\ncommit = "%s"\nhd1 = "hd1:' "$(git -C "$W/derived" rev-parse HEAD)")"
  out_has "$(printf 'requires = ["henki >= 0.2"]\ncontracts = ["history/1"]\nos = []\nports = ["history"]\ncommands = ["aa", "zz"]\nbins = ["derived"]\nobservers = ["session_end"]\nctx = []\nprompt_bytes = ')"
  out_has "$(printf 'card_bytes = 0\ncaps_net = "none"')"
  mkdir -p "$W/ix/mods/derived"
  printf 'index = 1\n%s\n' "$OUT" >"$W/ix/mods/derived/0.1.0.toml"
  run entry lint "$W/ix/mods/derived/0.1.0.toml"
  refused "conform"
  printf 'conform = "history/1 25/25"\n' >>"$W/ix/mods/derived/0.1.0.toml"
  run entry lint "$W/ix/mods/derived/0.1.0.toml"
  rc_is 0
  out_has "0.1.0.toml: ok"
  printf 'index = 1\nname = "derived"\nkind = "git"\nrepo = "http://x.example/r"\npath = ""\nsummary = "s"\nmaintainer = "someone"\nlicense = "0BSD"\nlatest = "0.1.0"\nadded = "2026-11-08"\n' >"$W/ix/mods/derived.toml"
  run entry lint "$W/ix/mods/derived.toml"
  refused "repo must be an https URL"
  RC=0
  OUT=$($RUN "$HERE/henki-mods-dev" entry derive "file://$W/derived" "" main 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
  refused "\"main\" is not a full commit id"
  # test --port P DIR: a module directory not added.
  run test --port history "$W/nothere"
  refused "is not a program"
  if [ -n "${HENKI_BIN:-}" ]; then
    mkport tree
    run test --port history "$W/tree"
    rc_is 0
    out_has "history/1: 25 checks, 25 passed"
  fi
}

# --- updates, masks, assets, packs, profiles, the index verbs -------------------------------------------------------------

# The ID of a store copy dir: its last name.
idof() { printf '%s' "${1##*/}"; }

t_update() {
  case_ "outdated, update (the screen, the allow rules), pin, rollback (hd1 checks), gc, the history"
  local r="$W/up" s1 s2 n
  mkmod "$r"
  printf '# Changelog\n\n## 0.1.0\n- first\n' >"$r/CHANGELOG.md"
  mkrepo "$r"
  mkmod "$W/pm"
  run add "file://$r" --yes
  rc_is 0
  check "a URL install follows its default branch" [ "$(field up 11)" = ref=HEAD ]
  run add "$W/pm" --yes
  s1=$(readlink "$CONF/ext/up")
  run outdated
  rc_is 0
  check "all up to date" [ "$(printf '%s\n' "$OUT" | command grep -c 'up to date')" = 2 ]
  # A new version: a command more, a changed doc, a CHANGELOG section.
  setkey "$r/.henki/module.toml" version '"0.2.0"'
  mkdir -p "$r/cmd"
  printf '#!/bin/sh\necho new\n' >"$r/cmd/newcmd"
  chmod 755 "$r/cmd/newcmd"
  printf '# up: says hello for tests\n\nA longer body.\n' >"$r/doc.md"
  printf '# Changelog\n\n## 0.2.0\n- a command\n\n## 0.1.0\n- first\n' >"$r/CHANGELOG.md"
  commit "$r" second
  printf 'more\n' >>"$W/pm/doc.md"
  run outdated
  out_has "HEAD is at $(git -C "$r" rev-parse HEAD | cut -c 1-12) (henki-mods update up)"
  out_has "changed: $W/pm holds hd1:"
  mkdir -p "$CONF"
  printf '*\tup --loud\n/src\tgit status\n' >"$CONF/allow"
  chmod 600 "$CONF/allow"
  printf 'n\nn\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run update up
  refused "not updated (the answer was no)"
  out_has "up: 0.1.0 -> 0.2.0"
  out_has "commands - -> newcmd"
  out_has "files changed:"
  out_has "    cmd/newcmd +2 -0"
  out_has "    doc.md +1 -1"
  out_has "CHANGELOG.md, newest section:"
  out_has "    ## 0.2.0"
  out_lacks "    ## 0.1.0"
  out_has "allow rules that run its programs without asking: 1 (asked after the update)"
  check "nothing changed on a no" [ "$(readlink "$CONF/ext/up")" = "$s1" ] && [ ! -f "$CONF/mods.history" ]
  printf 'n\ny\nk\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run update up
  rc_is 0
  out_has "updated up: 0.1.0 -> 0.2.0"
  out_has "kept them"
  s2=$(readlink "$CONF/ext/up")
  check "the link moved to the new copy, the old one stays" [ "$s2" != "$s1" ] && [ -d "$s1" ] && [ -f "$s2/cmd/newcmd" ]
  check "the lock line" [ "$(field up 4):$(field up 7)" = "0.2.0:$(git -C "$r" rev-parse HEAD)" ]
  check "a hist/1 update line with the old lock line" [ "$(cut -f 3 "$CONF/mods.history")" = update ] && [ "$(cut -f 7 "$CONF/mods.history")" = 0.1.0 ]
  check "the rule kept" command grep -q 'up --loud' "$CONF/allow"
  run verify
  rc_is 0
  run update up
  rc_is 0
  out_has "up is up to date (0.2.0"
  run pin up
  rc_is 0
  check "pin" [ "$(field up 11)" = ref=HEAD,pin ]
  run update up --yes
  refused "up is pinned; fix: henki-mods unpin up, or henki-mods update up --force"
  run unpin up
  check "unpin" [ "$(field up 11)" = ref=HEAD ]
  # rollback: the pin before, its copy checked; the rules for the code that changed go by default.
  run rollback up --yes
  rc_is 0
  out_has "up: 0.2.0 -> 0.1.0 (the pin before update"
  out_has "rolled back up to 0.1.0"
  out_has "removed rule 1: *  up --loud"
  check "back on the old copy" [ "$(readlink "$CONF/ext/up")" = "$s1" ] && [ "$(field up 4)" = 0.1.0 ]
  check "the rollback line" [ "$(tail -n 1 "$CONF/mods.history" | cut -f 3,7)" = "rollback${tab}0.2.0" ]
  check "the other rule stays" [ "$(cat "$CONF/allow")" = "/src${tab}git status" ]
  # The copy rolled back to must hold its hd1: an edited one is skipped, and with none left the rollback refuses.
  chmod u+w "$s2/doc.md" && printf 'edited\n' >>"$s2/doc.md"
  run rollback up --yes
  refused "no earlier pin of up can be gone back to"
  out_has "skipped up 0.2.0"
  check "still 0.1.0" [ "$(field up 4)" = 0.1.0 ]
  run verify up
  rc_is 0
  # A path module updated three times: gc keeps the lock's copy and the two newest history pins.
  for n in 2 3 4; do
    printf 'v%s\n' "$n" >>"$W/pm/doc.md"
    run update pm --yes
    rc_is 0
  done
  check "four copies of pm" [ "$(ls "$STORE/pm" | wc -l | tr -d ' ')" = 4 ]
  mkdir -p "$XDG_DATA_HOME/henki/index/gone.git"
  printf 'n\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run gc
  refused "nothing removed (the answer was no)"
  out_has "gone.git"
  run gc --yes
  rc_is 0
  check "gc keeps three of pm" [ "$(ls "$STORE/pm" | wc -l | tr -d ' ')" = 3 ]
  check "the index clone no index uses is gone" [ ! -d "$XDG_DATA_HOME/henki/index/gone.git" ]
  check "up's copies are kept (lock and history)" [ -d "$s1" ] && [ -d "$s2" ]
  run gc --all --yes
  rc_is 0
  check "gc --all keeps the lock's copy only" [ "$(ls "$STORE/pm" | wc -l | tr -d ' ')" = 1 ] && [ ! -d "$s2" ] && [ -d "$s1" ]
  run gc
  out_has "nothing to remove"
  run verify
  rc_is 0
  run outdated
  out_has "up to date"
  case_ "update and outdated for an index package and an official module"
  ix_two
  run add hello --version 0.1.0 --yes
  rc_is 0
  run outdated
  out_has "0.2.0 in index deorlovnis (henki-mods update hello)"
  run update hello --to @main --yes
  refused "--to takes a version for a module from an index"
  run update hello --yes
  rc_is 0
  out_has "review   reviewed by someone (signed index henki-index, 2026-11-08)"
  check "hello 0.2.0 from the index" [ "$(field hello 4):$(field hello 5)" = "0.2.0:index:deorlovnis" ]
  run update hello --to 0.1.0 --yes
  rc_is 0
  check "back to 0.1.0 by --to" [ "$(field hello 4)" = 0.1.0 ]
  mkmod "$W/share/official/offi"
  run add offi --yes
  rc_is 0
  printf 'more\n' >>"$W/share/official/offi/doc.md"
  run outdated
  out_has "henki's copy changed (henki-mods update offi)"
  run update offi --yes
  rc_is 0
  run outdated
  check "official up to date" [ -n "$(printf '%s\n' "$OUT" | awk '$1 == "offi" && /up to date/')" ]
}

t_masks() {
  case_ "masks: off NAME/CMD and on NAME/CMD; allow rules on add, off and rm"
  local store link
  mkmod "$W/ma"
  mkdir -p "$W/ma/cmd"
  printf '#!/bin/sh\necho fork\n' >"$W/ma/cmd/fork"
  printf 'the fork template\n' >"$W/ma/cmd/fork.md"
  printf '#!/bin/sh\necho keep\n' >"$W/ma/cmd/keep"
  chmod 755 "$W/ma/cmd/fork" "$W/ma/cmd/keep"
  mkdir -p "$CONF"
  printf '*\tma x\n*\tls\n' >"$CONF/allow"
  chmod 600 "$CONF/allow"
  run add "$W/ma" --yes
  rc_is 0
  out_has "allow rules that run these programs without asking:"
  out_has "removed rule 1: *  ma x"
  check "the other rule stays" [ "$(cat "$CONF/allow")" = "*${tab}ls" ]
  store=$(readlink "$CONF/ext/ma")
  run off ma/fork
  rc_is 0
  out_has "ma/fork: off"
  link=$(readlink "$CONF/ext/ma")
  check "the link names a copy beside the store copy" [ "$link" != "$store" ] && [ "${link%.m*}" = "$store" ]
  check "without /fork, with /keep" [ ! -e "$link/cmd/fork" ] && [ ! -e "$link/cmd/fork.md" ] && [ -x "$link/cmd/keep" ]
  check "the flag, the module's own hd1" [ "$(field ma 11)" = mask=cmd/fork ] && [ "$(field ma 8)" = "$(cd "$W" && $RUN "$MODS" verify --dir ./ma | sed 's/.*, //')" ]
  check "the masked copy is read-only" [ -z "$(find "$link" ! -type l -perm -200 -print)" ]
  run off ma/fork
  out_has "ma/fork is already off"
  run off ma/nope
  refused "ma has no command /nope"
  run list
  out_has "(masked: /fork)"
  run verify
  rc_is 0
  run off ma
  run on ma
  rc_is 0
  check "on links the masked copy" [ "$(readlink "$CONF/ext/ma")" = "$link" ]
  chmod u+w "$link/cmd/keep" && printf 'x\n' >>"$link/cmd/keep"
  run verify
  refused "is not its store copy less the commands it masks"
  run off ma
  run on ma
  rc_is 0
  run verify
  rc_is 0
  out_has "verify: ok"
  run on ma/fork
  rc_is 0
  check "on NAME/CMD: the store copy again" [ "$(readlink "$CONF/ext/ma")" = "$store" ] && [ -z "$(field ma 11)" ]
  run verify
  rc_is 0
  # rm and off offer to drop the rules of programs no longer on.
  printf '*\tma y\n' >>"$CONF/allow"
  printf 'y\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run off ma
  rc_is 0
  out_has "allow rules for programs that are no longer on:"
  out_has "removed rule 2: *  ma y"
  printf '*\tma z\n' >>"$CONF/allow"
  printf 'n\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run rm ma
  rc_is 0
  out_has "kept them"
  check "kept" command grep -q 'ma z' "$CONF/allow"
  HENKI_MODS_TTY="$W/none" run add "$W/ma" --yes
  rc_is 0
  printf '*\tma w\n' >>"$CONF/allow"
  HENKI_MODS_TTY="$W/none" run rm ma
  out_has "henki-mods allow rm N removes one"
  if [ -n "${HENKI_BIN:-}" ]; then
    mkport tree
    printf '*\ttree --x\n' >"$CONF/allow"
    run add "$W/tree" --yes
    printf '*\ttree --x\n' >"$CONF/allow"
    run swap history tree --yes
    rc_is 0
    out_has "kept them"
    check "swap keeps a rule by default (its code did not change)" command grep -q 'tree --x' "$CONF/allow"
    # An update of a bound module: the new copy passes the suite, then the binding moves with it.
    printf 'more\n' >>"$W/tree/doc.md"
    run update tree --yes
    rc_is 0
    out_has "history: tree 0.1.0 passes the history/1 suite"
    check "the port link moved" [ "$(readlink "$CONF/port/history")" = "$(readlink "$CONF/ext/tree")/port/history" ]
    check "bind/1 names the new copy" [ "$(awk -F '\t' '$1 == "bind/1" { print $4 }' "$CONF/mods.lock")" = "$(idof "$(readlink "$CONF/ext/tree")")" ]
    run verify
    rc_is 0
    run rollback tree --yes
    rc_is 0
    check "the port link moved back" [ "$(readlink "$CONF/port/history")" = "$(readlink "$CONF/ext/tree")/port/history" ]
    run swap history std
  fi
}

# Serves W/www over https on 127.0.0.1 (a certificate made here, trusted through CURL_CA_BUNDLE): ASSET_URL, SRV.
asset_server() {
  mkdir -p "$W/www" "$W/tls"
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/tls/key.pem" -out "$W/tls/cert.pem" -days 2 -subj /CN=127.0.0.1 \
    -addext subjectAltName=IP:127.0.0.1 >/dev/null 2>&1 || return 1
  python3 -I -c '
import http.server, ssl, sys, os
d, t, pf = sys.argv[1:4]
os.chdir(d)
class H(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
c = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
c.load_cert_chain(t + "/cert.pem", t + "/key.pem")
s.socket = c.wrap_socket(s.socket, server_side=True)
open(pf + ".tmp", "w").write(str(s.server_address[1]))
os.rename(pf + ".tmp", pf)
s.serve_forever()
' "$W/www" "$W/tls" "$W/port" 2>/dev/null &
  SRV=$!
  local i=0
  while [ ! -f "$W/port" ] && [ $i -lt 100 ]; do sleep 0.1 && i=$((i + 1)); done
  [ -f "$W/port" ] || return 1
  ASSET_URL="https://127.0.0.1:$(cat "$W/port")"
  export CURL_CA_BUNDLE="$W/tls/cert.pem"
}

# This machine's asset target, as henki-mods names it.
my_target() {
  local m
  m=$(uname -m)
  [ "$m" != arm64 ] || m=aarch64
  case "$(uname -s)" in Darwin) echo "${m}_apple_darwin" ;; *) echo "${m}_unknown_linux_musl" ;; esac
}

t_assets() {
  case_ "assets: a prebuilt program by https and sha256 (right, wrong, missing target, http, a clash, edited), packed"
  if ! command -v python3 >/dev/null 2>&1 || ! command -v openssl >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
    echo "tests.sh: assets: skipped (no python3, openssl or curl)"
    return 0
  fi
  asset_server || {
    fail "the https server did not start"
    return 0
  }
  local t h store other
  t=$(my_target)
  case "$t" in aarch64_*) other=x86_64_apple_darwin ;; *) other=aarch64_apple_darwin ;; esac
  printf '#!/bin/sh\necho prebuilt\n' >"$W/www/tool"
  h=$(sha <"$W/www/tool")
  mkmod "$W/as" "asset_$t = [\"bin/tool\", \"$ASSET_URL/tool\", \"sha256:$h\"]\n"
  run verify --dir "$W/as"
  rc_is 0
  local want=${OUT##*, }
  run add "$W/as" --yes
  rc_is 0
  out_has "asset    bin/tool for $t: $ASSET_URL/tool, sha256:$h (downloaded, sha256 checked)"
  out_has "bin/tool ("
  store=$(readlink "$CONF/ext/as")
  check "the program in place" [ -x "$store/bin/tool" ] && [ "$("$store/bin/tool")" = prebuilt ]
  check "the lock's hd1 is the tree's (the asset left out)" [ "$(field as 8)" = "$want" ]
  run verify
  rc_is 0
  run off as
  run on as
  rc_is 0
  # Packed with its program, installed where curl is not.
  mkdir -p "$W/nocurl"
  printf '#!/bin/sh\necho called >"%s/curl-called"\nexit 7\n' "$W" >"$W/nocurl/curl"
  chmod 755 "$W/nocurl/curl"
  run pack as -o "$W/as.tgz"
  rc_is 0
  check "PACK holds the asset/1 line" sh -c "tar -xzOf '$W/as.tgz' PACK | command grep -q \"^asset/1${tab}as${tab}bin/tool${tab}sha256:$h\$\""
  local keep="$HOME"
  export HOME="$W/home2" XDG_DATA_HOME="$W/data2"
  mkdir -p "$HOME"
  PATH="$W/nocurl:$PATH" run unpack "$W/as.tgz" --yes
  rc_is 0
  check "installed from the pack, no curl" [ -x "$XDG_DATA_HOME/henki/mods/as/$(idof "$store")/bin/tool" ] && [ ! -f "$W/curl-called" ]
  run verify
  rc_is 0
  export HOME="$keep" XDG_DATA_HOME="$SCRATCH/$CASES/data"
  CONF="$HOME/.config/henki"
  chmod u+w "$store/bin/tool" && printf 'echo changed\n' >>"$store/bin/tool"
  run verify
  refused "not the prebuilt program its manifest names"
  # Refused at add: the wrong bytes, no build for here, an http URL, a file where the program goes.
  mkmod "$W/wrong" "asset_$t = [\"bin/tool\", \"$ASSET_URL/tool\", \"sha256:$(printf x | sha)\"]\n"
  run add "$W/wrong" --yes
  refused "$ASSET_URL/tool is not the program wrong names (sha256 $h"
  check "nothing added" [ -z "$(lockline wrong)" ]
  mkmod "$W/elsewhere" "asset_$other = [\"bin/tool\", \"$ASSET_URL/tool\", \"sha256:$h\"]\n"
  run add "$W/elsewhere" --yes
  refused "elsewhere runs a prebuilt program made for $other only, none for this machine"
  mkmod "$W/plain" "asset_$t = [\"bin/tool\", \"http://127.0.0.1:1/tool\", \"sha256:$h\"]\n"
  run add "$W/plain" --yes
  refused "asset_$t must be [\"bin/NAME\", \"https://...\", \"sha256:HEX\"]"
  local d
  for d in bin/.. bin/.; do
    mkmod "$W/dots" "asset_$t = [\"$d\", \"$ASSET_URL/tool\", \"sha256:$h\"]\n"
    run add "$W/dots" --yes
    refused "(NAME not . or .., an https URL"
  done
  mkmod "$W/clash" "asset_$t = [\"bin/tool\", \"$ASSET_URL/tool\", \"sha256:$h\"]\n"
  cp "$W/www/tool" "$W/clash/bin/tool" && chmod 755 "$W/clash/bin/tool"
  run add "$W/clash" --yes
  refused "bin/tool: a file of the module where its asset_$t goes"
  mkmod "$W/gone" "asset_$t = [\"bin/tool\", \"$ASSET_URL/nothere\", \"sha256:$h\"]\n"
  run add "$W/gone" --yes
  refused "cannot download $ASSET_URL/nothere"
  kill "$SRV" 2>/dev/null
  wait "$SRV" 2>/dev/null
  unset CURL_CA_BUNDLE
}

t_pack() {
  case_ "pack and unpack: a round trip with no network, the same bytes twice, an index bundle, hostile tars"
  ix_two
  run add hello --version 0.1.0 --yes
  rc_is 0
  mkmod "$W/pp"
  run add "$W/pp" --yes
  run off pp
  run pack --with-index -o "$W/a.tgz"
  rc_is 0
  out_has "packed 2 modules into $W/a.tgz"
  out_has "index deorlovnis: "
  run pack --with-index -o "$W/b.tgz"
  check "the same bytes twice" cmp -s "$W/a.tgz" "$W/b.tgz"
  check "the members" [ "$(tar -tzf "$W/a.tgz" | command grep -v '/$' | command grep -vc '^store/')" = 2 ]
  local ix_url keep_home="$HOME" keep_data="$XDG_DATA_HOME" share2="$W/share2"
  # Another machine: the same anchor, an index it cannot reach, no curl.
  mkdir -p "$share2/index" "$W/nocurl"
  cp "$W/share/index/"* "$share2/index/"
  printf 'file:///nonexistent/index\n' >"$share2/index/deorlovnis.url"
  printf '#!/bin/sh\necho called >"%s/curl-called"\nexit 7\n' "$W" >"$W/nocurl/curl"
  chmod 755 "$W/nocurl/curl"
  export HOME="$W/home2" XDG_DATA_HOME="$W/data2" HENKI_SHARE="$share2"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  PATH="$W/nocurl:$PATH" run unpack "$W/a.tgz" --yes
  rc_is 0
  out_has "index deorlovnis in the pack: "
  out_has "verified from the anchor here"
  out_has "review   reviewed by someone (signed index henki-index, 2026-11-08)"
  out_has "installed hello 0.1.0: on"
  out_has "installed pp 0.1.0: off"
  check "the lock lines" [ "$(field hello 5):$(field hello 3):$(field pp 3)" = "index:deorlovnis:on:off" ]
  check "no clone, no verified ref moved" [ ! -d "$XDG_DATA_HOME/henki/index/deorlovnis.git" ]
  check "no curl" [ ! -f "$W/curl-called" ]
  run verify
  rc_is 0
  run unpack "$W/a.tgz" --yes
  rc_is 0
  out_has "hello 0.1.0: already here, left as it is"
  out_has "nothing to install"
  # A bundle signed by a key the anchor here does not hold: not used, and with nothing synced here either the index
  # line cannot be checked, so it is refused; the path module alone goes in, unreviewed.
  export HOME="$W/home3" XDG_DATA_HOME="$W/data3"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  signer k2 >"$share2/index/deorlovnis.signers"
  run unpack "$W/a.tgz" --yes
  refused "hello: the pack says it comes from index deorlovnis, and nothing of index deorlovnis is here to check that against; fix: henki-mods sync, or pack it again with --with-index"
  out_has "not used: "
  check "nothing installed" [ ! -f "$CONF/mods.lock" ]
  repack "$W/a.tgz" "$W/a-pp.tgz" '$1 == "lock/1" && $2 == "hello" { next } { print }'
  run unpack "$W/a-pp.tgz" --yes
  rc_is 0
  out_has "UNREVIEWED: in no index you use"
  out_has "installed pp 0.1.0: off"
  # A malicious yank made after the pack, verified here: the pack's older bundle cannot undo it.
  signer k1 >"$share2/index/deorlovnis.signers"
  printf 'file://%s\n' "$IX" >"$share2/index/deorlovnis.url"
  setkey "$IX/mods/hello/0.1.0.toml" yanked '"malicious: steals keys (2026-11-08)"'
  ixc "$IX" yank
  export HOME="$W/home4" XDG_DATA_HOME="$W/data4"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  run sync
  rc_is 0
  run unpack "$W/a.tgz" --yes
  refused "hello 0.1.0 was yanked as malicious (steals keys (2026-11-08)) by index deorlovnis; not installed"
  out_has "no longer holds the verified commit"
  check "nothing installed" [ ! -f "$CONF/mods.lock" ]
  # The index takes the yank back in a newer signed commit, packed from a machine synced to it: the bundle is used, and
  # the malicious yank verified here still applies after it.
  unkey "$IX/mods/hello/0.1.0.toml" yanked
  ixc "$IX" unyank
  export HOME="$keep_home" XDG_DATA_HOME="$keep_data" HENKI_SHARE="$W/share"
  CONF="$HOME/.config/henki"
  run sync
  rc_is 0
  run pack --with-index -o "$W/c.tgz"
  rc_is 0
  export HOME="$W/home4" XDG_DATA_HOME="$W/data4" HENKI_SHARE="$share2"
  CONF="$HOME/.config/henki"
  run unpack "$W/c.tgz" --yes
  refused "hello 0.1.0 was yanked as malicious (steals keys (2026-11-08)) by index deorlovnis; not installed"
  out_has "verified from the anchor here"
  check "nothing installed" [ ! -f "$CONF/mods.lock" ]
  # Hostile archives: refused before anything is extracted.
  local k
  for k in abs dotdot under fifo hard outside notpack; do
    python3 -I -c '
import tarfile, sys, io
k, out = sys.argv[1:3]
t = tarfile.open(out, "w:gz", format=tarfile.USTAR_FORMAT)
def add(name, data=b"x", type=tarfile.REGTYPE, link=""):
    i = tarfile.TarInfo(name); i.type = type; i.linkname = link; i.mode = 0o644
    i.size = len(data) if type == tarfile.REGTYPE else 0
    t.addfile(i, io.BytesIO(data) if type == tarfile.REGTYPE else None)
add("PACK", b"pack/1\n")
if k == "abs": add("/tmp/henki-escape")
if k == "dotdot": add("store/a/0123456789ab/../../../escape")
if k == "under":
    add("store/a/0123456789ab/l/passwd")
    add("store/a/0123456789ab/l", type=tarfile.SYMTYPE, link="/etc")
if k == "fifo": add("store/a/0123456789ab/f", type=tarfile.FIFOTYPE)
if k == "hard": add("store/a/0123456789ab/h", type=tarfile.LNKTYPE, link="PACK")
if k == "outside": add("l", type=tarfile.SYMTYPE, link="store")
if k == "notpack": add("etc/x")
t.close()
' "$k" "$W/bad-$k.tgz"
    run unpack "$W/bad-$k.tgz" --yes
    refused "fix: do not unpack it"
  done
  check "nothing escaped" [ ! -e /tmp/henki-escape ] && [ ! -e "$W/escape" ] && [ ! -f "$CONF/mods.lock" ]
  export HOME="$keep_home" XDG_DATA_HOME="$keep_data"
}

# Pack SRC again as DST with its PACK put through the awk program PROG (tab-separated fields), and store copy
# store/NAME/FROM renamed to TO when given.
repack() { # SRC DST PROG [NAME FROM TO]
  local d="$W/repack"
  chmod -R u+w "$d" 2>/dev/null
  rm -rf "$d" && mkdir -p "$d" && (cd "$d" && tar -xzf "$1") || fail "repack: cannot extract $1"
  awk -F '\t' 'BEGIN { OFS = "\t" } '"$3" "$d/PACK" >"$d/PACK.new" && mv -f "$d/PACK.new" "$d/PACK"
  [ -z "${4:-}" ] || mv "$d/store/$4/$5" "$d/store/$4/$6"
  (cd "$d" && tar -czf "$2" PACK store) || fail "repack: cannot write $2"
}

t_pack_lines() {
  case_ "unpack and init check what a line claims: its kind, its commit, its characters; removed packages refuse"
  ix_two
  run add hello --version 0.1.0 --yes
  rc_is 0
  mkmod "$W/o/clip"
  run add "$W/o/clip" --yes
  rc_is 0
  run pack hello -o "$W/h.tgz"
  rc_is 0
  run pack clip -o "$W/c.tgz"
  rc_is 0
  run profile save "$W/p.lock"
  rc_is 0
  local id k
  id=$(field hello 7 | cut -c 1-12)
  setkey "$IX/mods/hello/0.1.0.toml" yanked '"malicious: steals keys (2026-11-08)"'
  ixc "$IX" yank
  export HOME="$W/home2" XDG_DATA_HOME="$W/data2"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  run sync
  rc_is 0
  # A yanked package relabelled: as henki's own, as an index this machine does not use, as a directory.
  repack "$W/h.tgz" "$W/h-official.tgz" '$1 == "lock/1" { $5 = "official" } { print }'
  run unpack "$W/h-official.tgz" --yes
  refused "hello: the pack says it is henki's official hello, and this henki ships no hello"
  repack "$W/h.tgz" "$W/h-other.tgz" '$1 == "lock/1" { $5 = "index:other" } { print }'
  run unpack "$W/h-other.tgz" --yes
  refused "hello: the pack says it comes from index other, which you do not use"
  repack "$W/h.tgz" "$W/h-path.tgz" '$1 == "lock/1" { $5 = "path"; $6 = "/elsewhere/hello" } { print }'
  run unpack "$W/h-path.tgz" --yes
  refused "hello 0.1.0 was yanked as malicious"
  # The same package under the index's name, but another version than its files are.
  repack "$W/h.tgz" "$W/h-ver.tgz" '$1 == "lock/1" { $4 = "0.2.0" } { print }'
  run unpack "$W/h-ver.tgz" --yes
  refused "hello: index deorlovnis here does not list hello 0.2.0 with these files"
  check "nothing installed" [ ! -f "$CONF/mods.lock" ] && [ ! -d "$XDG_DATA_HOME/henki/mods/hello" ]
  # Other code named as henki's official module of that name, which is installed: never replaced.
  run add clip --yes
  rc_is 0
  repack "$W/c.tgz" "$W/c-official.tgz" '$1 == "lock/1" { $5 = "official"; $6 = "clip" } { print }'
  run unpack "$W/c-official.tgz" --yes
  refused "clip: the pack says it is henki's official clip, and this henki's clip is other files"
  check "the official clip stays" [ "$(field clip 5)" = official ]
  # A commit that walks out of the store, a control character, an hd1 that is not one, a relative path.
  repack "$W/h.tgz" "$W/h-dots.tgz" '$1 == "lock/1" { $7 = "../../index/" } { print }' hello "$id" index
  run unpack "$W/h-dots.tgz" --yes
  refused "its PACK is not lock/1 lines this henki-mods reads"
  check "no index dir made" [ ! -e "$XDG_DATA_HOME/henki/index/.henki" ]
  for k in '$6 = $6 "\033[2J"' '$8 = "hd1:../x"' '$9 = $9 "\r"' '$5 = "path"; $6 = "elsewhere/hello"'; do
    repack "$W/h.tgz" "$W/h-bad.tgz" '$1 == "lock/1" { '"$k"' } { print }'
    run unpack "$W/h-bad.tgz" --yes
    refused "its PACK is not lock/1 lines this henki-mods reads"
  done
  # A profile naming a package the index removed as malicious: refused as add refuses it.
  unkey "$IX/mods/hello/0.1.0.toml" yanked
  printf 'hello = "malicious: steals keys"\n' >>"$IX/removed.toml"
  ixc "$IX" removed
  run sync
  rc_is 0
  run init "$W/p.lock" --yes
  refused "hello 0.1.0 was yanked as malicious (steals keys) by index deorlovnis; not installed"
  check "nothing added" [ -z "$(lockline hello)" ]
  # Other code labelled as the index's package, on a machine that has not synced: nothing to check it against, so
  # refused (else list, verify and outdated would call it the index's after the next sync).
  export HOME="$W/home3" XDG_DATA_HOME="$W/data3"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  mkmod "$W/x/hello"
  printf 'other code\n' >>"$W/x/hello/doc.md"
  mkdir -p "$W/x/hello/cmd"
  printf '#!/bin/sh\necho greet\n' >"$W/x/hello/cmd/greet"
  chmod 755 "$W/x/hello/cmd/greet"
  run add "$W/x/hello" --yes
  rc_is 0
  run off hello/greet
  rc_is 0
  run pack hello -o "$W/x.tgz"
  rc_is 0
  export HOME="$W/home4" XDG_DATA_HOME="$W/data4"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  repack "$W/x.tgz" "$W/x-ix.tgz" '$1 == "lock/1" { $4 = "0.2.0"; $5 = "index:deorlovnis"; $6 = "hello" } { print }'
  run unpack "$W/x-ix.tgz" --yes
  refused "hello: the pack says it comes from index deorlovnis, and nothing of index deorlovnis is here to check that against"
  # A mask of a command the copy does not have.
  repack "$W/x.tgz" "$W/x-mask.tgz" '$1 == "lock/1" { $11 = "mask=cmd/..,pin" } { print }'
  run unpack "$W/x-mask.tgz" --yes
  refused "hello: the pack masks /.., which its copy has no command of"
  check "nothing installed" [ ! -f "$CONF/mods.lock" ]
  # Its own mask goes along.
  run unpack "$W/x.tgz" --yes
  rc_is 0
  check "the mask kept" [ "$(field hello 11)" = mask=cmd/greet ] && [ ! -e "$(readlink "$CONF/ext/hello")/cmd/greet" ]
  run verify
  rc_is 0
}

# Shipped profiles in W/share/mod-profiles (HENKI_SHARE): official modules sessions (no prompt bytes) and tooly.
mkprofiles() {
  mkdir -p "$W/share/mod-profiles" "$W/share/official/sessions/cmd" "$W/share/index"
  printf '# sessions: for tests\n' >"$W/share/official/sessions/doc.md"
  printf '#!/bin/sh\n' >"$W/share/official/sessions/cmd/sessions"
  chmod 755 "$W/share/official/sessions/cmd/sessions"
  mkmod "$W/share/official/tooly"
  printf 'lock/1\tsessions\ton\t-\tofficial\tsessions\t-\t-\t-\t-\t\n' >"$W/share/mod-profiles/minimal.lock"
  printf 'lock/1\tsessions\ton\t-\tofficial\tsessions\t-\t-\t-\t-\t\nlock/1\ttooly\ton\t-\tofficial\ttooly\t-\t-\t-\t-\t\nlock/1\tlater\ton\t-\tofficial\tlater\t-\t-\t-\t-\t\n' >"$W/share/mod-profiles/starter.lock"
  export HENKI_SHARE="$W/share"
}

t_init() {
  case_ "init: the offer, a profile (tombstones, bytes, a module this henki lacks), profile list, show, save"
  mkprofiles
  local b h='Extensions are commands on PATH; read the doc in brackets before first use.'
  b=$($RUN "$MODS" verify --dir "$W/share/official/tooly" | sed 's/.*ok: tooly 0.1.0, \([0-9,]*\) B of prompt.*/\1/' | tr -d ,)
  run list
  out_has "henki-mods init offers sets of them"
  run init
  rc_is 0
  check "minimal: 0 B" [ -n "$(printf '%s\n' "$OUT" | awk '$1 == "minimal" && $2 == "0" && $4 == "sessions"')" ]
  check "starter: its bytes with the header" [ -n "$(printf '%s\n' "$OUT" | HM_B="$(num $((b + ${#h} + 2)))" awk '$1 == "starter" && $2 == ENVIRON["HM_B"] && $4 == "sessions" && $5 == "tooly" && $6 == "later"')" ]
  run add sessions --yes
  run off sessions
  printf 'n\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run init starter
  refused "nothing added (the answer was no)"
  out_has "  sessions: off in your lock, so it stays off"
  out_has "  later: this henki ships no later; left out"
  out_has "  tooly 0.1.0 (official): +$b B of prompt"
  check "nothing added on a no" [ -z "$(lockline tooly)" ]
  run init starter --yes
  rc_is 0
  out_has "added tooly: on"
  check "tooly on, sessions still off" [ "$(field tooly 3):$(field sessions 3)" = "on:off" ]
  run init starter --yes
  out_has "nothing to add"
  run init nothere
  refused "no profile nothere"
  run profile list
  rc_is 0
  out_has "minimal"
  run profile show starter
  rc_is 0
  out_has "tooly"
  run profile save "$W/mine.lock"
  rc_is 0
  out_has "wrote $W/mine.lock (1 modules)"
  check "the saved line" [ "$(cat "$W/mine.lock")" = "lock/1${tab}tooly${tab}on${tab}-${tab}official${tab}tooly${tab}-${tab}-${tab}-${tab}-${tab}" ]
  run profile save "$W/mine.lock"
  refused "exists"
  export HOME="$W/home2"
  CONF="$HOME/.config/henki"
  mkdir -p "$HOME"
  run init "$W/mine.lock" --yes
  rc_is 0
  check "from the saved file" [ "$(field tooly 3)" = on ]
  printf 'lock/1\tx\ton\t-\tpath\t/x\t-\t-\t-\t-\t\n' >"$W/bad.lock"
  run init "$W/bad.lock" --yes
  refused "a profile holds official modules and index packages only"
  # The profiles shipped in the repository read as profiles, and name official modules.
  unset HENKI_SHARE
  run init
  rc_is 0
  out_has "minimal"
  out_has "starter"
  out_has "mac"
}

t_index_verbs() {
  case_ "index list, add, rm; the trust-changing verbs refuse inside a tool call"
  ix_two
  local main="$IX" v
  run index list
  rc_is 0
  out_has "index deorlovnis: file://$main"
  out_has "verified   never"
  mkindex "$W/extra"
  signer k1 >"$W/extra.signers"
  signer k2 >"$W/k2.signers"
  run index add extra "file://$W/extra" --signers "$W/k2.signers" --yes
  refused "(index extra was not added)"
  check "not added" [ ! -f "$CONF/mods.index" ] && [ ! -f "$CONF/index/extra.signers" ]
  printf 'n\n' >"$W/answer"
  HENKI_MODS_TTY="$W/answer" run index add extra "file://$W/extra" --signers "$W/extra.signers"
  refused "not added (the answer was no)"
  out_has "  signer   henki-index ssh-ed25519 SHA256:"
  out_has "no checkpoint"
  run index add extra "file://$W/extra" --signers "$W/extra.signers" --checkpoint "$(git -C "$W/extra" rev-parse HEAD)" --yes
  rc_is 0
  out_has "added index extra"
  check "mods.index and the anchor" [ "$(cat "$CONF/mods.index")" = "index/1${tab}extra${tab}file://$W/extra" ] && cmp -s "$W/extra.signers" "$CONF/index/extra.signers"
  run index list
  out_has "index extra: file://$W/extra"
  out_has "checkpoint $(git -C "$W/extra" rev-parse HEAD)"
  run index add extra "file://$W/extra" --signers "$W/extra.signers" --yes
  refused "index extra is in use already"
  run index add deorlovnis "file://$W/extra" --signers "$W/extra.signers" --yes
  refused "index deorlovnis is in use already"
  run index add x2 "http://example.invalid/x" --signers "$W/extra.signers" --yes
  refused "is not an https, ssh or file URL"
  run index rm deorlovnis --yes
  refused "index deorlovnis ships with henki"
  run index rm extra --yes
  rc_is 0
  check "gone from mods.index" [ ! -f "$CONF/mods.index" ] && [ ! -f "$CONF/index/extra.signers" ]
  run gc --yes
  rc_is 0
  check "gc removes its clone" [ ! -d "$XDG_DATA_HOME/henki/index/extra.git" ]
  # Inside a tool call (a session id, no HENKI_NEXT) the verbs that change what henki runs refuse.
  mkmod "$W/hello2"
  run add "$W/hello2" --yes
  printf 'y\n' >"$W/answer"
  for v in "update hello2 --yes" "rollback hello2 --yes" "unpack $W/x.tgz --yes" "init minimal --yes" "index add e2 file://$W/extra --signers $W/extra.signers --yes" "on hello2/x"; do
    # shellcheck disable=SC2086 # the verb and its arguments
    HENKI_SESSION=20261108T100000Z-abcd HENKI_MODS_TTY="$W/answer" run $v
    refused "this changes what henki runs, and it runs inside a tool call; fix: run it at the prompt with /mods"
  done
}

all() {
  t_add_path
  t_ask
  t_git
  t_raw_blobs
  t_hd1
  t_refusals
  t_gitattributes
  t_transport
  t_case_clash
  t_awk_data
  t_lock_newer
  t_onoff
  t_verify
  t_list_bytes
  t_cards
  t_shadow
  t_official
  t_halti
  t_allow
  t_ext2
  t_observe
  t_restart
  t_ports
  t_port_toy
  t_policy
  t_eval
  t_index_sync
  t_index_long
  t_index_add
  t_index_yank
  t_index_guard
  t_update
  t_masks
  t_assets
  t_pack
  t_pack_lines
  t_init
  t_index_verbs
}

static_bash32
static_contracts
static_formats
RUN=bash
all
t_live
t_halti_live
t_eval_live
t_general
# POSIX mode turns off some bash behaviour a script may lean on without knowing; the manager must not.
RUN="bash --posix"
all
if [ "$FAILS" != 0 ]; then
  echo "tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "tests.sh: $CASES cases passed"
