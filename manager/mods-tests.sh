#!/usr/bin/env bash
# The tests of the official modules in mods/ (the ones a module's own dir cannot hold: ext/2 refuses a stray file).
# Hermetic: each case runs a module's command or tool as henki would, with a scratch HENKI_STATE, HENKI_SESSION,
# HENKI_MODULE and HENKI_NEXT, and stand-ins for the programs it calls on PATH. Bash 3.2 compatible like the manager.
#
#   manager/mods-tests.sh               every case
#   MODS_TEST_HENKI=PATH manager/mods-tests.sh
#       also runs a henki session bound to compact (check.sh passes the henki it built)
#
# Exit 0 when every case passes; each failure prints the case, the check and the command's output.
set -u
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 1
MODS_DIR="$(pwd -P)/mods"
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/henki-mods-tests.XXXXXX") || exit 1
trap 'rm -rf "$SCRATCH"' EXIT
nl='
'
FAILS=0
CASES=0

case_() {
  CASE="$1"
  CASES=$((CASES + 1))
  D="$SCRATCH/$CASES"
  mkdir -p "$D/state/sessions/scope1" "$D/state/sessions/scope2" "$D/bin"
  export HENKI_STATE="$D/state" HENKI_NEXT="$D/next" HENKI_SESSION=20261108T100000Z-cccc
  : >"$HENKI_NEXT"
}

# Runs PROGRAM ARGS with PATH as given in P (default: the system dirs); OUT, ERR, RC.
run() {
  RC=0
  OUT=$(PATH=${P:-/usr/bin:/bin} "$@" 2>"$SCRATCH/err") || RC=$?
  ERR=$(cat "$SCRATCH/err")
}
fail() {
  FAILS=$((FAILS + 1))
  printf 'FAIL %s: %s\n  rc=%s\n  out: %s\n  err: %s\n  next: %s\n' "$CASE" "$*" "${RC:-}" "${OUT:-}" "${ERR:-}" "$(cat "$HENKI_NEXT" 2>/dev/null)" >&2
}
rc_is() { [ "$RC" = "$1" ] || fail "exit $RC, wanted $1"; }
out_has() { case "$OUT" in *"$1"*) ;; *) fail "stdout lacks: $1" ;; esac; }
err_has() { case "$ERR" in *"$1"*) ;; *) fail "stderr lacks: $1" ;; esac; }
next_is() { [ "$(cat "$HENKI_NEXT")" = "$1" ] || fail "HENKI_NEXT is not: $1"; }

# The std scope dir of a directory: FNV-1a 64 of its realpath (journal/1).
fnv() {
  local h=-3750763034362895579 b
  for b in $(printf '%s' "$1" | od -An -v -tu1); do h=$(((h ^ b) * 1099511628211)); done
  printf '%016x\n' "$h"
}

# A std journal for session ID in scope dir S, its first user record holding TEXT (JSON-escaped already), its mtime T
# (touch -t form).
journal() { # S ID TEXT T
  local f="$HENKI_STATE/sessions/$1/$2.jsonl"
  mkdir -p "${f%/*}"
  printf '{"t":"session","v":1}\n' >"$f"
  [ -z "$3" ] || printf '{"msg":{"content":"%s","role":"user"},"t":"user","ts":"2026-11-08T10:00:00Z"}\n' "$3" >>"$f"
  touch -t "$4" "$f"
}

