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

t_sessions
t_prompts
t_clip
if [ "$FAILS" != 0 ]; then
  echo "mods-tests.sh: $FAILS checks failed in $CASES cases" >&2
  exit 1
fi
echo "mods-tests.sh: $CASES cases passed"
