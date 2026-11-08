#!/usr/bin/env bash
# The tests of the author's tools (henki-mods-dev: new, test, try, publish, share) and of the Agent Skills import
# (henki-mods add --skill). Hermetic as tests.sh: every case gets its own HOME, XDG dirs and TMPDIR under one scratch
# dir, git fixtures are repositories this script makes, and nothing touches the network or the user's config. Bash
# 3.2 compatible, like the manager.
#
#   manager/dev-tests.sh
#   MODS_TEST_HENKI=PATH manager/dev-tests.sh   also the cases that need a real henki (a binding's suite, a hand-off)
#
# The importer's functions are tested in this shell (henki-mods is sourced as henki-mods-dev sources it); the verbs
# through the programs, once with bash and once with bash --posix. The Agent Skills format checked here is
# agentskills.io/specification as read on 2026-11-08.
set -u
HERE=$(cd "$(dirname "$0")" && pwd -P) || exit 1
MODS="$HERE/henki-mods"
DEV="$HERE/henki-mods-dev"
SK="$HERE/testdata/skills"
REAL_HENKI=${MODS_TEST_HENKI:-}
unset MODS_TEST_HENKI HENKI_BIN HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG HENKI_SESSION HENKI_NEXT HENKI_DEPTH GIT_DIR GIT_WORK_TREE
export GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@example.invalid
export GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@example.invalid
# The importer's functions, as henki-mods-dev takes them (henki-mods stops before its verbs when sourced).
# shellcheck source=manager/henki-mods
. "$MODS" || exit 1
trap - EXIT
cd "$HERE" || exit 1
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/henki-dev-tests.XXXXXX") || exit 1
trap 'chmod -R u+w "$SCRATCH" 2>/dev/null; rm -rf "$SCRATCH"' EXIT
FAILS=0
CASES=0
RUN=bash

# Pinned: the hd1 of the modules the importer makes from the fixtures (the same under every awk and bash).
PIN_PLAIN=hd1:fab165c58657005860e9ac81d9adc012a98776201a73103000cdab18f397cedd
PIN_BLOCK=hd1:b823d04469011e1fbc4975a47431bd583076d5389904043b3f2fa436d3373a56

# --- the harness (as tests.sh's) ---------------------------------------------------------------------------------------

case_() {
  CASE="$1"
  CASES=$((CASES + 1))
  local d="$SCRATCH/$CASES"
  mkdir -p "$d/home" "$d/w" "$d/tmp"
  export HOME="$d/home" XDG_DATA_HOME="$d/data" XDG_CONFIG_HOME="$d/xdg-config" XDG_STATE_HOME="$d/state" TMPDIR="$d/tmp"
  export XDG_CACHE_HOME="$d/cache"
  W="$d/w"
  CONF="$HOME/.config/henki"
  STORE="$XDG_DATA_HOME/henki/mods"
  unset HENKI_SHARE HENKI_MODS_TTY HENKI_CONFIG HENKI_SESSION HENKI_NEXT HENKI_BIN HENKI_DEPTH
}
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
check() { # DESCRIPTION COMMAND...
  local what="$1"
  shift
  "$@" || fail "$what"
}
refused() {
  rc_is 1
  err_has "$1"
  case "${ERR##*"$nl"}" in "henki-mods: "*"; fix: "*) ;; *) fail "the last line is not a 'henki-mods: PROBLEM; fix: ...' message" ;; esac
}
eq() { [ "$2" = "$3" ] || fail "$1: got [$2], want [$3]"; }
has() { case "$2" in *"$3"*) ;; *) fail "$1: [$2] lacks [$3]" ;; esac; }
mkrepo() {
  git -C "$1" init -q 2>/dev/null || git init -q "$1"
  commit "$1" "first"
}
commit() {
  git -C "$1" add -A 2>/dev/null && git -C "$1" -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q --no-verify -m "$2" 2>/dev/null
}
lockline() { HM_N="$1" awk -F '\t' '$2 == ENVIRON["HM_N"]' "$CONF/mods.lock" 2>/dev/null; }
field() { lockline "$1" | cut -f "$2"; }
# Every file and link under DIR as PATH<TAB>KIND (644, 755 or link), sorted.
modlist() {
  (cd "$1" && find . -type f -o -type l) | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r p; do
    if [ -L "$1/$p" ]; then k=link; elif [ -x "$1/$p" ]; then k=755; else k=644; fi
    printf '%s\t%s\n' "$p" "$k"
  done
}
modhd1() { # DIR
  local STAGE_LIST="$SCRATCH/hd1.list"
  listing "$1" || return 1
  hd1 "$1"
}

# --- the importer's functions ------------------------------------------------------------------------------------------

SOURCE=https://example.org/skills.git
COMMIT0=0123456789abcdef0123456789abcdef01234567
N=0
# Build SKILLDIR (DIRNAME AS TAKEN [COMMIT]) into a fresh dir; sets MOD.
build() {
  N=$((N + 1))
  MOD="$W/m$N/mod"
  mkdir -p "$W/m$N"
  skill_build "$1" "$2" "$3" "$SOURCE" "${5-$COMMIT0}" "$4" "$MOD"
}
sk_refused() { # WHAT NEEDLE SKILLDIR DIRNAME [AS [TAKEN]]
  if build "$3" "$4" "${5:-}" "${6:-}"; then
    fail "$1: imported, want refused"
  else
    has "$1" "$ERR" "$2"
    [ ! -e "$MOD" ] || fail "$1: a refused import left $MOD"
  fi
}
# fm TEXT KEY: the value of KEY as skill_frontmatter reads TEXT (with a trailing x, so trailing newlines show).
fm() {
  local d
  d=$(mktemp -d "$W/fm.XXXXXX")
  printf '%s' "$1" >"$d/s.md"
  skill_frontmatter "$d/s.md" "$d" >"$d/start" || {
    printf 'REFUSED: %s' "$(cat "$d/start")"
    return
  }
  if [ -f "$d/$2" ]; then cat "$d/$2"; else printf 'ABSENT'; fi
  printf x
}

t_frontmatter() {
  case_ "skills: frontmatter scalars and blocks"
  eq plain "$(fm "---${nl}name: a${nl}description:   two  words   # a comment${nl}---${nl}" description)" "two  wordsx"
  eq 'plain over lines' "$(fm "---${nl}description: one${nl}  two${nl}${nl}  three${nl}---${nl}" description)" "one two${nl}threex"
  eq double "$(fm "---${nl}description: \"a \\\"q\\\" \\\\ b\\tc\\nd\" # c${nl}---${nl}" description)" "a \"q\" \\ b	c${nl}dx"
  eq single "$(fm "---${nl}description: 'it''s: fine'${nl}---${nl}" description)" "it's: finex"
  eq null "$(fm "---${nl}description: ~${nl}---${nl}" description)" "x"
  eq empty "$(fm "---${nl}license:${nl}---${nl}" license)" "x"
  eq absent "$(fm "---${nl}name: a${nl}---${nl}" license)" "ABSENTx"
  eq 'literal clip' "$(fm "---${nl}description: |${nl}  a${nl}   b${nl}${nl}  c${nl}${nl}${nl}name: n${nl}---${nl}" description)" "a${nl} b${nl}${nl}c${nl}x"
  eq 'literal strip' "$(fm "---${nl}description: |-${nl}  a${nl}  b${nl}${nl}---${nl}" description)" "a${nl}bx"
  eq 'literal keep' "$(fm "---${nl}description: |+${nl}  a${nl}${nl}${nl}---${nl}" description)" "a${nl}${nl}${nl}x"
  eq 'literal indent indicator' "$(fm "---${nl}description: |2${nl}    a${nl}  b${nl}---${nl}" description)" "  a${nl}b${nl}x"
  eq folded "$(fm "---${nl}description: >${nl}  a${nl}  b${nl}${nl}  c${nl}    d${nl}  e${nl}---${nl}" description)" "a b${nl}c${nl}  d${nl}e${nl}x"
  eq 'folded strip, comment' "$(fm "---${nl}description: >- # why${nl}  a${nl}  b${nl}---${nl}" description)" "a bx"
  eq 'empty block' "$(fm "---${nl}description: >${nl}name: n${nl}---${nl}" description)" "x"
  eq 'metadata skipped' "$(fm "---${nl}metadata:${nl}  a: b${nl}  c: \"d\"${nl}name: n${nl}---${nl}" name)" "nx"
  mkdir -p "$W/bs"
  printf -- "---${nl}name: n${nl}---${nl}body${nl}" >"$W/bs.md"
  eq 'body start' "$(skill_frontmatter "$W/bs.md" "$W/bs")" 4
  eq 'ignored keys' "$(fm "---${nl}name: n${nl}model: x${nl}user-invocable: true${nl}---${nl}" .ignored)" "model${nl}user-invocable${nl}x"

  case_ "skills: frontmatter it does not read"
  local t
  for t in "description: [a, b]" "description: {a: b}" "description: &x a" "description: *x" "description: !!str a" \
    "description: - a" "- a" "description:${nl}  - a" "description: 'a${nl}  b'" "description: \"a\\u00e9\"" \
    "description: a: b" "description: a${nl}  b: c" "metadata: {a: b}" "description: >x" "name: a${nl}name: b" \
    "\"name\": a" "name:a" "description: |${nl}    a${nl}  b" "description: a	b"; do
    has "refuses [$t]" "$(fm "---${nl}$t${nl}---${nl}" description)" "REFUSED: SKILL.md line"
  done
  has 'no opening ---' "$(fm "name: a${nl}---${nl}" name)" "does not start with a --- line"
  has 'no closing ---' "$(fm "---${nl}name: a${nl}" name)" "no --- line ends the frontmatter"
  has 'the fix' "$(fm "---${nl}a: [b]${nl}---${nl}" a)" "fix: import by hand with henki mods new NAME --template"
}