# The sessions mod reads sessions through `henki mods history`: HENKI_BIN is a stand-in that runs this checkout's
# manager, with a HOME of its own (no history link: std) and the work dir W as the session's folder.
t_sessions() {
  case_ "sessions: list, resume, new, reload, fork"
  local s="$MODS_DIR/sessions/cmd" w="$D/w" here bad sc home=$HOME
  mkdir -p "$w" "$D/home"
  printf '#!/bin/sh\n[ "$1" = mods ] && shift\nexec bash %s/../manager/henki-mods "$@"\n' "$MODS_DIR" >"$D/bin/henki"
  chmod 755 "$D/bin/henki"
  export HENKI_MODULE="$MODS_DIR/sessions" HENKI_BIN="$D/bin/henki" HOME="$D/home"
  sc=$(fnv "$(cd -P "$w" && pwd)")
  journal "$sc" 20261108T080000Z-aaaa 'cwd: /w\nos: linux x\ndate: 2026-11-08\n\nfix the \"parser\" please\nand more' 202611080800
  journal "$sc" 20261108T090000Z-bbbb 'cwd: /w\nos: linux x\ndate: 2026-11-08\n\n'"$(printf 'x%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69 70)" 202611080900
  journal "$sc" "$HENKI_SESSION" '' 202611081000
  printf '{"t":"notice","text":"n","ts":"2026-11-08T10:00:00Z"}\n{"msg":{"content":"second","role":"user"},"t":"user","ts":"2026-11-08T10:00:00Z"}\n' >>"$HENKI_STATE/sessions/$sc/20261108T080000Z-aaaa.jsonl"
  touch -t 202611080800 "$HENKI_STATE/sessions/$sc/20261108T080000Z-aaaa.jsonl"
  journal scope2 20261108T110000Z-dddd 'cwd: /v\nos: linux x\ndate: 2026-11-08\n\nelsewhere' 202611081100
  here=$(pwd)
  cd "$w" || return
  run "$s/sessions"
  rc_is 0
  out_has " 1 * 20261108T100000Z-cccc 2026-11-08 10:00"
  out_has " 2   20261108T090000Z-bbbb 2026-11-08 09:00"
  out_has "  $(printf 'x%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60)$nl"
  out_has " 3   20261108T080000Z-aaaa 2026-11-08 08:00"
  out_has 'fix the "parser" please'
  case "$OUT" in *dddd* | *"and more"*) fail "another scope's session, or a second line" ;; esac
  run "$s/resume"
  rc_is 0
  next_is "resume 20261108T090000Z-bbbb"
  run "$s/resume" 3
  next_is "resume 20261108T080000Z-aaaa"
  run "$s/resume" 20261108T090000Z-bbbb
  next_is "resume 20261108T090000Z-bbbb"
  : >"$HENKI_NEXT"
  for bad in 9 20261108T110000Z-dddd; do
    run "$s/resume" "$bad"
    rc_is 1
    err_has "resume: no session $bad in this folder; fix: /sessions lists them"
  done
  next_is ""
  run "$s/new"
  next_is new
  run "$s/reload"
  next_is restart
  # /fork: the messages by line; N alone is this session's, ID@N another's; a line that is no user record is refused.
  : >"$HENKI_NEXT"
  run "$s/fork" 20261108T080000Z-aaaa@4
  rc_is 0
  next_is "resume 20261108T080000Z-aaaa@4"
  : >"$HENKI_NEXT"
  for bad in 20261108T080000Z-aaaa@3 20261108T080000Z-aaaa@1 20261108T080000Z-aaaa@x 20261108T080000Z-aaaa@02; do
    run "$s/fork" "$bad"
    rc_is 1
  done
  err_has 'fork: "02" is not a line'
  run "$s/fork" 20261108T080000Z-aaaa@3
  err_has "fork: line 3 of session 20261108T080000Z-aaaa is not a message of yours; fix: /fork lists them"
  run "$s/fork" nope@2
  rc_is 1
  err_has "no session nope in std's store"
  next_is ""
  printf '4\n' >"$D/answer"
  RC=0
  OUT=$(HENKI_SESSION=20261108T080000Z-aaaa PATH=/usr/bin:/bin "$s/fork" <"$D/answer" 2>"$SCRATCH/err") || RC=$?
  rc_is 0
  out_has "     2  fix the \"parser\" please"
  out_has "     4  second"
  out_has "fork at line? "
  next_is "resume 20261108T080000Z-aaaa@4"
  RC=0
  OUT=$(HENKI_NEXT='' HENKI_SESSION=20261108T080000Z-aaaa PATH=/usr/bin:/bin "$s/fork" 2 2>"$SCRATCH/err") || RC=$?
  rc_is 0
  out_has "henki --resume=20261108T080000Z-aaaa@2"
  # One session only: /resume has nothing to switch to.
  rm -f "$HENKI_STATE/sessions/$sc"/*-aaaa.jsonl "$HENKI_STATE/sessions/$sc"/*-bbbb.jsonl
  : >"$HENKI_NEXT"
  run "$s/resume"
  rc_is 1
  err_has "no session other than this one"
  cd "$here" || return
  unset HENKI_BIN
  export HOME="$home"
}

t_prompts() {
  case_ "prompts: send the template, the argument after a blank line"
  export HENKI_MODULE="$MODS_DIR/prompts"
  local n
  for n in review explain commit-msg fix-test; do
    [ -L "$HENKI_MODULE/cmd/$n" ] && [ "$(readlink "$HENKI_MODULE/cmd/$n")" = ../.henki/template ] || fail "cmd/$n is not the template link"
    run "$HENKI_MODULE/cmd/$n"
    rc_is 0
    next_is "send$nl$(cat "$HENKI_MODULE/cmd/$n.md")"
    run "$HENKI_MODULE/cmd/$n" "the parser, and its tests"
    rc_is 0
    next_is "send$nl$(cat "$HENKI_MODULE/cmd/$n.md")$nl${nl}the parser, and its tests"
  done
  # A link to the template without its NAME.md (a prompt of your own, half made): it fails, and hands back nothing.
  mkdir -p "$D/mine/cmd" && ln -s "$HENKI_MODULE/.henki/template" "$D/mine/cmd/hi" && : >"$HENKI_NEXT"
  HENKI_MODULE="$D/mine" run "$D/mine/cmd/hi" "an argument"
  rc_is 1
  err_has "hi.md"
  next_is ""
}

t_clip() {
  case_ "clip: the first clipboard tool on PATH, else the fix"
  local b="$D/bin" t
  for t in pbpaste pbcopy wl-paste wl-copy xclip xsel; do
    printf '#!/bin/sh\necho "%s $*"\ncat >/dev/null\n' "$t" >"$b/$t"
  done
  chmod 755 "$b"/*
  P="$b:/usr/bin:/bin" run "$MODS_DIR/clip/bin/clip-get" </dev/null
  check_out() { [ "$OUT" = "$1" ] || fail "stdout is not: $1"; }
  check_out "pbpaste "
  P="$b:/usr/bin:/bin" run "$MODS_DIR/clip/bin/clip-put" </dev/null
  check_out "pbcopy "
  rm "$b/pbpaste" "$b/pbcopy"
  WAYLAND_DISPLAY=wayland-0 P="$b:/usr/bin:/bin" run "$MODS_DIR/clip/bin/clip-get" </dev/null
  check_out "wl-paste --no-newline"
  DISPLAY=:0 P="$b:/usr/bin:/bin" run env -u WAYLAND_DISPLAY "$MODS_DIR/clip/bin/clip-put" </dev/null
  check_out "xclip -selection clipboard -i"
  rm "$b/xclip"
  DISPLAY=:0 P="$b:/usr/bin:/bin" run env -u WAYLAND_DISPLAY "$MODS_DIR/clip/bin/clip-get" </dev/null
  check_out "xsel --clipboard --output"
  P="$b:/usr/bin:/bin" run env -u WAYLAND_DISPLAY -u DISPLAY "$MODS_DIR/clip/bin/clip-get" </dev/null
  rc_is 1
  err_has "clip-get: no clipboard here; fix: on Linux install wl-clipboard, xclip or xsel"
  # A refusing wrapper earlier on PATH (the halti mod's pbpaste) has its say.
  printf '#!/bin/sh\necho refused >&2\nexit 126\n' >"$b/pbpaste" && chmod 755 "$b/pbpaste"
  P="$b:/usr/bin:/bin" run "$MODS_DIR/clip/bin/clip-get" </dev/null
  rc_is 126
}

# A call_start event line for a NAME call with TARGET (JSON-escaped already; "" for none).
event() { # NAME TARGET
  local t=null
  [ -z "$2" ] || t="\"$2\""
  printf '{"data":{"call":{"args":{"name":"read","path":"x"},"id":"h1","name":"%s"},"rule":null,"target":%s,"verdict":"auto"},"event":"call_start","seq":3,"session":"%s","v":1}\n' "$1" "$t" "$HENKI_SESSION"
}

t_checkpoints() {
  case_ "checkpoints: snapshots in git and outside, /checkpoints, /undo, /rewind"
  local m="$MODS_DIR/checkpoints" repo="$D/repo" here n
  here=$(pwd)
  [ "$(readlink "$m/on/call_start.wait")" = ../cmd/checkpoints ] || fail "on/call_start.wait is not the link"
  export GIT_CEILING_DIRECTORIES="$D" HENKI_MODULE="$m"
  mkdir -p "$repo/sub" && cd "$repo" || return
  git init -q . && printf 'one\n' >a.txt && printf '*.log\n' >.gitignore && printf 'log\n' >c.log
  git add a.txt .gitignore && git -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -qm first
  printf 'untracked\n' >b.txt
  head=$(git rev-parse HEAD)
  cd sub || return
  event write "$repo/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 0
  cd "$repo" || return
  [ "$(git show "refs/henki/checkpoints/$HENKI_SESSION/1:a.txt")" = one ] || fail "the snapshot lacks a.txt"
  [ "$(git show "refs/henki/checkpoints/$HENKI_SESSION/1:b.txt")" = untracked ] || fail "the snapshot lacks the untracked b.txt"
  git cat-file -e "refs/henki/checkpoints/$HENKI_SESSION/1:c.log" 2>/dev/null && fail "the snapshot holds an ignored file"
  [ "$(git rev-parse HEAD)" = "$head" ] && git diff --cached --quiet && [ "$(git status --porcelain)" = "?? b.txt" ] || fail "HEAD, the index or the work tree changed"
  event read "$repo/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  event shell "" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  [ "$(git for-each-ref "refs/henki/checkpoints/$HENKI_SESSION/" | wc -l | tr -d ' ')" = 2 ] || fail "not one snapshot per write and shell call"
  run "$m/cmd/checkpoints"
  out_has "  1  "
  out_has "before write $repo/a.txt"
  out_has "before shell"
  # The edits after snapshot 1 and 2: a.txt changed, b.txt gone, d.txt new.
  printf 'two\n' >a.txt && rm b.txt && printf 'new\n' >d.txt
  run "$m/cmd/undo" <<<n
  out_has "a.txt"
  [ "$(cat a.txt)" = two ] && [ -f d.txt ] || fail "n changed the files"
  run "$m/cmd/undo" <<<y
  rc_is 0
  [ "$(cat a.txt)" = one ] && [ "$(cat b.txt)" = untracked ] && [ ! -e d.txt ] && [ "$(cat c.log)" = log ] || fail "/undo did not put the files back"
  out_has "back at checkpoint 2"
  [ "$(git for-each-ref "refs/henki/checkpoints/$HENKI_SESSION/" | wc -l | tr -d ' ')" = 1 ] || fail "/undo did not drop the snapshot"
  printf 'three\n' >a.txt
  run "$m/cmd/rewind" x
  err_has "usage: /rewind N"
  run "$m/cmd/rewind" 9
  err_has "no checkpoint 9"
  run "$m/cmd/rewind" 1 <<<y
  [ "$(cat a.txt)" = one ] || fail "/rewind 1 did not put a.txt back"
  run "$m/cmd/checkpoints"
  out_has "no checkpoints in this session"
  run "$m/cmd/undo"
  rc_is 1
  err_has "no checkpoints in this session"
  # An edit that drops an ignore rule: the file it hid was in no snapshot, so /undo keeps it, and says what goes.
  mkdir -p secret && printf 'KEY\n' >secret/key && printf '*.log\nsecret/\n' >.gitignore
  event shell "" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  : >.gitignore && printf 'new\n' >d.txt
  run "$m/cmd/undo" <<<y
  rc_is 0
  out_has "to be removed:${nl}  d.txt${nl}put these"
  case "$OUT" in *secret* | *c.log*) fail "/undo lists a file the snapshot's rules ignore" ;; esac
  [ "$(cat secret/key)" = KEY ] && [ "$(cat c.log)" = log ] || fail "/undo removed a file the snapshot's rules ignore"
  [ ! -e d.txt ] && [ "$(cat .gitignore)" = "*.log${nl}secret/" ] || fail "/undo did not put the files back"
  # A repo whose config names programs for git to run (a model can write .git/config): no snapshot or /undo runs one.
  mkdir -p "$D/trap/hooks" && cd "$D/trap" || return
  printf '#!/bin/sh\necho >>"%s/ran"; cat\n' "$D" >"$D/prog" && chmod 755 "$D/prog" && cp "$D/prog" hooks/reference-transaction
  git init -q . && git config core.fsmonitor "$D/prog" && git config core.hooksPath "$D/trap/hooks" && git config core.pager "$D/prog"
  for k in clean smudge process; do git config "filter.x.$k" "$D/prog"; done
  git config filter.x.required true && printf '* filter=x\n' >.gitattributes && printf 'one\n' >a.txt
  event write "$D/trap/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 0
  printf 'two\n' >a.txt
  run "$m/cmd/undo" <<<y
  rc_is 0
  [ "$(cat a.txt)" = one ] || fail "/undo did not put a.txt back in a repo with filters"
  [ ! -e "$D/ran" ] || fail "a program the repo's config names ran: $(wc -l <"$D/ran") times"
  # Outside git: the write and edit targets, kept or noted absent.
  mkdir -p "$D/plain" && cd "$D/plain" || return
  printf 'old\n' >x.txt
  event edit "$D/plain/x.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  event write "$D/plain/y.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  event shell "" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  n=$(ls "$HENKI_STATE/checkpoints/$HENKI_SESSION" | tr '\n' ' ')
  [ "$n" = "1 2 " ] || fail "outside git: snapshots $n"
  printf 'new\n' >x.txt && printf 'made\n' >y.txt
  run "$m/cmd/checkpoints"
  out_has "2  before write or edit $D/plain/y.txt (absent)"
  run "$m/cmd/undo" <<<y
  [ ! -e y.txt ] || fail "/undo did not remove the file made after the snapshot"
  run "$m/cmd/undo" <<<y
  [ "$(cat x.txt)" = old ] || fail "/undo did not put x.txt back"
  event write 'a\\nb' >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 1
  err_has "no plain path in the event"
  # A target that is not UTF-8 comes as null: nothing is kept, and nothing named after a lossy path.
  event write "" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 1
  err_has "cannot keep the target"
  [ "$(ls "$HENKI_STATE/checkpoints/$HENKI_SESSION")" = missed ] || fail "a null target was kept"
  run "$m/cmd/checkpoints"
  out_has "no checkpoint before write: cannot keep the target: no plain path"
  HENKI_SESSION="" run env "$m/cmd/checkpoints"
  rc_is 1
  err_has "no session (HENKI_SESSION)"
  cd "$here" || exit 1
  unset GIT_CEILING_DIRECTORIES
}

t_checkpoints_missed() {
  case_ "checkpoints: what git cannot read is left out and said; a filter git -c cannot turn off; a racy index"
  local m="$MODS_DIR/checkpoints" here
  here=$(pwd)
  export GIT_CEILING_DIRECTORIES="$D" HENKI_MODULE="$m"
  mkdir -p "$D/repo/shut" && cd "$D/repo" || return
  # A nested repo with no commit fails git add for anyone; mode 000 does for all but root.
  git init -q . && git init -q dep && printf 'one\n' >a.txt && printf 'x\n' >locked && printf 'x\n' >shut/f && chmod 000 locked shut
  event write "$D/repo/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 0
  [ "$(git show "refs/henki/checkpoints/$HENKI_SESSION/1:a.txt")" = one ] || fail "the snapshot lacks a.txt"
  run "$m/on/call_start.wait" <"$D/ev"
  [ "$(git for-each-ref "refs/henki/checkpoints/$HENKI_SESSION/" | wc -l | tr -d ' ')" = 2 ] || fail "not one snapshot per call"
  run "$m/cmd/checkpoints"
  out_has "before write $D/repo/a.txt (partial)"
  out_has "  checkpoint 2, before write $D/repo/a.txt, lacks what git could not read: "
  out_has "'dep/' does not have a commit checked out;"
  [ -r locked ] || { out_has "could not open directory 'shut/': Permission denied;" && out_has "unable to index file 'locked'"; }
  printf 'two\n' >a.txt
  run "$m/cmd/undo" <<<y
  rc_is 0
  out_has "a partial one"
  [ "$(cat a.txt)" = one ] && [ -d dep/.git ] || fail "/undo did not put a.txt back, or removed what git could not read"
  chmod 755 locked shut
  # [filter "a=b"]: git -c would read filter.a=b.clean= as filter.a, so the clean program would run.
  printf '#!/bin/sh\necho >>"%s/ran"; cat\n' "$D" >"$D/prog" && chmod 755 "$D/prog"
  git config 'filter.a=b.clean' "$D/prog" && printf '* filter=a=b\n' >.gitattributes
  run "$m/on/call_start.wait" <"$D/ev"
  rc_is 1
  err_has "a filter driver with \"=\" in its name"
  run "$m/cmd/undo" <<<y
  rc_is 1
  out_has "no checkpoint before write $D/repo/a.txt: the repo's config names a filter driver"
  [ ! -e "$D/ran" ] || fail "the filter's program ran"
  # Outside git, a target that cannot be copied leaves no snapshot for /undo to fail on.
  mkdir -p "$D/plain" && cd "$D/plain" && printf 'x\n' >x.txt && chmod 000 x.txt || return
  event edit "$D/plain/x.txt" >"$D/ev"
  if [ ! -r x.txt ]; then
    run "$m/on/call_start.wait" <"$D/ev"
    rc_is 1
    err_has "cannot copy $D/plain/x.txt"
    [ "$(ls "$HENKI_STATE/checkpoints/$HENKI_SESSION")" = missed ] || fail "a copy that failed left a snapshot"
    run "$m/cmd/checkpoints"
    out_has "no checkpoint before edit $D/plain/x.txt: cannot copy"
  fi
  chmod 644 x.txt
  # A file changed in the second the index was written has the stat data git keeps for it (ctime aside, which this
  # repo's config does not trust): git reads it again only as long as the index's mtime says so.
  mkdir -p "$D/racy" && cd "$D/racy" && git init -q . && git config core.trustctime false && printf 'one\n' >a.txt || return
  touch -t 202301010000 a.txt && git add a.txt && printf 'two\n' >a.txt && touch -t 202301010000 a.txt .git/index
  event write "$D/racy/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  [ "$(git show "refs/henki/checkpoints/$HENKI_SESSION/1:a.txt")" = two ] || fail "the snapshot holds a.txt as the index has it"
  cd "$here" || exit 1
  unset GIT_CEILING_DIRECTORIES
}

t_checkpoints_kept() {
  case_ "checkpoints: a target the commit cannot hold is kept by path; names are shown quoted"
  local m="$MODS_DIR/checkpoints" here out
  here=$(pwd)
  export GIT_CEILING_DIRECTORIES="$D" HENKI_MODULE="$m"
  mkdir -p "$D/repo" "$D/outside" && cd "$D/repo" || return
  git init -q . && printf '*.log\n' >.gitignore && printf 'log\n' >c.log && printf 'one\n' >a.txt && git init -q nested &&
    git -C nested -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m first
  printf 'orig\n' >"$D/outside/o.txt"
  # Outside the work tree, a file to be made there, an ignored file in it, a file in a repo nested in it.
  for t in "$D/outside/o.txt" "$D/outside/new.txt" "$D/repo/c.log" "$D/repo/nested/n.txt"; do
    event write "$t" >"$D/ev"
    run "$m/on/call_start.wait" <"$D/ev"
    rc_is 0
  done
  event edit "$D/repo/a.txt" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  [ "$(ls "$HENKI_STATE/checkpoints/$HENKI_SESSION" | tr '\n' ' ')" = "1 2 3 4 " ] || fail "not one kept target per call the commit cannot hold"
  printf 'clobbered\n' >"$D/outside/o.txt" && printf 'made\n' >"$D/outside/new.txt" && printf 'x\n' >c.log && printf 'n\n' >nested/n.txt
  run "$m/cmd/checkpoints"
  out_has "  1  "
  out_has "before write $D/outside/o.txt (target kept by path)"
  case "$OUT" in *"a.txt (target kept"*) fail "a target the commit holds was kept by path" ;; esac
  run "$m/cmd/undo" <<<y
  out_has "nothing changed since checkpoint 5"
  run "$m/cmd/rewind" 1 <<<y
  rc_is 0
  out_has "kept by path, to be put back as before the call:$nl  $D/outside/o.txt$nl  $D/outside/new.txt (absent then: to be removed)$nl  $D/repo/c.log$nl"
  out_has "back at checkpoint 1"
  [ "$(cat "$D/outside/o.txt")" = orig ] && [ ! -e "$D/outside/new.txt" ] && [ "$(cat c.log)" = log ] && [ ! -e nested/n.txt ] ||
    fail "/rewind did not put the kept targets back"
  [ ! -e "$HENKI_STATE/checkpoints/$HENKI_SESSION/1" ] || fail "/rewind did not drop the kept targets"
  # A made file whose name holds terminal controls (OSC 52, cursor moves) is listed as git quotes it, never raw.
  event shell "" >"$D/ev"
  run "$m/on/call_start.wait" <"$D/ev"
  touch "$(printf 'x\033]52;c;ZWNobw==\007\033[2K\033[1Ay')"
  run "$m/cmd/undo" <<<n
  out_has 'made since then, to be removed:'
  out_has '  "x\033]52;c;ZWNobw==\a\033[2K\033[1Ay"'
  out=$(printf '%s' "$OUT" | tr -d '\033\007')
  [ "$out" = "$OUT" ] || fail "a control character reached the terminal raw"
  cd "$here" || exit 1
  unset GIT_CEILING_DIRECTORIES
}

# The project mod's context source, run as henki runs it: in the working directory, stdin /dev/null, HENKI_CONFIG set.
t_project() {
  case_ "project: AGENTS.md or CLAUDE.md at the git root, cut, links, CRLF and NBSP, HOME, fsmonitor"
  local src="$MODS_DIR/project/ctx/project" repo="$D/repo" here big i
  here=$(pwd)
  export GIT_CEILING_DIRECTORIES="$D" HENKI_CONFIG="$D/config"
  mkdir -p "$repo/sub" "$D/config" "$D/plain" && cd "$repo/sub" || return
  git init -q -b main "$repo"
  run "$src" </dev/null
  rc_is 0
  [ "$OUT" = "git: $repo on main" ] || fail "no file: not the git line alone"
  printf 'Use make.\n' >"$repo/CLAUDE.md"
  run "$src" </dev/null
  [ "$OUT" = "# project instructions ($repo/CLAUDE.md)$nl${nl}Use make.$nl${nl}git: $repo on main" ] || fail "CLAUDE.md alone"
  printf 'Run the tests first.\n' >"$repo/AGENTS.md"
  run "$src" </dev/null
  [ "$OUT" = "# project instructions ($repo/AGENTS.md)$nl${nl}Run the tests first.$nl${nl}git: $repo on main" ] || fail "AGENTS.md wins"
  # A link is never read, whatever it names (a secret outside the project): CLAUDE.md is used instead.
  printf 'SECRET\n' >"$D/secret"
  mv "$repo/AGENTS.md" "$repo/real.md" && ln -s "$D/secret" "$repo/AGENTS.md"
  run "$src" </dev/null
  out_has "($repo/AGENTS.md is a link; not read)$nl$nl# project instructions ($repo/CLAUDE.md)$nl${nl}Use make."
  case "$OUT" in *SECRET*) fail "a linked AGENTS.md was read" ;; esac
  rm "$repo/AGENTS.md" && ln -s real.md "$repo/AGENTS.md"
  run "$src" </dev/null
  case "$OUT" in *"Run the tests"*) fail "a link inside the project was read" ;; esac
  rm "$repo/AGENTS.md" && mv "$repo/real.md" "$repo/AGENTS.md"
  # CRLF ends, NBSP and a byte order mark are formatting: gone, a space, gone; the rest is the file.
  printf '\357\273\277One\302\240two\r\nthree\r\n' >"$repo/AGENTS.md"
  run "$src" </dev/null
  out_has "$nl${nl}One two${nl}three$nl$nl"
  [ "$(printf '%s' "$OUT" | LC_ALL=C tr -d '\r\240\273\277\357\302' | wc -c)" = "$(printf '%s' "$OUT" | wc -c)" ] || fail "a CR, NBSP or BOM byte is left"
  # Over 15,000 bytes: cut at a line end, with a line saying so; the whole stays within a source's 16 KiB.
  big=""
  i=0
  while [ $i -lt 400 ]; do
    big="$big$(printf 'line %03d: %s' $i 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx')$nl"
    i=$((i + 1))
  done
  printf '%s' "$big" >"$repo/AGENTS.md"
  run "$src" </dev/null
  out_has "${nl}line 364: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx$nl[cut here: $repo/AGENTS.md is longer; read it for the rest]$nl${nl}git: $repo on main"
  case "$OUT" in *"line 365"*) fail "past the cut" ;; esac
  [ "$(printf '%s' "$OUT" | wc -c)" -le 16384 ] || fail "over 16 KiB"
  # The global AGENTS.md (HENKI_CONFIG's) is never printed: henki loads it itself.
  printf 'Be brief.\n' >"$repo/AGENTS.md"
  HENKI_CONFIG="$repo" run "$src" </dev/null
  case "$OUT" in *"Be brief"*) fail "the global AGENTS.md was printed" ;; esac
  out_has "Use make."
  # A detached HEAD; then a repository's fsmonitor hook never runs (nothing reads the index).
  git -C "$repo" -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -q --allow-empty -m first
  git -C "$repo" checkout -q --detach
  printf '#!/bin/sh\ntouch %s/fsmonitor-ran\n' "$D" >"$D/fsmon" && chmod 755 "$D/fsmon"
  git -C "$repo" config core.fsmonitor "$D/fsmon"
  run "$src" </dev/null
  out_has "git: $repo on $(git -C "$repo" rev-parse --short HEAD)"
  [ ! -e "$D/fsmonitor-ran" ] || fail "the repository's core.fsmonitor ran"
  # Nothing outside git, nor in a repository at or above HOME (a dotfiles repo).
  cd "$D/plain" || return
  run "$src" </dev/null
  [ "$RC:$OUT" = "0:" ] || fail "outside git: $OUT"
  cd "$repo/sub" || return
  for h in "$repo" "$repo/sub"; do
    HOME="$h" HENKI_CONFIG="" run "$src" </dev/null
    [ "$RC:$OUT" = "0:" ] || fail "a repo at or above HOME=$h: $OUT"
  done
  cd "$here" || exit 1
  unset GIT_CEILING_DIRECTORIES HENKI_CONFIG
}

t_notify() {
  case_ "notify: turn_end and question through the first notifier on PATH"
  local b="$D/bin"
  [ "$(readlink "$MODS_DIR/notify/on/question")" = turn_end ] || fail "on/question is not the link"
  printf '#!/bin/sh\nprintf "%%s|" "$@"\n' >"$b/notify-send" && chmod 755 "$b/notify-send"
  printf '{"data":{"status":"ok","steps":2,"t":"turn_end","ts":"x","usage":{}},"event":"turn_end","seq":9,"session":"s","v":1}\n' >"$D/ev"
  HENKI_EVENT=turn_end P="$b:/usr/bin:/bin" run "$MODS_DIR/notify/on/turn_end" <"$D/ev"
  [ "$OUT" = "--|henki: ${PWD##*/}|turn ended: ok|" ] || fail "turn_end: $OUT"
  printf '{"data":{"args":{"command":"ls"},"id":"h1","name":"shell","t":"ask","ts":"x"},"event":"question","seq":9,"session":"s","v":1}\n' >"$D/ev"
  HENKI_EVENT=question P="$b:/usr/bin:/bin" run "$MODS_DIR/notify/on/question" <"$D/ev"
  [ "$OUT" = "--|henki: ${PWD##*/}|asks before a shell call|" ] || fail "question: $OUT"
  printf '#!/bin/sh\nprintf "%%s|" "$@"\n' >"$b/osascript" && chmod 755 "$b/osascript"
  HENKI_EVENT=turn_end P="$b:/usr/bin:/bin" run "$MODS_DIR/notify/on/turn_end" <<<'{}'
  out_has "display notification (item 2 of argv) with title (item 1 of argv)|-e|end run|henki: "
  rm "$b/osascript" "$b/notify-send"
  HENKI_EVENT=turn_end P="$b:/usr/bin:/bin" run "$MODS_DIR/notify/on/turn_end" <<<'{}'
  err_has "notify: no osascript or notify-send on PATH"
}

