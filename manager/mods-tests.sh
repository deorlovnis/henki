#!/usr/bin/env bash
# The tests of the official modules in mods/ (the ones a module's own dir cannot hold: ext/2 refuses a stray file).
# Hermetic: each case runs a module's command or tool as henki would, with a scratch HENKI_STATE, HENKI_SESSION,
# HENKI_MODULE and HENKI_NEXT, and stand-ins for the programs it calls on PATH. Bash 3.2 compatible like the manager.
#
#   manager/mods-tests.sh               every case
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

# A std journal for session ID in scope dir S, its first user record holding TEXT (JSON-escaped already), its mtime T
# (touch -t form).
journal() { # S ID TEXT T
  local f="$HENKI_STATE/sessions/$1/$2.jsonl"
  printf '{"t":"session","v":1}\n' >"$f"
  [ -z "$3" ] || printf '{"msg":{"content":"%s","role":"user"},"t":"user","ts":"2026-11-08T10:00:00Z"}\n' "$3" >>"$f"
  touch -t "$4" "$f"
}

t_sessions() {
  case_ "sessions: list, resume, new, reload"
  local s="$MODS_DIR/sessions/cmd"
  export HENKI_MODULE="$MODS_DIR/sessions"
  journal scope1 20261108T080000Z-aaaa 'cwd: /w\nos: linux x\ndate: 2026-11-08\n\nfix the \"parser\" please\nand more' 202611080800
  journal scope1 20261108T090000Z-bbbb 'cwd: /w\nos: linux x\ndate: 2026-11-08\n\n'"$(printf 'x%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68 69 70)" 202611080900
  journal scope1 "$HENKI_SESSION" '' 202611081000
  journal scope2 20261108T110000Z-dddd 'cwd: /v\nos: linux x\ndate: 2026-11-08\n\nelsewhere' 202611081100
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
  # One session only: /resume has nothing to switch to.
  rm -f "$HENKI_STATE"/sessions/scope1/*-aaaa.jsonl "$HENKI_STATE"/sessions/scope1/*-bbbb.jsonl
  : >"$HENKI_NEXT"
  run "$s/resume"
  rc_is 1
  err_has "no session other than this one"
  # History bound to another store: no std file for this session.
  export HENKI_SESSION=elsewhere-1
  run "$s/sessions"
  rc_is 1
  err_has "history is bound to another store; use its commands"
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
  [ -z "$(ls "$HENKI_STATE/checkpoints/$HENKI_SESSION")" ] || fail "a null target was kept"
  HENKI_SESSION="" run env "$m/cmd/checkpoints"
  rc_is 1
  err_has "no session (HENKI_SESSION)"
  cd "$here" || exit 1
  unset GIT_CEILING_DIRECTORIES
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

t_sessions
t_prompts
t_clip
t_checkpoints
t_notify
if [ "$FAILS" != 0 ]; then
  echo "mods-tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "mods-tests.sh: $CASES cases passed"