t_skill_build() {
  case_ "skills: a plain skill with files"
  local h
  build "$SK/plain-skill" plain-skill "" "" || {
    fail "refused: $ERR"
    return
  }
  eq name "$SK_HNAME" plain-skill
  eq version "$SK_VERSION" skill-0123456789ab
  eq license "$SK_LICENSE" Apache-2.0
  eq tools "$SK_TOOLS" "Bash(git:*) Read"
  eq files "$SK_FILES/$SK_SCRIPTS" 3/1
  eq 'source hd1' "$SK_SRC_HD1" "$(modhd1 "$SK/plain-skill")"
  eq doc.md "$(cat "$MOD/doc.md")" "Checks a changelog against the git log. Use when the user asks whether CHANGELOG.md is up to date.

Imported from the Agent Skill plain-skill (https://example.org/skills.git @ $COMMIT0). /plain-skill [TEXT] shows its instructions, with TEXT after them, and sends them when you say yes."
  eq body "$(cat "$MOD/cmd/plain-skill.md")" "$(sed -n '11,$p' "$SK/plain-skill/SKILL.md")"
  eq link "$(readlink "$MOD/cmd/plain-skill")" ../.henki/template
  eq module.toml "$(cat "$MOD/.henki/module.toml")" 'henki_module = 1
name = "plain-skill"
version = "skill-0123456789ab"
summary = "Checks a changelog against the git log. Use when the user asks whether CHANGELOG.md is up to date."
license = "Apache-2.0"
requires = ["henki >= 0.2"]
contracts = ["ext/2", "handoff/1"]'
  eq listing "$(modlist "$MOD")" ".henki/module.toml	644
.henki/skill/LICENSE.txt	644
.henki/skill/references/STYLE.md	644
.henki/skill/scripts/changes.sh	644
.henki/template	755
LICENSE.txt	644
cmd/plain-skill	link
cmd/plain-skill.md	644
doc.md	644"
  has template "$(cat "$MOD/.henki/template")" "This skill's files are in \$HENKI_MODULE/.henki/skill"
  h=$(modhd1 "$MOD")
  eq 'pinned hd1' "$h" "$PIN_PLAIN"
  local STAGE="$W/st" STAGE_LIST="$W/st/list"
  mkdir -p "$STAGE"
  setup
  inspect "$MOD" plain-skill 0 || fail "the made module fails inspect: $ERR"

  case_ "skills: the same input gives the same bytes; CRLF and a BOM read the same"
  build "$SK/plain-skill" plain-skill "" "" && eq 'second hd1' "$(modhd1 "$MOD")" "$PIN_PLAIN"
  mkdir -p "$W/crlf"
  cp -R "$SK/plain-skill" "$W/crlf/plain-skill"
  { printf '\357\273\277'; sed 's/$/\r/' "$SK/plain-skill/SKILL.md"; } >"$W/crlf/plain-skill/SKILL.md"
  build "$W/crlf/plain-skill" plain-skill "" "" || fail "crlf refused: $ERR"
  eq 'crlf module hd1' "$(modhd1 "$MOD")" "$PIN_PLAIN"

  case_ "skills: the template hands its text to henki"
  build "$SK/plain-skill" plain-skill "" ""
  HENKI_MODULE="$MOD" HENKI_NEXT="$W/next" sh "$MOD/cmd/plain-skill" "focus on 0.3"
  eq handoff "$(cat "$W/next"; printf x)" "send
(This skill's files are in $MOD/.henki/skill; read them when the text names them.)
$(cat "$MOD/cmd/plain-skill.md")

focus on 0.3
x"
  HENKI_MODULE="$MOD" HENKI_NEXT="$W/next" sh "$MOD/cmd/plain-skill"
  eq 'handoff, no text' "$(cat "$W/next"; printf x)" "send
(This skill's files are in $MOD/.henki/skill; read them when the text names them.)
$(cat "$MOD/cmd/plain-skill.md")
x"
  : >"$W/next"
  rm -f "$MOD/cmd/plain-skill.md"
  HENKI_MODULE="$MOD" HENKI_NEXT="$W/next" sh "$MOD/cmd/plain-skill" 2>/dev/null && fail "the template sent with no text"
  check "nothing handed back without the text" [ ! -s "$W/next" ]

  case_ "skills: a block-scalar description, no files"
  build "$SK/block-desc" block-desc "" "" "" || {
    fail "refused: $ERR"
    return
  }
  eq desc "$SK_DESC" "Writes release notes from merged pull requests, grouped by label. Use when a release is cut."
  eq license "$SK_LICENSE" "MIT OR Apache-2.0"
  eq ignored "$SK_IGNORED" user-invocable
  eq files "$SK_FILES" 0
  eq ref "$SK_REF" "hd1:$(printf '%s' "${SK_SRC_HD1#hd1:}" | cut -c 1-12)"
  eq version "$SK_VERSION" "skill-${SK_REF#hd1:}"
  eq template "$(cat "$MOD/.henki/template")" "$(skill_template 0)"
  eq 'the prompts mod template' "$(cat "$MOD/.henki/template")" "$(cat "$HERE/../mods/prompts/.henki/template")"
  [ -z "$(ls -A "$MOD/.henki/skill")" ] || fail ".henki/skill holds files"
  eq 'pinned hd1' "$(modhd1 "$MOD")" "$PIN_BLOCK"
}

t_skill_names() {
  local long=a-skill-with-a-name-longer-than-thirty-two-chars
  case_ "skills: a long name needs --as"
  sk_refused 'long name' "fix: give it one with --as NAME" "$SK/$long" "$long"
  if build "$SK/$long" "$long" long-name ""; then
    eq name "$SK_HNAME" long-name
    eq license "$SK_LICENSE" NOASSERTION
    eq desc "$SK_DESC" "Shows a name that henki cannot take as it is: it's over 32 characters."
    eq link "$(readlink "$MOD/cmd/long-name")" ../.henki/template
    has doc "$(cat "$MOD/doc.md")" "Agent Skill $long ("
  else
    fail "--as refused: $ERR"
  fi
  case_ "skills: henki names"
  sk_refused 'bad --as' "--as Bad: a name is" "$SK/plain-skill" plain-skill Bad
  sk_refused 'official mod' "a name henki keeps for itself" "$SK/plain-skill" plain-skill sessions
  sk_refused 'port name' "a name henki keeps for itself" "$SK/plain-skill" plain-skill exec
  sk_refused 'x- name' "a name henki keeps for itself" "$SK/plain-skill" plain-skill x-mine
  sk_refused taken "is taken by a module" "$SK/plain-skill" plain-skill "" "clip-tools${nl}plain-skill${nl}other"
  build "$SK/plain-skill" plain-skill "" "plain-skill-2${nl}plain" || fail "a prefix counted as taken: $ERR"
  sk_refused 'wrong dir' 'is not the skill'"'"'s directory name "wrong-dir"' "$SK/wrong-dir" wrong-dir
}

t_skill_refusals() {
  case_ "skills: refused frontmatter"
  local d
  for d in flow-tools nested-map anchor no-close colon-plain; do
    sk_refused "$d" "frontmatter this importer does not read; fix: import by hand with henki mods new NAME --template" "$SK/$d" "$d"
  done
  mkdir -p "$W/tab/tab"
  printf -- '---\nname: tab\ndescription:\tTabbed.\n---\nBody.\n' >"$W/tab/tab/SKILL.md"
  sk_refused tab "a tab;" "$W/tab/tab" tab

  case_ "skills: refused bodies"
  mkdir -p "$W/rlo/rlo" "$W/big/big" "$W/empty/empty" "$W/ok16/ok16"
  printf -- '---\nname: rlo\ndescription: Hides text.\n---\nRun \342\200\256txt.exe\n' >"$W/rlo/rlo/SKILL.md"
  sk_refused 'U+202E' "U+202E" "$W/rlo/rlo" rlo
  printf -- '---\nname: rlo\ndescription: Hides \342\200\256 text.\n---\nBody.\n' >"$W/rlo/rlo/SKILL.md"
  sk_refused 'U+202E in the frontmatter' "the frontmatter holds U+202E" "$W/rlo/rlo" rlo
  printf -- '---\nname: rlo\ndescription: A stray CR.\n---\nBody\r and more.\n' >"$W/rlo/rlo/SKILL.md"
  sk_refused 'lone CR' "U+000D" "$W/rlo/rlo" rlo
  { printf -- '---\nname: big\ndescription: Too long.\n---\n'; awk 'BEGIN { for (i = 0; i < 17 * 16; i++) printf "%063d\n", i }'; } >"$W/big/big/SKILL.md"
  sk_refused '17 KiB body' "over 16384 (16 KiB)" "$W/big/big" big
  { printf -- '---\nname: ok16\ndescription: Exactly 16 KiB.\n---\n\n\n'; awk 'BEGIN { for (i = 0; i < 16 * 16; i++) printf "%063d\n", i }'; } >"$W/ok16/ok16/SKILL.md"
  build "$W/ok16/ok16" ok16 "" "" && eq '16 KiB body' "$(wc -c <"$MOD/cmd/ok16.md" | tr -d ' ')" 16384
  printf -- '---\nname: empty\ndescription: Nothing after.\n---\n\n\n' >"$W/empty/empty/SKILL.md"
  sk_refused 'empty body' "no instructions after the frontmatter" "$W/empty/empty" empty

  case_ "skills: refused skill dirs"
  mkdir -p "$W/ln/ln"
  printf -- '---\nname: ln\ndescription: Has a link.\n---\nBody.\n' >"$W/ln/ln/SKILL.md"
  ln -s /etc/passwd "$W/ln/ln/passwd"
  sk_refused link "a link in the skill" "$W/ln/ln" ln
  mkdir -p "$W/many/many/data"
  printf -- '---\nname: many\ndescription: Many files.\n---\nBody.\n' >"$W/many/many/SKILL.md"
  D="$W/many/many/data" awk 'BEGIN { for (i = 1; i <= 200; i++) { f = ENVIRON["D"] "/f" i; printf "" > f; close(f) } }'
  sk_refused '201 files' "holds 201 files, over 200" "$W/many/many" many
  rm "$W/many/many/data/f200"
  build "$W/many/many" many "" "" || fail "200 files refused: $ERR"
  mkdir -p "$W/nomd/nomd"
  printf 'x\n' >"$W/nomd/nomd/README.md"
  sk_refused 'no SKILL.md' "no SKILL.md in the skill dir" "$W/nomd/nomd" nomd
}

t_skill_utf8() {
  case_ "skills: UTF-8 only, checked on bytes"
  local t want
  # TEXT (printf escapes) : the offset skill_utf8 reports, or ok.
  for t in "plain ascii:ok" ":ok" "\303\251 \342\200\256 \360\237\230\200:ok" "\364\217\277\277:ok" \
    "\355\237\277:ok" "\377:1" "a\200:2" "\300\257:1" "\301\277:1" "\340\200\200:1" "\360\200\200\200:1" \
    "\355\240\200:1" "\364\220\200\200:1" "\370\210\200\200\200:1" "ab\342(:3" "a\342\200:2" \
    "\303\251\303:3" "Run \377 then \342\200\256txt.exe:5"; do
    want=${t##*:}
    # shellcheck disable=SC2059 # the fixture is printf's escapes.
    printf "${t%:*}" >"$W/u8.txt"
    eq "utf8 [${t%:*}]" "$(skill_utf8 "$W/u8.txt" || :)" "$([ "$want" = ok ] || echo "$want")"
    if [ "$want" = ok ]; then skill_utf8 "$W/u8.txt" >/dev/null || fail "utf8 [${t%:*}] exit"; fi
  done
  case_ "skills: a stray byte does not hide a character henki escapes"
  mkdir -p "$W/u8/rlo"
  printf -- '---\nname: rlo\ndescription: Hides.\n---\nRun \377 then \342\200\256txt.exe\n' >"$W/u8/rlo/SKILL.md"
  sk_refused 'body' "SKILL.md: not UTF-8 (byte 43); fix: save it as UTF-8" "$W/u8/rlo" rlo
  printf -- '---\nname: rlo\ndescription: x \377 y \342\200\256 z\n---\nBody.\n' >"$W/u8/rlo/SKILL.md"
  sk_refused 'description' "SKILL.md: not UTF-8 (byte 30)" "$W/u8/rlo" rlo
  printf -- '---\nname: rlo\ndescription: Fine.\nmetadata:\n  k: "\377"\n---\nBody.\n' >"$W/u8/rlo/SKILL.md"
  sk_refused 'metadata' "SKILL.md: not UTF-8" "$W/u8/rlo" rlo
  printf -- '\357\273\277---\nname: rlo\ndescription: Caf\303\251 \360\237\230\200.\n---\nBody \344\275\240.\n' >"$W/u8/rlo/SKILL.md"
  build "$W/u8/rlo" rlo "" "" || fail "valid UTF-8 refused: $ERR"
}

t_skill_text() {
  case_ "skills: description characters and the 160-byte cut"
  mkdir -p "$W/d/d"
  local e1024 e l want
  e1024=$(awk 'BEGIN { for (i = 0; i < 1024; i++) printf "\303\251" }')
  printf -- '---\nname: d\ndescription: %s\n---\nBody.\n' "$e1024" >"$W/d/d/SKILL.md"
  if build "$W/d/d" d "" ""; then
    eq 'cut bytes' "$(printf '%s' "$SK_DESC" | wc -c | tr -d ' ')" 160
    eq 'summary' "$(sed -n 's/^summary = "\(.*\)"$/\1/p' "$MOD/.henki/module.toml")" "$SK_DESC"
  else
    fail "1,024 characters refused: $ERR"
  fi
  printf -- '---\nname: d\ndescription: %s\303\251\n---\nBody.\n' "$e1024" >"$W/d/d/SKILL.md"
  sk_refused '1,025 characters' "description is 1025 characters, over 1024" "$W/d/d" d
  e=$(awk 'BEGIN { for (i = 0; i < 159; i++) printf "a"; printf "\303\251 more words" }')
  printf -- '---\nname: d\ndescription: %s\n---\nBody.\n' "$e" >"$W/d/d/SKILL.md"
  build "$W/d/d" d "" "" && eq 'boundary' "$SK_DESC" "$(awk 'BEGIN { for (i = 0; i < 159; i++) printf "a" }')"
  e=$(awk 'BEGIN { for (i = 0; i < 40; i++) printf "word%d ", i % 10 }')
  printf -- '---\nname: d\ndescription: %s\n---\nBody.\n' "$e" >"$W/d/d/SKILL.md"
  build "$W/d/d" d "" "" && eq 'word cut' "$SK_DESC" "$(awk 'BEGIN { for (i = 0; i < 26; i++) printf "%sword%d", (i ? " " : ""), i % 10 }')"
  printf -- '---\nname: d\ndescription: Says "hi" with a \\ in it.\n---\nBody.\n' >"$W/d/d/SKILL.md"
  build "$W/d/d" d "" "" && eq 'toml summary' "$(sed -n 's/^summary = //p' "$MOD/.henki/module.toml")" "\"Says 'hi' with a / in it.\""
  eq 'doc keeps them' "$(sed -n 1p "$MOD/doc.md")" 'Says "hi" with a \ in it.'
  printf -- '---\nname: d\ndescription: "   "\n---\nBody.\n' >"$W/d/d/SKILL.md"
  sk_refused 'blank description' "no description" "$W/d/d" d
  printf -- '---\nname: Dee\ndescription: x\n---\nBody.\n' >"$W/d/d/SKILL.md"
  sk_refused 'name rule' 'name "Dee" is not' "$W/d/d" ""
  printf -- '---\nname: d--e\ndescription: x\n---\nBody.\n' >"$W/d/d/SKILL.md"
  sk_refused 'double hyphen' 'name "d--e" is not' "$W/d/d" ""

  case_ "skills: licenses"
  mkdir -p "$W/d/d"
  for l in "Apache-2.0:Apache-2.0" "(MIT OR Apache-2.0) AND BSD-3-Clause:(MIT OR Apache-2.0) AND BSD-3-Clause" \
    "MIT OR:NOASSERTION" "GPL-2.0-only:NOASSERTION" "Complete terms in LICENSE.txt:NOASSERTION" "(MIT:NOASSERTION" \
    "MIT AND AND ISC:NOASSERTION"; do
    want=${l##*:}
    printf -- '---\nname: d\ndescription: x\nlicense: %s\n---\nBody.\n' "${l%:*}" >"$W/d/d/SKILL.md"
    build "$W/d/d" d "" "" && eq "license ${l%:*}" "$SK_LICENSE" "$want"
  done
}

t_skill_find() {
  case_ "skills: a repository of skills"
  if skill_find "$SK/repo" ""; then fail "the repo root counted as a skill"; fi
  eq message "$ERR" "not a skill (no SKILL.md), and 3 skill(s) are below it (above); fix: name one with #DIR"
  eq listing "$SK_FOUND" "  #skills/alpha
  #skills/beta
  #tools/gamma"
  skill_find "$SK/repo/skills/beta" skills/beta || fail "a skill dir not found: $ERR"
  skill_find "$SK/repo/skills" skills && fail "skills/ counted as a skill"
  has 'subdir' "$ERR" "#skills: not a skill"
  has 'subdir names' "$SK_FOUND" "#skills/alpha"
  skill_find "$SK/repo/a" a && fail "a/ counted as a skill"
  has 'deep' "$SK_FOUND" "#a/b/c/too-deep"
  mkdir -p "$W/none"
  skill_find "$W/none" "" && fail "an empty dir counted"
  has 'none' "$ERR" "no SKILL.md here or within 3 levels"
  mkdir -p "$W/lots"
  local i=1
  while [ "$i" -le 23 ]; do
    mkdir -p "$W/lots/s$i" && printf -- '---\nname: s%d\n---\n' "$i" >"$W/lots/s$i/SKILL.md"
    i=$((i + 1))
  done
  skill_find "$W/lots" ""
  has 'over 20' "$ERR" "23 skill(s) are below it"
  has 'more' "$SK_FOUND" "(3 more not shown)"
  eq 'shown' "$(printf '%s\n' "$SK_FOUND" | command grep -c '^  #')" 20
}

# --- add --skill -------------------------------------------------------------------------------------------------------

t_skill_add() {
  case_ "add --skill from a directory"
  run add --skill "$SK/plain-skill" --yes
  rc_is 0
  out_has "plain-skill skill-"
  out_has "  source   $SK/plain-skill (a directory on this machine)"
  out_has "  skill    plain-skill from $SK/plain-skill @ hd1:"
  out_has "  allowed-tools: Bash(git:*) Read (ignored: henki asks for each call as usual)"
  out_has "  data     3 files of the skill in .henki/skill, 1 of them scripts (not on PATH"
  out_has "  prompt   +0 B on every request"
  out_has "cmd/plain-skill -> ../.henki/template (sh, 12 lines): when you type /plain-skill"
  out_lacks "UNREVIEWED"
  local hd src
  hd=$(field plain-skill 8)
  src=$(modhd1 "$SK/plain-skill")
  check "lock: skill, the dir, no commit, skill= the source's hd1" [ "$(lockline plain-skill | cut -f 3,5,6,7,10,11)" = "on${tab}skill${tab}$SK/plain-skill${tab}-${tab}yes${tab}skill=$src" ]
  check "version from the source's hd1" [ "$(field plain-skill 4)" = "skill-$(printf '%s' "${src#hd1:}" | cut -c 1-12)" ]
  check "the store copy holds the skill's files as data" [ "$(modlist "$CONF/ext/plain-skill/" | cut -f 1 | tr '\n' ' ')" = ".henki/module.toml .henki/skill/LICENSE.txt .henki/skill/references/STYLE.md .henki/skill/scripts/changes.sh .henki/template LICENSE.txt cmd/plain-skill cmd/plain-skill.md doc.md " ]
  check "the scripts are not runnable" [ ! -x "$CONF/ext/plain-skill/.henki/skill/scripts/changes.sh" ]
  run verify
  rc_is 0
  run outdated
  out_has "up to date"
  run list
  out_has "plain-skill"
  out_lacks "UNREVIEWED"
  # Another name, and the same skill twice.
  run add --skill "$SK/plain-skill" --yes
  refused "plain-skill is taken by a module or a command of one (henki-mods list); fix: give the skill another name with --as NAME, or henki-mods update plain-skill"
  run add --skill "$SK/plain-skill" --as changes --yes
  rc_is 0
  check "the copy under --as" [ -f "$CONF/ext/changes/cmd/changes.md" ] && [ -L "$CONF/ext/changes/cmd/changes" ]
  run rm changes
  rc_is 0
  check "rm takes it out" [ ! -e "$CONF/ext/changes" ] && [ -z "$(lockline changes)" ]
  # Another module's command, and the names henki keeps.
  mkdir -p "$W/cm/cmd" && printf '# has /alpha\n' >"$W/cm/doc.md" && printf '#!/bin/sh\n' >"$W/cm/cmd/alpha" && chmod 755 "$W/cm/cmd/alpha"
  run add "$W/cm" --yes
  rc_is 0
  run add --skill "$SK/repo#skills/alpha" --yes
  refused "alpha is taken by a module or a command of one"
  run add --skill "$SK/repo#skills/alpha" --as sessions --yes
  refused "sessions is a name henki keeps for itself"
  run add --skill "$SK/repo" --yes
  refused "3 skill(s) are below it (above); fix: name one with #DIR"
  err_has "  #skills/beta"
  run add --skill "$SK/no-close" --yes
  refused "frontmatter this importer does not read"
  check "nothing of a refused import stays" [ ! -e "$CONF/ext/no-close" ] && [ -z "$(find "$STORE" -name '.add.*' -print)" ]
  # Flags that go with --skill only, and the other way.
  run add "$SK/plain-skill" --as x --yes
  rc_is 2
  run add --skill "$SK/plain-skill" --version 1 --yes
  rc_is 2

  case_ "add --skill asks, and can show the text first"
  printf 'y\ny\n' >"$W/answer"
  PAGER=cat HENKI_MODS_TTY="$W/answer" run add --skill "$SK/block-desc"
  rc_is 0
  out_has "Gather the merged pull requests since the last release"
  check "HOW tty" [ "$(field block-desc 10)" = tty ]
  run rm block-desc
  printf 'n\nn\n' >"$W/answer"
  PAGER=cat HENKI_MODS_TTY="$W/answer" run add --skill "$SK/block-desc"
  refused "not installed (the answer was no)"
  out_lacks "Gather the merged"
  check "a no installs nothing" [ -z "$(lockline block-desc)" ]
  # Inside a tool call (a session id, no hand-off file) it refuses, --yes or not.
  HENKI_SESSION=abc run add --skill "$SK/block-desc" --yes
  refused "runs inside a tool call"

  case_ "add --skill: a skill whose directory changed"
  cp -R "$SK/plain-skill" "$W/plain-skill"
  run add --skill "$W/plain-skill" --yes
  rc_is 0
  local old
  old=$(field plain-skill 8)
  printf '4. Say it in one line.\n' >>"$W/plain-skill/SKILL.md"
  run outdated
  out_has "changed: the skill in $W/plain-skill changed (henki-mods update plain-skill)"
  run update plain-skill --yes
  rc_is 0
  check "a new copy" [ "$(field plain-skill 8)" != "$old" ]
  check "skill= follows the source" [ "$(field plain-skill 11)" = "skill=$(modhd1 "$W/plain-skill")" ]
  check "the new text" command grep -q 'Say it in one line' "$CONF/ext/plain-skill/cmd/plain-skill.md"
  run outdated
  out_has "up to date"
  run rollback plain-skill --yes
  rc_is 0
  check "rollback goes back" [ "$(field plain-skill 8)" = "$old" ]
  run update plain-skill --to @main --yes
  refused "a skill from a directory has one version"
}

t_skill_url() {
  case_ "add --skill from a git URL, update, share"
  cp -R "$SK/repo" "$W/skills"
  mkrepo "$W/skills"
  local url="file://$W/skills" c1 c2 hd line
  c1=$(git -C "$W/skills" rev-parse HEAD)
  run add --skill "$url" --yes
  refused "3 skill(s) are below it"
  run add --skill "$url#skills/beta" --yes
  rc_is 0
  out_has "UNREVIEWED"
  out_has "  skill    beta from $url#skills/beta @ $c1 (Agent Skill)"
  check "lock: skill, URL#DIR, the commit, ref and skill flags" [ "$(lockline beta | cut -f 4-7,11)" = "skill-$(printf '%s' "$c1" | cut -c 1-12)${tab}skill${tab}$url#skills/beta${tab}$c1${tab}ref=HEAD,skill=$(modhd1 "$W/skills/skills/beta")" ]
  has doc "$(cat "$CONF/ext/beta/doc.md")" "Imported from the Agent Skill beta ($url#skills/beta @ $c1)."
  run list
  out_has "$url#skills/beta (UNREVIEWED)"
  run add --skill "$url#../x" --yes
  refused "is not a plain directory path"
  run outdated
  out_has "up to date"
  printf 'Do beta twice.\n' >>"$W/skills/skills/beta/SKILL.md"
  commit "$W/skills" second
  c2=$(git -C "$W/skills" rev-parse HEAD)
  run outdated
  out_has "HEAD is at $(printf '%s' "$c2" | cut -c 1-12) (henki-mods update beta)"
  run update beta --yes
  rc_is 0
  check "the new commit" [ "$(field beta 7):$(field beta 4)" = "$c2:skill-$(printf '%s' "$c2" | cut -c 1-12)" ]
  check "flags kept and the skill's hd1 new" [ "$(field beta 11)" = "ref=HEAD,skill=$(modhd1 "$W/skills/skills/beta")" ]
  hd=$(field beta 8)
  # share: the line installs these files elsewhere, or refuses.
  RC=0
  OUT=$($RUN "$MODS" share beta 2>"$SCRATCH/err") || RC=$?
  rc_is 0
  eq 'share line' "$OUT" "henki mods add --skill $url@$c2#skills/beta --as beta --expect $hd"
  line=${OUT#henki mods }
  export HOME="$W/home2"
  mkdir -p "$HOME"
  CONF="$HOME/.config/henki"
  # shellcheck disable=SC2086 # the share line's words
  run $line --yes
  rc_is 0
  check "the same files on the second home" [ "$(field beta 8)" = "$hd" ]
  run rm beta
  # shellcheck disable=SC2086
  run $(printf '%s' "$line" | sed 's/hd1:[0-9a-f]*/hd1:0000000000000000000000000000000000000000000000000000000000000000/') --yes
  refused "holds other files than the line expects"
  check "a wrong --expect installs nothing" [ -z "$(lockline beta)" ]
  run add --skill "$url#skills/beta" --expect hd1:abc --yes
  refused "is not hd1: and 64 hex digits"
}

# --- add --expect, share -----------------------------------------------------------------------------------------------

mkmod() { # DIR [EXTRA]
  local dir="$1" n="${1##*/}"
  mkdir -p "$dir/bin" "$dir/.henki"
  printf '# %s: says hello for tests\n\nThe body.\n' "$n" >"$dir/doc.md"
  printf '#!/bin/sh\necho %s\n' "$n" >"$dir/bin/$n"
  chmod 755 "$dir/bin/$n"
  printf 'henki_module = 1\nname = "%s"\nversion = "0.1.0"\nlicense = "0BSD"\ncaps_net = "none"\n%b' "$n" "${2:-}" >"$dir/.henki/module.toml"
}

t_share() {
  case_ "share: one line per kind; add --expect"
  mkdir -p "$W/repo"
  mkmod "$W/repo/hi"
  mkrepo "$W/repo"
  local c hd
  c=$(git -C "$W/repo" rev-parse HEAD)
  run add "file://$W/repo#hi" --yes
  rc_is 0
  hd=$(field hi 8)
  RC=0
  OUT=$($RUN "$MODS" share hi 2>"$SCRATCH/err") || RC=$?
  eq 'url' "$OUT" "henki mods add file://$W/repo@$c#hi --expect $hd"
  run add sessions --yes
  rc_is 0
  OUT=$($RUN "$MODS" share sessions 2>"$SCRATCH/err")
  eq 'official' "$OUT" "henki mods add sessions"
  mkmod "$W/local"
  run add "$W/local" --yes
  RC=0
  OUT=$($RUN "$MODS" share local 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
  refused "comes from a directory on this machine"
  # An index line (written as an index install writes it).
  printf 'lock/1\tixm\ton\t0.2.0\tindex:deorlovnis\tixm\t%s\t%s\t2026-11-08T10:00Z\tyes\t\n' "$c" "$hd" >>"$CONF/mods.lock"
  OUT=$($RUN "$MODS" share ixm 2>"$SCRATCH/err")
  eq 'index' "$OUT" "henki mods add ixm"
  run share nope
  refused "nope is not added"
  # The line on another home: these files, or a refusal.
  export HOME="$W/home2"
  mkdir -p "$HOME"
  CONF="$HOME/.config/henki"
  run add "file://$W/repo@$c#hi" --expect "$hd" --yes
  rc_is 0
  check "pinned to the commit (no ref to follow)" [ "$(field hi 7):$(field hi 11)" = "$c:" ]
  run rm hi
  printf 'echo changed\n' >>"$W/repo/hi/bin/hi"
  commit "$W/repo" changed
  run add "file://$W/repo#hi" --expect "$hd" --yes
  refused "holds other files than the line expects ($(printf '%s' "$(field hi 8)")"
  check "nothing installed" [ -z "$(lockline hi)" ]
  run add "$W/repo/hi" --expect "$hd" --yes
  refused "holds other files than the line expects"
}

# --- .henki data -------------------------------------------------------------------------------------------------------

t_henki_data() {
  case_ ".henki holds data files"
  mkmod "$W/lib"
  printf 'print("a library its port or command runs")\n' >"$W/lib/.henki/lib.py"
  mkdir -p "$W/lib/.henki/data/deep"
  printf 'x\n' >"$W/lib/.henki/data/deep/table.txt"
  run add "$W/lib" --yes
  rc_is 0
  run verify
  rc_is 0
  chmod 755 "$W/lib/.henki/lib.py"
  run verify --dir "$W/lib"
  refused "data files of mode 644"
}

# --- new ---------------------------------------------------------------------------------------------------------------

t_new() {
  local k n
  case_ "new: every kind passes test and add"
  for k in tool cmd template "observer turn_end" "port history"; do
    n=$(printf '%s' "$k" | cut -d ' ' -f 1)x
    [ "$k" != "port history" ] || [ -n "$REAL_HENKI" ] || continue
    RC=0
    OUT=$(cd "$W" && $RUN "$MODS" new "$n" --$k 2>"$SCRATCH/err") || RC=$?
    ERR=$(cat "$SCRATCH/err")
    rc_is 0
    out_has "made $W/$n/$n ($(printf '%s' "$k" | cut -d ' ' -f 1))"
    out_has "henki mods add ./$n"
    check "$k: the files around it" [ -f "$W/$n/test.sh" ] && [ -x "$W/$n/test.sh" ] && [ -f "$W/$n/README.md" ] && [ -f "$W/$n/LICENSE" ]
    RC=0
    OUT=$(cd "$W/$n" && HENKI_BIN=$REAL_HENKI $RUN "$MODS" test 2>"$SCRATCH/err") || RC=$?
    ERR=$(cat "$SCRATCH/err")
    rc_is 0
    out_has "ok   $n: layout, texts and links as an install checks them (0.1.0"
    out_has "ok   test.sh: exit 0 (a scratch home)"
    out_has "test: ok"
    out_lacks "FAIL"
    HENKI_BIN=$REAL_HENKI run add "$W/$n/$n" --yes
    rc_is 0
    # Committed and pushed as made, it is an entry the index takes.
    mkrepo "$W/$n"
    git init -q --bare "$W/$n.git"
    git -C "$W/$n" remote add origin "file://$W/$n.git"
    git -C "$W/$n" push -q origin HEAD:main 2>/dev/null
    git -C "$W/$n" fetch -q origin 2>/dev/null
    HENKI_BIN=$REAL_HENKI HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/$n" publish "./$n"
    rc_is 0
    entry_lints "$n"
  done
  check "the tool's eval task" [ -x "$W/toolx/toolx/eval/greet/check" ]
  run eval --check "$W/toolx/toolx/eval"
  rc_is 0
  check "the template is the prompts mod's" cmp -s "$W/templatex/templatex/.henki/template" "$HERE/../mods/prompts/.henki/template"
  check "the observer's event" [ -x "$W/observerx/observerx/on/turn_end" ]
  if [ -n "$REAL_HENKI" ]; then
    HENKI_BIN=$REAL_HENKI run swap history portx --yes
    rc_is 0
    out_has "passed (history/1"
  fi

  case_ "new: two modules in one directory, and the refusals"
  RC=0
  OUT=$($RUN "$MODS" new one --dir "$W/r" 2>"$SCRATCH/err") || RC=$?
  rc_is 0
  OUT=$($RUN "$MODS" new two --cmd --dir "$W/r" 2>"$SCRATCH/err") || RC=$?
  rc_is 0
  out_has "test.sh was there already: add two's tests to it"
  check "test.sh is the first one's" command grep -q 'one/bin/one' "$W/r/test.sh"
  RC=0
  OUT=$(cd "$W/r" && $RUN "$MODS" test 2>&1) || RC=$?
  rc_is 0
  out_has "ok   one:"
  out_has "ok   two:"
  for k in "one --dir $W/r:exists" "sessions:a name henki keeps" "history:a name henki keeps" "Bad:a name is 1 to 32" \
    "x --observer turn_middle:not an event" "x --port policy:writes a history binding only"; do
    RC=0
    # shellcheck disable=SC2086 # the case's words
    OUT=$(cd "$W" && $RUN "$MODS" new ${k%%:*} 2>"$SCRATCH/err") || RC=$?
    ERR=$(cat "$SCRATCH/err")
    refused "${k#*:}"
  done
}

# --- test --------------------------------------------------------------------------------------------------------------

dev_run() { # DIR ARGS...: henki-mods ARGS in DIR, into OUT (stdout and stderr), RC
  local d="$1"
  shift
  RC=0
  OUT=$(cd "$d" && $RUN "$MODS" "$@" 2>&1) || RC=$?
  ERR=""
}

t_test() {
  case_ "test: what it finds"
  mkmod "$W/r/good"
  mkdir -p "$W/r/bad/cmd" && printf '# bad\n' >"$W/r/bad/doc.md" && printf '#!/bin/sh\n' >"$W/r/bad/cmd/help" && chmod 755 "$W/r/bad/cmd/help"
  dev_run "$W/r" test
  rc_is 1
  has 'the bad one' "$OUT" "FAIL bad: ./cmd/help: a name henki keeps for its own command"
  has 'the good one still' "$OUT" "ok   good:"
  has 'the count' "$OUT" "1 checks failed"
  rm -rf "$W/r/bad"
  # A command an official module has too, a module without a manifest, a module named apart from its dir.
  mkdir -p "$W/r/rev/cmd" && printf '# rev\n' >"$W/r/rev/doc.md" && printf '#!/bin/sh\n' >"$W/r/rev/cmd/review" && chmod 755 "$W/r/rev/cmd/review"
  dev_run "$W/r" test
  rc_is 0
  has 'clash' "$OUT" "warn rev: cmd/review is also the official module prompts's; henki does not start with both on"
  has 'no manifest' "$OUT" "warn rev: no .henki/module.toml"
  # test.sh runs in the dir with a scratch home, and its failure fails the test.
  printf '#!/bin/sh\necho "home=$HOME"\necho "state=$XDG_STATE_HOME"\nexit 3\n' >"$W/r/test.sh"
  dev_run "$W/r" test
  rc_is 1
  has 'test.sh exit' "$OUT" "FAIL test.sh: exit 3 (above)"
  has 'test.sh output' "$OUT" "     | home=${TMPDIR}/henki-test."
  case "$OUT" in *"home=$HOME"*) fail "test.sh saw the real HOME" ;; esac
  check "the scratch home goes" [ -z "$(find "$TMPDIR" -maxdepth 1 -name 'henki-test.*' -print)" ]
  # One module by its dir: the module only, never test.sh.
  dev_run "$W/r" test ./good
  rc_is 0
  case "$OUT" in *test.sh*) fail "test of one module ran test.sh" ;; esac
  dev_run "$W" test ./nothing
  refused_out "is not a directory"
  mkdir -p "$W/empty"
  dev_run "$W/empty" test
  refused_out "holds no module"
  if [ -n "$REAL_HENKI" ]; then
    # A binding that fails its suite.
    mkdir -p "$W/p/bp/port" && printf '# a broken binding\n' >"$W/p/bp/doc.md"
    printf '#!/bin/sh\nwhile read -r l; do echo nope; done\n' >"$W/p/bp/port/history" && chmod 755 "$W/p/bp/port/history"
    RC=0
    OUT=$(cd "$W/p" && HENKI_BIN=$REAL_HENKI $RUN "$MODS" test 2>&1) || RC=$?
    rc_is 1
    has 'suite' "$OUT" "FAIL bp: port/history fails the history/1 suite (above)"
  fi
}
refused_out() { # TEXT: a refusal on OUT (stdout and stderr together)
  rc_is 1
  has refusal "$OUT" "$1"
}

# --- try ---------------------------------------------------------------------------------------------------------------

t_try() {
  case_ "try: a scratch home, only the module, nothing of it left"
  # A stand-in henki that records what it saw.
  mkdir -p "$W/bin"
  printf '#!/bin/sh\n{ echo "args=$*"; echo "home=$HOME"; echo "state=$XDG_STATE_HOME"; echo "data=$XDG_DATA_HOME"; echo "cache=$XDG_CACHE_HOME"; echo "config=$XDG_CONFIG_HOME"; echo "session=${HENKI_SESSION:-}"; ls "$HOME/.config/henki/ext"; cat "$HOME/.config/henki/config.toml"; } >%s/seen\nexit 4\n' "$W" >"$W/bin/henki"
  chmod 755 "$W/bin/henki"
  mkdir -p "$CONF" "$W/sentinel"
  printf 'model = "m"\next = ["/elsewhere"]\nbase_url = "http://127.0.0.1:9/v1"\n' >"$CONF/config.toml"
  mkmod "$W/tm"
  export XDG_STATE_HOME="$W/sentinel" XDG_DATA_HOME="$W/sentinel" XDG_CACHE_HOME="$W/sentinel"
  RC=0
  OUT=$(cd "$W" && HENKI_BIN="$W/bin/henki" $RUN "$MODS" try ./tm -- run --json hello 2>&1) || RC=$?
  rc_is 4
  has 'added' "$OUT" "added tm: on"
  has 'args' "$(cat "$W/seen")" "args=run --json hello"
  case "$(cat "$W/seen")" in *"home=$HOME"* | *"=$W/sentinel"*) fail "henki saw the real home or XDG dirs: $(cat "$W/seen")" ;; esac
  has 'scratch state' "$(cat "$W/seen")" "state=$TMPDIR/henki-try."
  has 'only tm' "$(sed -n '/^session=/,$p' "$W/seen" | sed -n 2p)" "tm"
  has 'config copied' "$(cat "$W/seen")" 'model = "m"'
  case "$(cat "$W/seen")" in *'ext = '*) fail "the ext key was copied" ;; esac
  check "the sentinel dir is untouched" [ -z "$(ls -A "$W/sentinel")" ]
  check "no store or lock in this home" [ ! -e "$CONF/mods.lock" ] && [ ! -e "$CONF/ext" ]
  check "the scratch dir goes" [ -z "$(find "$TMPDIR" -maxdepth 1 -name 'henki-try.*' -print)" ]
  RC=0
  OUT=$(cd "$W" && HENKI_BIN="$W/bin/henki" $RUN "$MODS" try ./tm --keep 2>&1) || RC=$?
  rc_is 4
  has 'kept' "$OUT" "try: kept $TMPDIR/henki-try."
  # A module that shadows a command on PATH starts with the shadow line in the scratch config.
  mkmod "$W/sh"
  mv "$W/sh/bin/sh" "$W/sh/bin/ls"
  RC=0
  OUT=$(cd "$W" && HENKI_BIN="$W/bin/henki" $RUN "$MODS" try ./sh 2>&1) || RC=$?
  has 'shadow' "$(cat "$W/seen")" 'shadow = ["sh/ls"]'
  # Inside henki, and a module that cannot be added.
  RC=0
  OUT=$(cd "$W" && HENKI_DEPTH=1 HENKI_BIN="$W/bin/henki" $RUN "$MODS" try ./tm 2>&1) || RC=$?
  refused_out "try starts henki, and this runs inside henki"
  printf '\033[31m\n' >>"$W/tm/doc.md"
  RC=0
  OUT=$(cd "$W" && HENKI_BIN="$W/bin/henki" $RUN "$MODS" try ./tm 2>&1) || RC=$?
  refused_out "could not be added in the scratch home"
  check "the scratch dir goes on a refusal too" [ "$(find "$TMPDIR" -maxdepth 1 -name 'henki-try.*' -print | wc -l | tr -d ' ')" = 1 ]
}

# --- publish -----------------------------------------------------------------------------------------------------------

# The two files publish printed (OUT) for NAME 0.1.0 pass the index's lint (entry lint), as a pull request would.
entry_lints() { # NAME
  local o="$OUT" f
  mkdir -p "$W/ix/mods/$1"
  printf '%s\n' "$o" | HM_H="# mods/$1.toml" awk 'index($0, ENVIRON["HM_H"]) == 1 { f = 1; next } /^$/ { f = 0 } f' >"$W/ix/mods/$1.toml"
  printf '%s\n' "$o" | HM_H="# mods/$1/0.1.0.toml" awk '$0 == ENVIRON["HM_H"] { f = 1; next } /^$/ { f = 0 } f' >"$W/ix/mods/$1/0.1.0.toml"
  for f in "$W/ix/mods/$1.toml" "$W/ix/mods/$1/0.1.0.toml"; do
    check "$f holds an entry" [ -s "$f" ]
    HENKI_INDEX_ALLOW_FILE=1 dev_run "$W" entry lint "$f"
    rc_is 0
  done
  OUT=$o
}

t_publish() {
  case_ "publish: the entry files for a pushed module"
  mkdir -p "$W/src"
  mkmod "$W/src/pm" 'summary = "says hello, for the index"\nmaintainer = "fixture-dev"\n'
  mkrepo "$W/src"
  git init -q --bare "$W/remote.git"
  git -C "$W/src" remote add origin "file://$W/remote.git"
  dev_run "$W/src" publish ./pm
  refused_out "no remote branch or tag holds"
  git -C "$W/src" push -q origin HEAD:main 2>/dev/null
  git -C "$W/src" fetch -q origin 2>/dev/null
  dev_run "$W/src" publish ./pm
  refused_out "is not https"
  printf 'x\n' >"$W/src/pm/README.md"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm
  refused_out "has changes that are not committed"
  rm "$W/src/pm/README.md"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm
  rc_is 0
  local c
  c=$(git -C "$W/src" rev-parse HEAD)
  has 'package' "$OUT" "# mods/pm.toml"
  has 'repo' "$OUT" "repo = \"file://$W/remote.git\""
  has 'path' "$OUT" 'path = "pm"'
  has 'maintainer' "$OUT" 'maintainer = "fixture-dev"'
  has 'version file' "$OUT" "# mods/pm/0.1.0.toml"
  has 'commit' "$OUT" "commit = \"$c\""
  has 'bins' "$OUT" 'bins = ["pm"]'
  has 'nothing sent' "$OUT" "Nothing was sent anywhere."
  entry_lints pm
  # A GitHub ssh remote is named by its https URL, and its owner is the maintainer when the manifest has none.
  sed '/^maintainer/d' "$W/src/pm/.henki/module.toml" >"$W/t" && mv "$W/t" "$W/src/pm/.henki/module.toml"
  commit "$W/src" "no maintainer"
  git -C "$W/src" push -q origin HEAD:main 2>/dev/null
  git -C "$W/src" fetch -q origin 2>/dev/null
  git -C "$W/src" remote set-url origin git@github.com:someone/hello-mods.git
  # The derive reads the local repository, so the remote URL is only what the entry names.
  dev_run "$W/src" publish ./pm
  rc_is 0
  has 'https' "$OUT" 'repo = "https://github.com/someone/hello-mods.git"'
  has 'owner' "$OUT" 'maintainer = "someone"'
  dev_run "$W" publish ./src
  refused_out "is not a module"
}

# --- eval: the statistics, report, badge and publish --eval -------------------------------------------------------------

EV="$HERE/testdata/eval"
# Pinned: the effect of 200 made-up tasks (the binomial and the bootstrap stay exact; the same under every awk).
PIN_BIG='"+60 per 600 (95% CI +35 to +85; tasks 80 better, 40 worse, 80 equal; sign p <0.001)"'

t_stats() {
  case_ "stats: badges pinned on the fixture rows"
  local f b got code st
  for f in "$EV"/*.tsv; do
    b=$(basename "$f" .tsv)
    code=0
    got=$(evalstats badge "$f") || code=$?
    eq "$b" "$got" "$(cat "$EV/expected/$b.badge")"
    case "$b" in pass | thirteen) eq "$b exit" "$code" 0 ;; *) eq "$b exit" "$code" 1 ;; esac
    eq "$b keys" "$(evalstats keys "$f")" "$(sed 1d "$EV/expected/$b.badge")"
  done
  for b in pass void; do
    eq "$b report" "$(evalstats report "$EV/$b.tsv")" "$(cat "$EV/expected/$b.report")"
  done
  eq stdin "$(evalstats badge - <"$EV/pass.tsv")" "$(cat "$EV/expected/pass.badge")"
  # more: the runner's cue for 10 more repeats, per suite.
  eq 'more: rerun' "$(evalstats more "$EV/rerun.tsv" | sort)" "$(printf 'general\tyes\nhalti\tyes')"
  eq 'more: pass' "$(evalstats more "$EV/pass.tsv" | sort)" "$(printf 'general\tyes\nown\tno')"

  case_ "stats: rows it cannot read"
  for f in "$EV"/bad/*.tsv; do
    b=$(basename "$f" .tsv)
    code=0
    got=$(evalstats badge "$f" 2>&1) || code=$?
    eq "$b exit" "$code" 2
    has "$b" "$got" "evalrun/1"
  done
  code=0
  got=$(evalstats badge /dev/null 2>&1) || code=$?
  eq 'no rows' "$code:$got" "2:evalrun/1: no rows"

  case_ "stats: STATUS is one of a closed set"
  # Each VOID cause voids the badge; each other status of henki's result counts as a run; anything else stops.
  for st in none setup error:config error:nested error:journal error:internal error:tampered; do
    ST="$st" awk -F '\t' 'BEGIN { OFS = FS } NR == 1 { $12 = ENVIRON["ST"] } { print }' "$EV/pass.tsv" >"$W/st.tsv"
    code=0
    got=$(evalstats badge "$W/st.tsv") || code=$?
    eq "$st exit" "$code" 1
    has "$st" "$got" "FAIL: a VOID run (1)"
    has "$st report" "$(evalstats report "$W/st.tsv")" "VOID: 1 run(s)"
  done
  for st in ok gave_up:max_steps gave_up:stuck gave_up:empty_answer needs_approval budget interrupted output_closed \
    error:auth error:changed error:context_exceeded error:gateway error:provider error:stream_incomplete; do
    ST="$st" awk -F '\t' 'BEGIN { OFS = FS } NR == 1 { $12 = ENVIRON["ST"] } { print }' "$EV/pass.tsv" >"$W/st.tsv"
    eq "$st" "$(evalstats badge "$W/st.tsv" | head -n 1)" PASS
  done
  for st in VOID max_steps gave_up error error:other error:config: OK "ok "; do
    ST="$st" awk -F '\t' 'BEGIN { OFS = FS } NR == 1 { $12 = ENVIRON["ST"] } { print }' "$EV/pass.tsv" >"$W/st.tsv"
    code=0
    got=$(evalstats badge "$W/st.tsv" 2>&1) || code=$?
    eq "[$st] exit" "$code" 2
    eq "[$st]" "$got" "evalrun/1 line 1: status $st is not an evalrun/1 status"
  done

  case_ "stats: bootstrap edge cases"
  # One task: every resample is that task, so the CI is the effect.
  printf 'evalrun/1\tR\tp\t1\thd1:x\tm\t%s\t%s\t1\tt1\t%s\tok\t1\t1\t0\n' own on 1 own off 0 general on 1 general off 1 >"$W/one.tsv"
  eq 'one task' "$(evalstats keys "$W/one.tsv" | sed -n 's/^eval_effect = //p')" '"+1 per 1 (95% CI +1 to +1; tasks 1 better, 0 worse, 0 equal; sign p 1.000)"'
  eq 'one task general' "$(evalstats keys "$W/one.tsv" | sed -n 's/^eval_general = //p')" '"on 1/1, off 1/1: 0 per 1 (95% CI 0 to 0)"'
  # Numeric-looking task names sort as bytes: "10" before "9".
  printf 'evalrun/1\tR\tp\t1\thd1:x\tm\town\t%s\t1\t%s\t%s\tok\t1\t1\t0\n' on 9 1 off 9 0 on 10 0 off 10 0 >"$W/num.tsv"
  eq 'byte order' "$(evalstats report "$W/num.tsv" | sed -n 's/^  \([0-9]*\) .*/\1/p' | tr '\n' ' ')" "10 9 "
  awk 'BEGIN { for (t = 1; t <= 200; t++) for (r = 1; r <= 3; r++) {
      on = (t * 7 + r) % 5 < 3; off = (t * 3 + r) % 4 < 2
      printf "evalrun/1\tR\tp\t1\thd1:x\tm\town\ton\t%d\tt%03d\t%d\tok\t1\t1\t0\n", r, t, on
      printf "evalrun/1\tR\tp\t1\thd1:x\tm\town\toff\t%d\tt%03d\t%d\tok\t1\t1\t0\n", r, t, off } }' >"$W/big.tsv"
  eq '200 tasks' "$(evalstats keys "$W/big.tsv" | sed -n 's/^eval_effect = //p')" "$PIN_BIG"
}

# A run dir RUN under the case's state dir: pass.tsv's rows with HD1 and RUN put in, and a run.toml (EXTRA: more lines).
mkrun() { # RUN HD1 [EXTRA]
  local d="$XDG_STATE_HOME/henki/eval/$1"
  mkdir -p "$d"
  HM_R="$1" HM_H="$2" awk -F '\t' 'BEGIN { OFS = FS } { $2 = ENVIRON["HM_R"]; $5 = ENVIRON["HM_H"]; print }' "$EV/pass.tsv" >"$d/runs.tsv"
  printf 'run = "%s"\nserver_build = "b9999-abc"\n%b' "$1" "${3:-}" >"$d/run.toml"
}
SHA64=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef

t_eval_dev() {
  case_ "eval report and badge: a run by its name, dir or rows file, through henki-mods"
  local h
  mkrun 20270101T000000Z-abcd hd1:5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e
  run eval report 20270101T000000Z-abcd
  rc_is 0
  eq report "$OUT" "$(sed 's/20270111T220000Z-a1b2/20270101T000000Z-abcd/' "$EV/expected/pass.report")"
  run eval badge "$XDG_STATE_HOME/henki/eval/20270101T000000Z-abcd"
  rc_is 0
  eq badge "$OUT" "$(cat "$EV/expected/pass.badge")"
  run eval badge "$EV/weak.tsv"
  rc_is 1
  out_has "FAIL: "
  run eval badge 20270101T000000Z-none
  refused "no eval run 20270101T000000Z-none"
  printf 'x\n' >"$W/junk.tsv"
  run eval report "$W/junk.tsv"
  refused "the rows do not read"

  case_ "eval badge --check: an index entry's badge against its rows"
  mkdir -p "$W/v"
  { printf 'index = 1\nversion = "0.3.0"\nhd1 = "hd1:5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e"\n'
    sed 1d "$EV/expected/pass.badge"; } >"$W/v/ok.toml"
  run eval badge --check "$EV/pass.tsv" "$W/v/ok.toml"
  rc_is 0
  out_has "ok: the rows recompute to the badge (PASS)"
  sed 's/^eval_on = .*/eval_on = "27\/36"/' "$W/v/ok.toml" >"$W/v/edited.toml"
  run eval badge --check "$EV/pass.tsv" "$W/v/edited.toml"
  rc_is 1
  out_has 'FAIL: eval_on is "27/36"; the rows give "26/36"'
  sed 's/^hd1 = .*/hd1 = "hd1:6e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e"/' "$W/v/ok.toml" >"$W/v/hd1.toml"
  run eval badge --check "$EV/pass.tsv" "$W/v/hd1.toml"
  rc_is 1
  out_has "FAIL: eval_hd1 is not the version's hd1"
  run eval badge --check "$EV/weak.tsv" "$W/v/ok.toml"
  rc_is 1
  out_has "FAIL: "
  # The index's own check.sh runs exactly this command.
  RC=0
  OUT=$($RUN "$DEV" eval badge --check "$EV/void.tsv" "$W/v/ok.toml" 2>&1) || RC=$?
  rc_is 1
  has 'void' "$OUT" "FAIL: a VOID run (1)"

  case_ "publish --eval: the badge block and the rows of a pushed module"
  mkdir -p "$W/src"
  mkmod "$W/src/pm" 'summary = "says hello, for the index"\nmaintainer = "fixture-dev"\n'
  mkrepo "$W/src"
  git init -q --bare "$W/remote.git"
  git -C "$W/src" remote add origin "file://$W/remote.git"
  git -C "$W/src" push -q origin HEAD:main 2>/dev/null
  git -C "$W/src" fetch -q origin 2>/dev/null
  h=$(modhd1 "$W/src/pm")
  mkrun R1 "$h"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm --eval R1
  refused_out "the run names no model file"
  mkrun R1 "$h" "model_sha256 = \"$SHA64\"\n"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm --eval R1 --by some-one
  rc_is 0
  has 'suite' "$OUT" 'eval_suite = "halti"'
  has 'sha' "$OUT" "eval_model_sha256 = \"$SHA64\""
  has 'server' "$OUT" 'eval_server = "llama.cpp b9999-abc"'
  has 'hd1' "$OUT" "eval_hd1 = \"$h\""
  has 'by' "$OUT" "eval_by = \"some-one $(date -u +%Y-%m-%d)\""
  has 'rows' "$OUT" "# evals/pm/0.1.0.tsv"
  has 'a row' "$OUT" "$(head -n 1 "$XDG_STATE_HOME/henki/eval/R1/runs.tsv")"
  entry_lints pm
  # The rows split out of the output recompute to the version file's badge.
  printf '%s\n' "$OUT" | awk 'index($0, "# evals/pm/0.1.0.tsv ") == 1 { f = 1; next } /^$/ { f = 0 } f' >"$W/rows.tsv"
  check "the printed rows are the run's" cmp -s "$W/rows.tsv" "$XDG_STATE_HOME/henki/eval/R1/runs.tsv"
  run eval badge --check "$W/rows.tsv" "$W/ix/mods/pm/0.1.0.toml"
  rc_is 0
  mkrun R2 hd1:1111111111111111111111111111111111111111111111111111111111111111 "model_sha256 = \"$SHA64\"\n"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm --eval R2
  refused_out "the run measured other files"
  cp "$EV/weak.tsv" "$XDG_STATE_HOME/henki/eval/R2/runs.tsv"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm --eval R2
  refused_out "no badge from this run: FAIL:"
  HENKI_INDEX_ALLOW_FILE=1 dev_run "$W/src" publish ./pm --by 'a b'
  refused_out "--by a b: not a GitHub login"

  case_ "publish --eval of an official module: a badge record under henki's version"
  mkdir -p "$W/share/official" "$W/bin"
  mkmod "$W/share/official/offi"
  printf '#!/bin/sh\necho "henki 0.3.0 (test)"\n' >"$W/bin/henki" && chmod 755 "$W/bin/henki"
  h=$(modhd1 "$W/share/official/offi")
  mkrun R3 "$h" "model_sha256 = \"$SHA64\"\n"
  HENKI_SHARE="$W/share" HENKI_BIN="$W/bin/henki" dev_run "$W" publish "$W/share/official/offi"
  refused_out "an index lists one for its badge only"
  HENKI_SHARE="$W/share" HENKI_BIN="$W/bin/henki" dev_run "$W" publish "$W/share/official/offi" --eval R3 --by owner
  rc_is 0
  has 'kind' "$OUT" 'kind = "official"'
  has 'version' "$OUT" "# mods/offi/0.3.0.toml"
  has 'hd1' "$OUT" "hd1 = \"$h\""
  out_lacks 'commit = '
  has 'rows' "$OUT" "# evals/offi/0.3.0.tsv"
  mkdir -p "$W/ix/mods/offi"
  printf '%s\n' "$OUT" | awk '$0 == "# mods/offi.toml (a package the index does not list yet; for one it does, set only latest = \"0.3.0\")" { f = 1; next } /^$/ { f = 0 } f' >"$W/ix/mods/offi.toml"
  printf '%s\n' "$OUT" | awk '$0 == "# mods/offi/0.3.0.toml" { f = 1; next } /^$/ { f = 0 } f' >"$W/ix/mods/offi/0.3.0.toml"
  dev_run "$W" entry lint "$W/ix/mods/offi.toml"
  rc_is 0
  dev_run "$W" entry lint "$W/ix/mods/offi/0.3.0.toml"
  rc_is 0
}

# --- routing -----------------------------------------------------------------------------------------------------------

t_routing() {
  case_ "henki-mods hands the author's verbs to henki-mods-dev"
  run help
  rc_is 0
  out_has "henki-mods add --skill SRC [--as NAME] [--yes]"
  out_has "henki-mods try [DIR|NAME] [--keep] [-- HENKI_ARGS]"
  RC=0
  OUT=$($RUN "$DEV" help 2>&1) || RC=$?
  rc_is 0
  has 'dev help' "$OUT" "henki-mods-dev new NAME"
  run test --port nope ./x
  refused "nope is not a port this henki binds"
  run new
  rc_is 2
  err_has "usage: henki-mods-dev new NAME"
}

# --- the hand-off through henki ----------------------------------------------------------------------------------------

t_skill_henki() {
  [ -n "$REAL_HENKI" ] || return 0
  case_ "a skill's command loads in henki, and costs no prompt bytes"
  run add --skill "$SK/plain-skill" --yes
  rc_is 0
  run add --skill "$SK/block-desc" --yes
  rc_is 0
  mkdir -p "$W/work"
  printf 'model = "m"\nbase_url = "http://127.0.0.1:9/v1"\n' >"$CONF/config.toml"
  RC=0
  OUT=$(cd "$W/work" && "$REAL_HENKI" run --json --max-seconds 1 hello 2>&1) || RC=$?
  has 'starts a session' "$OUT" '"session":"2'
  case "$OUT" in *'"kind":"config"'*) fail "henki refused the modules: $OUT" ;; esac
}

t_static() {
  case_ "static: the dev tests parse under bash -n"
  bash -n "$HERE/dev-tests.sh" || fail "bash -n dev-tests.sh"
  bash -n "$DEV" || fail "bash -n henki-mods-dev"
  local hits
  hits=$(command grep -nE '^[^#]*([[:alnum:]_]+=\(|\$\{[[:alnum:]_]+\[)' "$DEV" || true)
  [ -z "$hits" ] || fail "henki-mods-dev uses an array:$(printf '\n%s' "$hits")"
  hits=$(command grep -nE '^[^#]*(^|[^[:alnum:]_])awk([[:space:]]+-F[[:space:]]*[^[:space:]]+)*[[:space:]]+-v' "$DEV" || true)
  [ -z "$hits" ] || fail "henki-mods-dev passes data to awk with -v:$(printf '\n%s' "$hits")"
}

units() {
  t_stats
  t_frontmatter
  t_skill_build
  t_skill_names
  t_skill_refusals
  t_skill_utf8
  t_skill_text
  t_skill_find
}
verbs() {
  t_skill_add
  t_skill_url
  t_share
  t_henki_data
  t_new
  t_test
  t_try
  t_publish
  t_eval_dev
  t_routing
}

t_static
units
RUN=bash
verbs
t_skill_henki
RUN="bash --posix"
verbs
if [ "$FAILS" != 0 ]; then
  echo "dev-tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "dev-tests.sh: $CASES cases passed"