# compact, the context binding, with a stub model server on loopback (python3; compact needs it too): keep below its
# threshold, a summary record then the history's own records past it, the cache, the model in the cache key, a dead,
# remote or missing model leaves every request to henki; with MODS_TEST_HENKI, a henki session bound to it, whose
# requests carry the summary henki accepted while the journal keeps everything.
t_compact() {
  case_ "compact: keep, summaries, the cache, a dead or remote model, through henki"
  command -v python3 >/dev/null 2>&1 || { echo "skipped: compact (no python3)"; return 0; }
  cat >"$D/compact_tests.py" <<'PY'
import json, os, shutil, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
MOD, FIX, D, HENKI = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
PROG = os.path.join(MOD, "port", "context")
PREFIX = "Summary of the earlier part of this session, written by the assistant"
FAILS = []
STUB = {"mode": "ok", "log": []}


def check(what, ok, detail=""):
    if not ok:
        FAILS.append(what)
        print("FAIL compact: %s%s" % (what, ": " + str(detail)[:400] if detail else ""))


class Stub(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)))
        STUB["log"].append((dict(self.headers), body))
        if STUB["mode"] == "error":
            data, code = b'{"error":{"message":"stub"}}', 500
        else:
            ask = str(body["messages"][-1].get("content", ""))
            # henki's own requests carry tools: a long answer, so its history grows; compact's get a summary.
            text = ("Answer %d. " % len(STUB["log"])) * 200 if "tools" in body else "<think>x</think>Asked: %s‮" % ask[:60]
            data, code = json.dumps({"choices": [{"finish_reason": "stop", "index": 0, "message": {"content": text, "role": "assistant"}}],
                                     "usage": {"completion_tokens": 5, "prompt_tokens": 50}}).encode(), 200
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


srv = ThreadingHTTPServer(("127.0.0.1", 0), Stub)
threading.Thread(target=srv.serve_forever, daemon=True).start()
URL = "http://127.0.0.1:%d/v1" % srv.server_address[1]


def home(compact=None, config=None):
    h = os.path.join(D, "home%d" % len(os.listdir(D)))
    os.makedirs(os.path.join(h, ".config", "henki"))
    for name, text in (("compact.toml", compact), ("config.toml", config)):
        if text is not None:
            with open(os.path.join(h, ".config", "henki", name), "w") as f:
                f.write(text)
    return h


def talk(h, reqs):
    """compact run as henki runs it, with HOME h: hello, the requests, bye. The answers after hello, and stderr."""
    lines = [{"id": 0, "method": "hello", "params": {"port": "context", "protocol": [1], "versions": [1], "state": os.path.join(h, "state")}}]
    lines += [{"id": i + 1, "method": m, "params": p} for i, (m, p) in enumerate(reqs)] + [{"id": len(reqs) + 1, "method": "bye", "params": {}}]
    env = {"HOME": h, "PATH": os.environ["PATH"], "http_proxy": "http://127.0.0.1:9"}
    t = time.time()
    r = subprocess.run([PROG], input="".join(json.dumps(x) + "\n" for x in lines), capture_output=True, text=True, env=env, timeout=60)
    got = [json.loads(x) for x in r.stdout.splitlines()]
    check("one answer per request", len(got) == len(lines), r.stdout + r.stderr)
    return got[1:-1], r.stderr, got[0], time.time() - t


def fixture(name):
    with open(os.path.join(FIX, name + ".json")) as f:
        return json.load(f)


def size(recs, fixed):
    return fixed + sum(len(json.dumps(r, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()) + 1 for r in recs)


def fit(recs, window, fixed=2500, overflowed=False):
    return ("fit", {"budget": {"fixed": fixed, "limit": window * 16 // 5, "max_tokens": 1024, "window": window}, "overflowed": overflowed, "records": recs, "session": "s"})


long_ = fixture("long")
cfg = 'base_url = "%s"\nmodel = "stub-model"\n' % URL
h = home(cfg)
got, err, hello, _ = talk(h, [("x-none", {}), fit(long_[:24], 7500)])
check("hello", hello.get("result") == {"caps": [], "impl": "0.1.0", "name": "compact", "protocol": 1, "version": 1}, hello)
check("an unknown method is unsupported", got[0].get("error", {}).get("code") == "unsupported", got[0])
check("below start x limit: keep, no completion", got[1].get("result") == {"keep": True} and not STUB["log"], (got[1], len(STUB["log"])))
got, err, _, _ = talk(h, [("x-none", {}), ("x-none", {}), fit(long_, 7500), fit(long_, 7500, overflowed=True)])
recs = got[2].get("result", {}).get("records") or [{}]
head = recs[0].get("msg", {}).get("content", "")
check("over it: a summary record, then the history's own last records", recs[0].get("t") == "user" and head.startswith(PREFIX) and recs[1:] == long_[-(len(recs) - 1):], recs[:2])
check("the summary is cleaned (no think block, no U+202E)", "<think>" not in head and "‮" not in head and "Asked: " in head, head[-120:])
check("within the limit", size(recs, 2500) <= 24000, size(recs, 2500))
headers, asked = STUB["log"][0]
check("one completion of the model compact was given, no key, not streamed", len(STUB["log"]) == 2 and asked["model"] == "stub-model" and asked["stream"] is False
      and not any(k.lower() == "authorization" for k in headers), (len(STUB["log"]), asked.get("model")))
users = [r for r in got[3].get("result", {}).get("records", []) if r.get("t") == "user"]
check("overflowed: the summary and one turn, within start x limit", len(users) == 2 and size(got[3]["result"]["records"], 2500) <= 0.6 * 24000, len(users))
n = len(STUB["log"])
again, _, _, _ = talk(h, [fit(long_, 7500)])
check("a new instance reuses the cached summary", again[0].get("result") == got[2].get("result") and len(STUB["log"]) == n, len(STUB["log"]) - n)
with open(os.path.join(h, ".config", "henki", "compact.toml"), "w") as f:
    f.write('base_url = "%s"\nmodel = "other-model"\n' % URL)
talk(h, [fit(long_, 7500)])
check("another model's summary is not reused", len(STUB["log"]) == n + 1 and STUB["log"][-1][1]["model"] == "other-model", len(STUB["log"]) - n)
got, _, _, _ = talk(h, [fit(fixture("one_turn"), 625)])
check("one turn over the limit alone: keep", got[0].get("result") == {"keep": True}, got[0])
for text, why in (('base_url = "http://127.0.0.1:9/v1"\nmodel = "m"\n', ""), ('base_url = "http://192.0.2.1:8080/v1"\nmodel = "m"\n', "not loopback"),
                  ('base_url = "https://127.0.0.1:1/v1"\nmodel = "m"\n', "not loopback"), (None, "no base_url and model")):
    n = len(STUB["log"])
    got, err, _, took = talk(home(text), [fit(long_, 7500), fit(long_, 7500)])
    check("a dead, remote or missing model (%s): keep, quickly, said once" % (text or "none").split("\n")[0], all(g.get("result") == {"keep": True} for g in got)
          and took < 5 and err.count(why) == (1 if why else err.count(why)) and len(STUB["log"]) == n, (got, err, took))
STUB["mode"] = "error"
got, err, _, _ = talk(home(cfg), [fit(long_, 7500)])
check("a model error: keep, and a log line", got[0].get("result") == {"keep": True} and "failed" in err, err)
STUB["mode"] = "ok"
check("henki's config.toml gives base_url and model", talk(home(None, cfg), [fit(long_, 7500)])[0][0].get("result", {}).get("records"), "no records")

if HENKI:
    # A henki session bound to compact: three turns of long answers in a small window. The last request starts with the
    # summary henki took (no refusal notice), and the journal keeps every turn.
    h = home(None, cfg + 'approve = "auto"\ncontext_window = 4096\n')
    os.makedirs(os.path.join(h, ".config", "henki", "port"))
    os.symlink(PROG, os.path.join(h, ".config", "henki", "port", "context"))
    work = os.path.join(h, "work")
    os.makedirs(work)
    env = {"HOME": h, "PATH": os.environ["PATH"], "XDG_STATE_HOME": os.path.join(h, "state")}
    STUB["log"] = []
    out = ""
    for i, args in enumerate([["first"], ["--resume", "second"], ["--resume", "third"], ["--resume", "fourth"]]):
        r = subprocess.run([HENKI, "run", "--json"] + args, capture_output=True, text=True, env=env, cwd=work, timeout=60)
        out += r.stdout
        check("henki run %d ends ok" % i, r.returncode == 0, r.stdout[-600:] + r.stderr)
    recs = [json.loads(x) for x in out.splitlines()]
    check("no answer of compact refused", not [x for x in recs if x.get("t") == "notice" and "std elide" in x.get("text", "")], [x.get("text") for x in recs if x.get("t") == "notice"])
    mine = [b for _, b in STUB["log"] if "tools" in b]
    first = [str(m.get("content")) for m in mine[-1]["messages"] if m["role"] == "user"]
    check("the last request starts with the summary, then the last turns", first[0].startswith(PREFIX) and len(first) == 3, [f[:40] for f in first])
    journal = [json.loads(x) for d, _, fs in os.walk(os.path.join(h, "state", "henki", "sessions")) for f in fs for x in open(os.path.join(d, f))]
    check("the journal keeps every turn", len([x for x in journal if x.get("t") == "user"]) == 4, len(journal))
srv.shutdown()
sys.exit(1 if FAILS else 0)
PY
  RC=0
  mkdir -p "$D/homes"
  OUT=$(python3 -I "$D/compact_tests.py" "$MODS_DIR/compact" "$MODS_DIR/../manager/conform/fixtures" "$D/homes" "${MODS_TEST_HENKI:-}" 2>&1 </dev/null) || RC=$?
  [ "$RC" = 0 ] || { FAILS=$((FAILS + 1)) && printf '%s\n' "$OUT" >&2; }
}

# sandbox, the exec binding: per backend that works here (bwrap, user namespaces, Seatbelt), exec/1's checks and the
# sandbox's own (the home and henki's dirs hidden, writes kept to the project and the temp dir, control sockets, the
# host's terminals, /dev/shm, IPC and processes out of reach, a TERM during the setup, the refusals), then the bwrap
# argv from a stand-in bwrap. A Linux machine with neither backend fails the case: the sandbox would run nothing there.
t_sandbox() {
  case_ "sandbox: exec/1's checks and what a call can reach, per backend"
  local b="" why=""
  if [ "$(uname -s)" = Darwin ]; then
    b=seatbelt
  else
    if command -v bwrap >/dev/null 2>&1; then
      if bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid true 2>/dev/null; then b=bwrap; else why="bwrap cannot mount a /proc here"; fi
    fi
    if unshare -Urm true 2>/dev/null; then b="$b ns"; fi
  fi
  [ -n "$why" ] && echo "sandbox: the bwrap backend not run ($why)"
  if [ -z "$b" ]; then
    FAILS=$((FAILS + 1))
    echo "FAIL $CASE: no backend works here (no bwrap, no user namespaces)" >&2
    return
  fi
  cat >"$D/sandbox-tests.sh" <<'SH'
#!/bin/bash
# The sandbox's checks inside one call per check, per backend: sandbox-tests.sh PORT_EXEC HENKI_MODS BACKEND...
# Hermetic: a scratch HOME, XDG dirs, agent socket dir and TMPDIR under one temp root; the real home is only counted.
set -u
exe=$1 mods=$2
shift 2
bash_abs=$(command -v bash)
fails=0
saved=$(export -p | command grep -E "^(declare -x|export) (HOME|TMPDIR|XDG_STATE_HOME|XDG_RUNTIME_DIR|SSH_AUTH_SOCK|TMUX|TMUX_TMPDIR)=")
ok() { printf 'ok   %s %s\n' "$be" "$1"; }
bad() { printf 'FAIL %s %s\n' "$be" "$1"; fails=$((fails + 1)); }
check() { # NAME EXPECT(0=must succeed, 1=must fail) COMMAND: COMMAND runs inside, in $proj
  r=$(cd "$proj" && HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c "$3" 2>&1 </dev/null); s=$?
  if [ "$2" = 0 ]; then [ "$s" = 0 ] && ok "$1" || bad "$1 (exit $s: $r)"
  else [ "$s" != 0 ] && [ "$s" != 126 ] && ok "$1" || bad "$1 (exit $s, want a failure of the command: $r)"
  fi
}
inside() { (cd "$proj" && HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c "$1" 2>&1 </dev/null); }
# A same-uid unix socket server on the host (python, never run inside); `conn PATH` is the client run inside.
serve() { python3 -I -c 'import socket, sys, time
s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.listen(8); time.sleep(600)' "$1" & srv="$srv $!"
  n=0; while [ ! -S "$1" ] && [ "$n" -lt 40 ]; do sleep 0.05; n=$((n + 1)); done; }
conn() { printf "python3 -I -c 'import socket, sys; socket.socket(socket.AF_UNIX).connect(sys.argv[1])' '%s'" "$1"; }
uid=$(id -u)
# A dir of the person's outside the home, the temp dirs and the project that stays visible inside: in the checkout
# (EXE is CHECKOUT/mods/sandbox/port/exec). Only every mount being read-only keeps a write there from the host.
out=$(cd -P "${exe%/*}/../../.." && mktemp -d "$PWD/.sandbox-test.XXXXXX") || { echo "FAIL no dir in the checkout"; exit 1; }
# Inside, every mount but the project, the temp dir, the kernel's, the hides (a tmpfs with dirs only) and the masks
# (/dev/null over a file) is read-only: prints `root ro`, then the others. Per mount point the last entry is the one on
# top; a mount point that is not there inside (under a hide) cannot be reached.
ro_all='awk '"'"'{ i = 7; while ($i != "-") i++; split($6, o, ","); top[$5] = o[1] " " $(i + 1) }
  END { for (m in top) print top[m], m }'"'"' /proc/self/mountinfo |
  while read -r rw t m; do
    [ "$m" = / ] && echo "root $rw"
    [ "$rw" = ro ] || [ ! -e "$m" ] && continue
    case $m in "$PWD" | "$TMPDIR" | /proc | /proc/* | /sys | /sys/* | /dev | /dev/*) continue ;; esac
    [ -c "$m" ] && continue
    [ "$t" = tmpfs ] && [ -z "$(find "$m" -xdev -mindepth 1 ! -type d 2>/dev/null | head -n 1)" ] && continue
    echo "writable: $m ($t)"
  done | sort'

for be in "$@"; do
  root=$(mktemp -d "${TMPDIR:-/tmp}/sandbox-test.XXXXXX"); root=$(cd -P "$root" && pwd)
  H=$root/home
  mkdir -p "$H/.ssh" "$H/.config/henki" "$H/.local/state/henki" "$H/code/projB" "$root/projA" \
    "$root/xstate/henki" "$root/run" "$root/agent" "$root/tmp"
  echo secret-kh >"$H/.ssh/known_hosts"; echo 'model = "x"' >"$H/.config/henki/config.toml"
  echo secret-state >"$root/xstate/henki/s"; echo secret-run >"$root/run/r"; echo sock >"$root/agent/sock"
  # The real home's entries are counted only, never read.
  # Inside, it holds no file: at most the empty dirs that hide what lay under it (bwrap makes them; so does ns-setup).
  realhome_n=$(HOME=$HOME; cd "$root/projA" && HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c 'find "$HOME" -mindepth 1 ! -type d | wc -l' 2>&1 </dev/null)
  [ "$(echo $realhome_n)" = 0 ] && ok "the real home holds no file inside" || bad "the real home is not empty inside: $realhome_n"
  # Control sockets: tmux by $TMUX and by TMUX_TMPDIR, an agent socket right in TMPDIR, a server under /tmp outside TMPDIR.
  srv=; hostt=$(mktemp -d /tmp/sandbox-test-host.XXXXXX)
  mkdir -p "$root/tmuxdir/tmux-$uid" "$root/tmuxenv"
  serve "$root/tmuxdir/tmux-$uid/default"; serve "$root/tmuxenv/default"; serve "$root/tmp/agent.sock"
  serve "$hostt/sock"
  export HOME=$H TMPDIR=$root/tmp XDG_STATE_HOME=$root/xstate XDG_RUNTIME_DIR=$root/run SSH_AUTH_SOCK=$root/agent/sock \
    TMUX_TMPDIR=$root/tmuxdir TMUX=$root/tmuxenv/default,4242,0
  for proj in "$root/projA" "$H/code/projB"; do
    case $proj in "$H"/*) where="project in home" ;; *) where="project outside home" ;; esac
    printf -- '-- %s %s\n' "$be" "$where"
    # (a bwrap on PATH that cannot set up here, as in some containers, is passed over)
    r=$(cd "$proj" && env -u HENKI_SANDBOX HENKI_USER_PATH="$PATH" "$exe" "$bash_abs" --noprofile --norc -c 'echo ran' 2>&1 </dev/null); s=$?
    [ "$s" = 0 ] && [ "$r" = ran ] && ok "the auto pick finds a backend that works" || bad "the auto pick fails (exit $s: $r)"
    r=$(cd "$proj" && HENKI_SANDBOX=$be bash "$mods" test --port exec "$exe" 2>&1 </dev/null); s=$?
    [ "$s" = 0 ] && ok "exec/1 R01-R07 ($where)" || bad "exec/1 R01-R07 ($where): $r"
    check "cat ~/.ssh/known_hosts fails" 1 'cat ~/.ssh/known_hosts'
    check "henki's config is hidden" 1 'cat ~/.config/henki/config.toml'
    check "henki state under XDG_STATE_HOME hidden" 1 'cat "$XDG_STATE_HOME/henki/s"'
    check "XDG_RUNTIME_DIR hidden" 1 'cat "$XDG_RUNTIME_DIR/r"'
    check "the agent socket's dir hidden" 1 "cat '$root/agent/sock'"
    check "SSH_AUTH_SOCK is unset" 0 '[ -z "${SSH_AUTH_SOCK+x}" ]'
    check "a write to /etc/x fails" 1 'echo x >/etc/x'
    if inside "[ -d '$out' ] && echo seen" | command grep -qx seen; then
      check "a write outside the project fails" 1 "echo x >'$out/x'"
      [ ! -e "$out/x" ] && ok "no write outside the project reaches the host" || { bad "a write outside the project reached the host"; rm -f "$out/x"; }
    else echo "sandbox: $be: the write outside the project not tried ($out is hidden inside)"
    fi
    if [ "$be" != seatbelt ]; then
      r=$(inside "$ro_all"); [ "$r" = "root ro" ] && ok "every other mount is read-only" || bad "mounts left writable: $r"
    fi
    check "a write to the project works" 0 'echo x >in-project && [ -s in-project ]'
    check "a write to TMPDIR works" 0 'echo x >"$TMPDIR/in-tmp"'
    [ -f "$proj/in-project" ] && [ -f "$root/tmp/in-tmp" ] && ok "project and TMPDIR writes reach the host" || bad "project or TMPDIR write lost"
    inside 'echo x >~/x' >/dev/null; [ ! -e "$H/x" ] && ok "a write to ~/x does not reach the home" || bad "~/x reached the home"
    check "the uid is the person's" 0 "[ \"\$(id -u)\" = $(id -u) ]"
    if [ "$be" != seatbelt ]; then
      check "umount of the home fails" 1 'umount "$HOME" 2>&1 && ls -A "$HOME" | command grep -q .ssh'
      check "a nested namespace cannot unmount it" 1 'unshare -rm sh -c "umount \"\$HOME\" 2>/dev/null; ls -A \"\$HOME\"" | command grep -q .ssh'
      check "a remount read-write fails" 1 "mount -o remount,rw / 2>/dev/null || unshare -rm sh -c 'mount -o remount,bind,rw $out' 2>/dev/null; echo x >'$out/y'"
      sleep 300 & hp=$!
      check "a host process's environ is unreadable" 1 "cat /proc/$hp/environ"
      check "a host process's root is unreadable" 1 "ls /proc/$hp/root/"
      check "a host process cannot be signalled" 1 "kill -0 $hp"
      kill "$hp"; wait "$hp" 2>/dev/null
    fi
    check "tmux's socket under TMUX_TMPDIR unreachable" 1 "$(conn "$root/tmuxdir/tmux-$uid/default")"
    check "the socket named by TMUX unreachable" 1 "$(conn "$root/tmuxenv/default")"
    check "a host socket under /tmp outside TMPDIR unreachable" 1 "$(conn "$hostt/sock")"
    SSH_AUTH_SOCK=$root/tmp/agent.sock check "an agent socket right in TMPDIR unreachable" 1 "$(conn "$root/tmp/agent.sock")"
    if [ "$be" != seatbelt ]; then
      pf=$(mktemp "$root/pty.XXXXXX")
      python3 -I -c 'import os, pty, sys, time; m, s = pty.openpty(); print(os.ttyname(s), flush=True); time.sleep(600)' >"$pf" &
      srv="$srv $!"; n=0; while [ ! -s "$pf" ] && [ "$n" -lt 40 ]; do sleep 0.05; n=$((n + 1)); done
      if [ -s "$pf" ]; then check "the host's terminal $(cat "$pf") cannot be opened" 1 "exec 7<>$(cat "$pf")"
      else bad "no host terminal to try"; fi
      m=sandbox-test-$$-$RANDOM
      inside "echo x >/dev/shm/$m; echo x >/dev/mqueue/$m" >/dev/null
      [ ! -e "/dev/shm/$m" ] && [ ! -e "/dev/mqueue/$m" ] && ok "/dev/shm and /dev/mqueue writes stay private" ||
        { bad "a /dev/shm or /dev/mqueue write reached the host"; rm -f "/dev/shm/$m" "/dev/mqueue/$m"; }
      [ "$(inside 'readlink /proc/self/ns/ipc')" != "$(readlink /proc/self/ns/ipc)" ] && ok "an IPC namespace of its own" ||
        bad "the IPC namespace is the host's"
      check "/dev/null, /dev/urandom and a new pty work" 0 \
        'echo x >/dev/null && [ "$(head -c 8 /dev/urandom | wc -c)" = 8 ] && python3 -I -c "import pty; pty.openpty()"'
      touch /tmp/"$m"-host
      r=$(cd "$proj" && env -u TMPDIR HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c \
        "[ ! -e /tmp/$m-host ] && echo x >/tmp/$m-in && [ -s /tmp/$m-in ] && [ ! -e /var/tmp/$m-host ] && echo ok" 2>&1 </dev/null)
      [ "$r" = ok ] && [ ! -e /tmp/"$m"-in ] && ok "TMPDIR unset: a private, writable /tmp" || bad "TMPDIR unset: /tmp not private ($r)"
      rm -f /tmp/"$m"-host /tmp/"$m"-in
      mkdir -p /tmp/"$m"-home/.ssh && echo secret-k >/tmp/"$m"-home/.ssh/k
      r=$(cd "$proj" && env -u TMPDIR HOME=/tmp/"$m"-home HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c \
        'cat ~/.ssh/k 2>/dev/null; echo x >~/y && ls -A ~' 2>&1 </dev/null)
      [ "$r" = y ] && [ ! -e /tmp/"$m"-home/y ] && ok "TMPDIR unset, a home under /tmp: empty and writable, gone after" ||
        bad "TMPDIR unset, a home under /tmp: $r"
      rm -rf /tmp/"$m"-home
      # a TERM to the call's group while the sandbox is still being set up ends it too: at once, and (ns) once its
      # pid 1 runs plus 0 to 8 ms
      late=0
      for d in now 0 0.001 0.002 0.004 0.008; do
        (cd "$proj" && exec setsid "$exe" "$bash_abs" --noprofile --norc -c 'sleep 30' </dev/null >/dev/null 2>&1) & cp=$!
        if [ "$d" != now ] && [ "$be" = ns ]; then
          n=0; until pgrep -g "$cp" -f '^/bin/sh [^ ]*/ns-setup ' >/dev/null || [ "$n" -ge 400 ]; do n=$((n + 1)); done; sleep "$d"
        fi
        kill -TERM -- "-$cp" 2>/dev/null || kill -TERM "$cp" 2>/dev/null
        n=0; while kill -0 "$cp" 2>/dev/null && [ "$n" -lt 20 ]; do sleep 0.05; n=$((n + 1)); done
        kill -0 "$cp" 2>/dev/null && { late="$late $d"; kill -9 -- "-$cp" 2>/dev/null; }
        wait "$cp" 2>/dev/null
      done
      [ "$late" = 0 ] && ok "a SIGTERM during the setup ends the call within 1 s" || bad "a SIGTERM during the setup was lost (after${late#0} s)"
    fi
    mark=sandbox-bg-$$-$RANDOM
    inside "(exec -a $mark sleep 1000) >/dev/null 2>&1 &" >/dev/null
    n=0; while pgrep -f "$mark" >/dev/null && [ "$n" -lt 20 ]; do sleep 0.05; n=$((n + 1)); done
    if pgrep -f "$mark" >/dev/null; then
      ok "a background job outlives its call (the session sweep ends it)"; pkill -9 -f "$mark"
    else ok "a background job ends with its call ($((n * 50)) ms)"
    fi
  done
  proj=$H; r=$(inside 'echo ran'); s=$?
  [ "$s" = 126 ] && ok "refused in the home itself" || bad "ran in the home itself (exit $s: $r)"
  proj=/; r=$(inside 'echo ran'); s=$?
  [ "$s" = 126 ] && ok "refused in /" || bad "ran in / (exit $s: $r)"
  proj=$root; check "a project that holds the home: the home stays hidden" 1 'cat ~/.ssh/known_hosts'
  # An eval arm's layout: HOME under TMPDIR. The home stays hidden although TMPDIR is bound writable.
  mkdir -p "$root/tmp/arm/home/.ssh" "$root/tmp/arm/work"; echo secret-arm >"$root/tmp/arm/home/.ssh/k"
  proj=$root/tmp/arm/work
  HOME=$root/tmp/arm/home check "a home under TMPDIR stays hidden" 1 'cat ~/.ssh/k'
  # The temp dir must not be / or a system dir; a run through a link to port/exec finds its own files.
  proj=$root/projA
  r=$(cd "$proj" && TMPDIR=/ HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c 'echo ran' 2>&1 </dev/null); s=$?
  [ "$s" = 126 ] && ok "refused with TMPDIR=/" || bad "ran with TMPDIR=/ (exit $s: $r)"
  r=$(cd "$proj" && TMPDIR=/usr HENKI_SANDBOX=$be "$exe" "$bash_abs" --noprofile --norc -c 'echo ran' 2>&1 </dev/null); s=$?
  [ "$s" = 126 ] && ok "refused with TMPDIR=/usr" || bad "ran with TMPDIR=/usr (exit $s: $r)"
  mkdir -p "$root/cfg/port"; ln -s "$exe" "$root/cfg/port/exec"
  r=$(cd "$proj" && HENKI_SANDBOX=$be "$root/cfg/port/exec" "$bash_abs" --noprofile --norc -c 'echo ran' 2>&1 </dev/null); s=$?
  [ "$s" = 0 ] && [ "$r" = ran ] && ok "runs through a link to port/exec" || bad "a link to port/exec fails (exit $s: $r)"
  for p in $srv; do kill "$p" 2>/dev/null; done; wait 2>/dev/null; rm -rf "$hostt"
  unset HOME TMPDIR XDG_STATE_HOME XDG_RUNTIME_DIR SSH_AUTH_SOCK TMUX TMUX_TMPDIR
  eval "$saved"
  rm -rf "$root"
done
# The bwrap layout, from its argv (a stand-in bwrap that prints it; Linux, whether or not bwrap is here).
if [ "$(uname -s)" = Linux ]; then
  be=bwrap-argv root=$(mktemp -d "${TMPDIR:-/tmp}/sandbox-test.XXXXXX"); root=$(cd -P "$root" && pwd)
  mkdir -p "$root/fake" "$root/home" "$root/proj" "$root/tmp"
  printf '#!/bin/sh\nfor a in "$@"; do printf "%%s\\n" "$a"; done\n' >"$root/fake/bwrap"; chmod +x "$root/fake/bwrap"
  srv=; serve "$root/tmp/agent.sock"
  a=$(cd "$root/proj" && HOME=$root/home TMPDIR=$root/tmp SSH_AUTH_SOCK=$root/tmp/agent.sock HENKI_SANDBOX=bwrap \
    HENKI_USER_PATH=$root/fake PATH=/usr/bin:/bin "$exe" "$bash_abs" --noprofile --norc -c 'echo "a b"' x 'y z' | tr '\n' ' ')
  for p in $srv; do kill "$p" 2>/dev/null; done; wait 2>/dev/null
  case $a in "--ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-ipc "*) ok "starts with the read-only root, a /dev and IPC of its own" ;;
    *) bad "unexpected start: $a" ;; esac
  case $a in *" --tmpfs /tmp "*"--tmpfs $root/home "*"--bind $root/tmp $root/tmp "*"--bind $root/proj $root/proj "*"--ro-bind-try /dev/null $root/tmp/agent.sock "*) ok "a private /tmp, then the home hidden, the temp dir and project bound, the agent socket masked" ;;
    *) bad "unexpected layout: $a" ;; esac
  case $a in *" -- $bash_abs --noprofile --norc -c echo \"a b\" x y z ") ok "the command's arguments come last" ;; *) bad "the command is not last: $a" ;; esac
  rm -rf "$root"
fi

rm -rf "$out" # not by an EXIT trap: a subshell that a TERM ends before its exec runs that trap too
echo "sandbox tests: $fails failed"
[ "$fails" = 0 ]
SH
  RC=0
  # shellcheck disable=SC2086 # b is a list of backends
  OUT=$(bash "$D/sandbox-tests.sh" "$MODS_DIR/sandbox/port/exec" "$MODS_DIR/../manager/henki-mods" $b 2>&1 </dev/null) || RC=$?
  [ "$RC" = 0 ] || { FAILS=$((FAILS + 1)) && printf '%s\n' "$OUT" >&2; }
}

t_sessions
t_prompts
t_project
t_clip
t_checkpoints
t_checkpoints_missed
t_checkpoints_kept
t_notify
t_compact
t_sandbox
if [ "$FAILS" != 0 ]; then
  echo "mods-tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "mods-tests.sh: $CASES cases passed"
