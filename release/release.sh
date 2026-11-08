#!/usr/bin/env bash
# Builds the release tarballs: one dist/henki-VERSION-TARGET.tar.gz per target, and dist/SHA256SUMS over every
# tarball of VERSION in dist/. A tarball holds one directory, henki-VERSION-TARGET/, with
#
#   bin/henki                    the static binary (musl on Linux)
#   bin/henki-mods               the module manager; it finds the official modules at ../share/henki/official
#   bin/henki-mods-dev           the module author's tools (new, test, try, publish, ...); manager/testdata/ stays out
#   share/henki/official/NAME/   every module in mods/, as git tracks it (off until `henki-mods add NAME`)
#   share/henki/conform/         the conformance suites of manager/conform/ (henki-mods test --port, swap), with the
#                                histories in fixtures/ that context.tsv sends
#   share/henki/eval/general/    the general eval suite of eval/general/ (henki-mods eval --suite general)
#   share/henki/index/           the default module index's anchor from manager/index/: its URL, signers, checkpoint
#                                and the refused keys (henki-mods search, info, add NAME, sync)
#   share/henki/mod-profiles/    the module profiles of manager/mod-profiles/ (henki-mods init)
#   share/henki/config.*.toml    the config profiles in profiles/ (henki's no-config message names config.mac.toml)
#   LICENSE README.md CONTRACTS.md SECURITY.md EXTENDING.md
#
# Usage:
#   release/release.sh [--index-not-live] VERSION [TARGET...]
#                                            VERSION must be Cargo.toml's (else --version and the file name disagree);
#                                            TARGET defaults to this host's: ARCH-unknown-linux-musl on Linux,
#                                            ARCH-apple-darwin on macOS. A target of the other OS is refused (no
#                                            cross linker is assumed). The index anchor must be live: a key that is
#                                            not a refused one, and a checkpoint; --index-not-live packs the anchor
#                                            with no key instead (henki-mods then says the index is not live yet).
#   release/release.sh --formula VERSION TAP [BASE_URL]
#                                            writes TAP/Formula/henki.rb from dist/SHA256SUMS: one url and sha256
#                                            per target in it. BASE_URL defaults to the GitHub release of vVERSION.
#   release/release.sh --selftest            builds this host's target into a scratch dist, unpacks it, runs
#                                            bin/henki --version and bin/henki-mods with a scratch HOME, packs it
#                                            again (the same bytes), writes a formula, and checks the refusals.
#
# A release is built from a commit: a working tree with changes is refused, and the binary's --version names the
# commit (HENKI_COMMIT). Packing is reproducible: names sorted, owner 0, every mtime the commit's time, gzip -n.
#
# Environment: DIST (default dist/), CARGO_TARGET_DIR, CARGO_BUILD_JOBS (default 2), RUSTUP_HOME, as for check.sh.
# Bash 3.2 (macOS) compatible; POSIX tools plus git and cargo; GNU tar or bsdtar.
set -euo pipefail
export LC_ALL=C
# macOS's tar packs extended attributes (com.apple.provenance, quarantine) as AppleDouble ._NAME members unless told
# not to; this stops it for every tar, and pack passes --no-mac-metadata --no-xattrs to bsdtar on a Mac too.
export COPYFILE_DISABLE=1
cd "$(dirname "$0")/.."
ROOT=$PWD
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
# Pinned, as in check.sh, so the binary packed is the one this run built.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${CARGO_BUILD_TARGET_DIR:-$ROOT/target}}"
REPO_URL=https://github.com/deorlovnis/henki

die() {
  printf 'release.sh: %s\n' "$*" >&2
  exit 1
}
say() { printf 'release.sh: %s\n' "$*" >&2; }

if command -v sha256sum >/dev/null 2>&1; then
  sha_of() { sha256sum "$1" | awk '{ print $1 }'; }
elif command -v shasum >/dev/null 2>&1; then
  sha_of() { shasum -a 256 "$1" | awk '{ print $1 }'; }
else
  die "neither sha256sum nor shasum is on PATH; fix: install one of them (coreutils or perl)"
fi

# The [package] version in Cargo.toml.
cargo_version() {
  awk '/^\[package\]/ { p = 1; next } /^\[/ { p = 0 } p && /^version *=/ { gsub(/[" ]/, ""); sub(/^version=/, ""); print; exit }' Cargo.toml
}

host_os() {
  case "$(uname -s)" in
    Linux) echo linux ;;
    Darwin) echo macos ;;
    *) die "$(uname -s) is not a host release.sh builds on; fix: run it on Linux or macOS" ;;
  esac
}

host_arch() {
  case "$(uname -m)" in
    aarch64 | arm64) echo aarch64 ;;
    x86_64 | amd64) echo x86_64 ;;
    *) die "$(uname -m) has no release target; fix: build on aarch64 or x86_64" ;;
  esac
}

host_target() {
  case "$(host_os)" in
    linux) echo "$(host_arch)-unknown-linux-musl" ;;
    macos) echo "$(host_arch)-apple-darwin" ;;
  esac
}

# Fails unless TARGET is one this host can build and link.
check_target() {
  local t=$1 os
  os=$(host_os)
  case "$t" in
    aarch64-unknown-linux-musl | x86_64-unknown-linux-musl)
      [ "$os" = linux ] || die "$t: a Linux target, and this host is $os; fix: build it on Linux" ;;
    aarch64-apple-darwin | x86_64-apple-darwin)
      [ "$os" = macos ] || die "$t: a macOS target, and this host is $os; fix: build it on macOS" ;;
    *) die "$t is not a release target; fix: name one of aarch64-apple-darwin, x86_64-apple-darwin, aarch64-unknown-linux-musl, x86_64-unknown-linux-musl" ;;
  esac
  [ -d "$(rustc --print sysroot)/lib/rustlib/$t/lib" ] ||
    die "the $t target is not installed; fix: rustup target add $t (or set RUSTUP_HOME to a rustup home that has it)"
}

# Writes the name of the commit HEAD is at, with -dirty when the working tree has changes (selftest only).
commit_label() {
  local c
  c=$(git rev-parse --short=12 HEAD) || die "not a git checkout; fix: build from a clone of $REPO_URL"
  if [ -n "$(git status --porcelain)" ]; then c="$c-dirty"; fi
  printf '%s\n' "$c"
}

# Builds TARGET with HENKI_COMMIT set and writes the binary's path. The old binary goes first, so a build that
# lands elsewhere fails instead of packing a stale file.
build() {
  local t=$1 commit=$2 bin="$CARGO_TARGET_DIR/$1/release/henki"
  rm -f "$bin"
  HENKI_COMMIT=$commit cargo build --locked --release --target "$t" >&2
  [ -f "$bin" ] || die "cargo built no $bin (is build.target set in a cargo config?)"
  # A static musl binary has no ELF interpreter; one with an interpreter would not run in a bare environment.
  case "$t" in *-linux-musl)
    # Read whole, not piped into grep -q: under pipefail a match that closes the pipe early would read as no match.
    if command -v readelf >/dev/null 2>&1; then
      case "$(readelf -l "$bin")" in *INTERP*)
        die "$bin is dynamically linked; fix: build with the $t target's static default (no -C target-feature=-crt-static)"
        ;;
      esac
    fi
    ;;
  esac
  printf '%s\n' "$bin"
}

# Copies the files git tracks under mods/NAME (the working tree's content, git's mode) into DEST/NAME, so an
# ignored or untracked file in a module never ships.
copy_mod() {
  local name=$1 dest=$2 meta path mode
  git ls-files -s -z -- "mods/$name" | while IFS= read -r -d '' meta; do
    path=${meta#*$'\t'}
    mode=${meta%% *}
    mkdir -p "$dest/$(dirname "${path#mods/}")"
    case "$mode" in
      120000) ln -s "$(readlink "$path")" "$dest/${path#mods/}" ;;
      100755) cp "$path" "$dest/${path#mods/}" && chmod 755 "$dest/${path#mods/}" || die "cannot copy $path; fix: make it readable and check the free space in TMPDIR" ;;
      100644) cp "$path" "$dest/${path#mods/}" && chmod 644 "$dest/${path#mods/}" || die "cannot copy $path; fix: make it readable and check the free space in TMPDIR" ;;
      *) die "$path: git mode $mode (a submodule?); fix: a module holds files and symlinks only" ;;
    esac
  done
}

# The hd1 of a module dir, as henki-mods verify --dir reports it.
hd1_of() {
  local out
  out=$(HOME="$SCRATCH/home" XDG_DATA_HOME="$SCRATCH/home/data" manager/henki-mods verify --dir "$1" 2>&1) ||
    die "$1 does not pass henki-mods verify --dir: $out"
  printf '%s\n' "$out" | sed -n 's/.*\(hd1:[0-9a-f]*\).*/\1/p'
}

# anchor_check DIR NOT_LIVE: the index anchors in DIR (NAME.url, NAME.signers, NAME.checkpoint, refused.keys), as a
# release ships them. Each signers line must name a key, and no line may hold a refused one (a development key in
# refused.keys) anywhere in it: a fixed-string match on the whole line, as henki-mods drops such lines. An anchor with
# a key needs its checkpoint, the full commit a first sync must find on the index's first-parent line. An anchor with
# no key line is the placeholder of an index not live yet: refused unless NOT_LIVE is 1.
anchor_check() {
  local d=$1 f n keys blob ck refused
  [ -f "$d/refused.keys" ] || die "$d/refused.keys is missing; fix: restore it from git (it lists the development keys)"
  refused=$(LC_ALL=C command grep -v -e '^#' -e '^[[:space:]]*$' "$d/refused.keys" | LC_ALL=C tr -d ' \t\r' || true)
  for f in "$d"/*.url; do
    [ -f "$f" ] || die "$d holds no index URL (NAME.url); fix: restore manager/index/ from git"
    n=$(basename "$f" .url)
    [ -f "$d/$n.signers" ] || die "$d/$n.signers is missing; fix: restore it from git"
    keys=$(command grep -v -e '^#' -e '^[[:space:]]*$' "$d/$n.signers" || true)
    if [ -z "$keys" ]; then
      [ "$2" = 1 ] || die "index $n: $d/$n.signers holds no key, so the index is not live; fix: add its signing key's line and $n.checkpoint, or release with --index-not-live"
      continue
    fi
    for blob in $(printf '%s\n' "$keys" | awk '{ b = "-"; for (i = 2; i < NF; i++) if ($i ~ /^(ssh-|sk-|ecdsa-)/) { b = $(i + 1); break } print b }'); do
      [ "$blob" != - ] || die "index $n: a line of $d/$n.signers names no key; fix: write it as allowed_signers does (PRINCIPAL namespaces=\"git\" TYPE KEY)"
    done
    [ -z "$refused" ] || ! printf '%s\n' "$keys" | LC_ALL=C command grep -qF -e "$refused" ||
      die "index $n: $d/$n.signers holds a refused key (in refused.keys: a development key); fix: put the index's own signing key there"
    ck=$(sed -n 1p "$d/$n.checkpoint" 2>/dev/null) || ck=""
    case "$ck" in *[!0-9a-f]* | "") ck="" ;; esac
    [ ${#ck} = 40 ] || die "index $n: $d/$n.checkpoint does not hold a full commit; fix: write the commit the index's main is at (40 hex digits) there"
  done
}

# Lays out STAGE/henki-VERSION-TARGET from BIN and the repo.
stage() {
  local dir="$1/henki-$2-$3" bin=$4 m n
  rm -rf "$dir"
  mkdir -p "$dir/bin" "$dir/share/henki/official" "$dir/share/henki/conform/fixtures" "$dir/share/henki/eval"
  cp "$bin" "$dir/bin/henki" && chmod 755 "$dir/bin/henki"
  cp manager/henki-mods "$dir/bin/henki-mods" && chmod 755 "$dir/bin/henki-mods"
  cp manager/henki-mods-dev "$dir/bin/henki-mods-dev" && chmod 755 "$dir/bin/henki-mods-dev"
  for n in LICENSE README.md CONTRACTS.md SECURITY.md EXTENDING.md; do cp "$n" "$dir/$n" && chmod 644 "$dir/$n"; done
  # The profiles, where henki's no-config message sends a Homebrew user: $(brew --prefix)/share/henki/.
  for n in $(git ls-files 'profiles/config.*.toml'); do cp "$n" "$dir/share/henki/${n#profiles/}" && chmod 644 "$dir/share/henki/${n#profiles/}"; done
  for n in $(git ls-files 'manager/conform/*.tsv' 'manager/conform/fixtures/*.json'); do
    cp "$n" "$dir/share/henki/${n#manager/}" && chmod 644 "$dir/share/henki/${n#manager/}"
  done
  # The general suite as git tracks it: its programs (setup, check, fake-bin/) 755, the rest 644.
  git ls-files -s -z -- eval/general | while IFS= read -r -d '' m; do
    n=${m#*$'\t'}
    mkdir -p "$dir/share/henki/$(dirname "$n")"
    cp "$n" "$dir/share/henki/$n" || die "cannot copy $n; fix: make it readable and check the free space in TMPDIR"
    case "${m%% *}" in 100755) chmod 755 "$dir/share/henki/$n" ;; 100644) chmod 644 "$dir/share/henki/$n" ;; *) die "$n: git mode ${m%% *}; fix: the suite holds files only" ;; esac
  done
  # The index anchor (checked by anchor_check before the build) and the module profiles, as data.
  mkdir -p "$dir/share/henki/index" "$dir/share/henki/mod-profiles"
  for n in $(git ls-files 'manager/index/*' 'manager/mod-profiles/*.lock'); do
    cp "$n" "$dir/share/henki/${n#manager/}" && chmod 644 "$dir/share/henki/${n#manager/}" || die "cannot copy $n; fix: check the free space in TMPDIR"
  done
  for m in mods/*/; do
    n=$(basename "$m")
    copy_mod "$n" "$dir/share/henki/official"
    # The shipped copy must hash as the repo's does, or henki-mods would pin something else than what was reviewed.
    [ "$(hd1_of "$dir/share/henki/official/$n")" = "$(hd1_of "mods/$n")" ] ||
      die "share/henki/official/$n does not hash as mods/$n; fix: commit the module (only tracked files ship)"
  done
  find "$dir" -type d -exec chmod 755 {} +
}

# Packs STAGE/NAME into OUT reproducibly: a sorted list without recursion, owner 0, every mtime EPOCH, gzip -n.
pack() {
  local stage=$1 name=$2 out=$3 epoch=$4 when owner
  # touch -t takes local time; in UTC it is the epoch. GNU date takes -d @N, BSD date -r N.
  when=$(TZ=UTC date -u -d "@$epoch" +%Y%m%d%H%M.%S 2>/dev/null || TZ=UTC date -u -r "$epoch" +%Y%m%d%H%M.%S)
  (cd "$stage" && find "$name" -exec env TZ=UTC touch -h -t "$when" {} +)
  if tar --version 2>/dev/null | grep -q 'GNU tar'; then
    owner="--owner=0 --group=0 --numeric-owner"
  else
    owner="--uid 0 --gid 0 --numeric-owner"
    # Apple's bsdtar: no AppleDouble members, no xattrs (the bytes would depend on where a file came from).
    [ "$(uname -s)" != Darwin ] || owner="$owner --no-mac-metadata --no-xattrs"
  fi
  # shellcheck disable=SC2086 # owner is several flags
  (cd "$stage" && find "$name" | sort | tar -c --format=ustar --no-recursion $owner -f - -T -) | gzip -n -9 >"$out.tmp"
  mv "$out.tmp" "$out"
}

# DIST/SHA256SUMS over every henki-VERSION-*.tar.gz in DIST, in the format sha256sum -c reads.
sums() {
  local dist=$1 version=$2 f
  (
    cd "$dist"
    for f in henki-"$version"-*.tar.gz; do
      [ -f "$f" ] || continue
      printf '%s  %s\n' "$(sha_of "$f")" "$f"
    done
  ) >"$dist/SHA256SUMS.tmp"
  mv "$dist/SHA256SUMS.tmp" "$dist/SHA256SUMS"
}

# release VERSION DIRTY_OK NOT_LIVE TARGET...: the whole build. DIRTY_OK=1 only from the selftest; NOT_LIVE=1 packs
# an index anchor with no key (--index-not-live, and the selftest).
release() {
  local version=$1 dirty_ok=$2 not_live=$3 t bin commit epoch want
  shift 3
  want=$(cargo_version)
  [ "$version" = "$want" ] || die "VERSION $version is not Cargo.toml's $want; fix: set version = \"$version\" in Cargo.toml and commit, or release $want"
  [ $# -gt 0 ] || set -- "$(host_target)"
  for t in "$@"; do check_target "$t"; done
  commit=$(commit_label)
  case "$commit" in *-dirty)
    [ "$dirty_ok" = 1 ] || die "the working tree has changes (git status); fix: commit or stash them, a release is built from a commit"
    ;;
  esac
  anchor_check manager/index "$not_live"
  epoch=$(git log -1 --format=%ct HEAD)
  mkdir -p "$DIST" "$SCRATCH/stage"
  for t in "$@"; do
    say "building $t"
    bin=$(build "$t" "$commit")
    stage "$SCRATCH/stage" "$version" "$t" "$bin"
    pack "$SCRATCH/stage" "henki-$version-$t" "$DIST/henki-$version-$t.tar.gz" "$epoch"
    say "wrote $DIST/henki-$version-$t.tar.gz ($(wc -c <"$DIST/henki-$version-$t.tar.gz" | tr -d ' ') bytes)"
  done
  sums "$DIST" "$version"
  say "wrote $DIST/SHA256SUMS"
  cat "$DIST/SHA256SUMS"
}

# The sha256 SHA256SUMS lists for TARGET, or nothing.
sum_for() { awk -v f="henki-$2-$3.tar.gz" '$2 == f { print $1 }' "$1"; }

# formula VERSION TAP [BASE_URL]: TAP/Formula/henki.rb, one url per target SHA256SUMS lists.
formula() {
  local version=${1:-} tap=${2:-} base=${3:-} sums_file="$DIST/SHA256SUMS" out block t s os arch n=0 macos="" linux=""
  [ -n "$version" ] && [ -n "$tap" ] || die "--formula needs VERSION and TAP; fix: release/release.sh --formula 0.1.0 ../homebrew-henki"
  [ -d "$tap" ] || die "$tap is not a directory; fix: name the tap repository (git init it first)"
  [ -f "$sums_file" ] || die "$sums_file is missing; fix: release/release.sh $version first"
  base=${base:-$REPO_URL/releases/download/v$version}
  for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
    s=$(sum_for "$sums_file" "$version" "$t")
    [ -n "$s" ] || continue
    case "$t" in aarch64-*) arch=on_arm ;; *) arch=on_intel ;; esac
    block="    $arch do
      url \"$base/henki-$version-$t.tar.gz\"
      sha256 \"$s\"
    end
"
    case "$t" in *-apple-darwin) macos="$macos$block" ;; *) linux="$linux$block" ;; esac
    n=$((n + 1))
  done
  [ "$n" -gt 0 ] || die "$sums_file lists no henki-$version-*.tar.gz; fix: release/release.sh $version first"
  mkdir -p "$tap/Formula"
  out="$tap/Formula/henki.rb"
  {
    cat <<EOF
# Written by release/release.sh --formula in the henki repository; edit that, not this file.
class Henki < Formula
  desc "Small terminal agent"
  homepage "$REPO_URL"
  version "$version"
  license "Apache-2.0"

EOF
    for os in macos linux; do
      if [ "$os" = macos ]; then block=$macos; else block=$linux; fi
      [ -n "$block" ] || continue
      printf '  on_%s do\n%s  end\n\n' "$os" "$block"
    done
    cat <<'EOF'
  def install
    bin.install "bin/henki", "bin/henki-mods", "bin/henki-mods-dev"
    # henki-mods finds the official modules at ../share/henki/official from its own (symlink-resolved) dir.
    share.install "share/henki"
    doc.install "README.md", "CONTRACTS.md", "SECURITY.md", "EXTENDING.md"
  end

  def caveats
    <<~TEXT
      henki reads ~/.config/henki/config.toml; start from a profile, then set model and base_url:
        mkdir -p ~/.config/henki && cp #{HOMEBREW_PREFIX}/share/henki/config.mac.toml ~/.config/henki/config.toml
      The official modules (checkpoints, clip, compact, halti, itse, notify, project, prompts, sandbox, sessions) ship off.
      henki-mods (also henki mods) manages modules:
        henki-mods list        what is added, on, and what each adds to the prompt
        henki-mods add halti   shows what it installs, then asks
    TEXT
  end

  test do
    assert_match "henki #{version} ", shell_output("#{bin}/henki --version")
    ENV["HOME"] = testpath
    assert_match "official, not added", shell_output("#{bin}/henki-mods list")
  end
end
EOF
  } >"$out.tmp"
  mv "$out.tmp" "$out"
  say "wrote $out ($n targets)"
}

# expect WHAT WANTED CMD...: CMD fails with WANTED in its stderr, or WANTED=pass and it succeeds.
expect() {
  local what=$1 wanted=$2 rc=0 err
  shift 2
  err=$("$@" 2>&1 >/dev/null) || rc=$?
  if [ "$wanted" = pass ] && [ "$rc" = 0 ]; then return 0; fi
  if [ "$wanted" != pass ] && [ "$rc" != 0 ]; then
    case "$err" in *"$wanted"*) return 0 ;; esac
  fi
  say "selftest: $what: expected ${wanted}, got exit $rc${err:+: $err}"
  return 1
}

selftest() {
  local t version target name x x_exec got want bad=0 commit
  t=$SCRATCH
  version=$(cargo_version)
  target=$(host_target)
  name="henki-$version-$target"
  commit=$(commit_label)

  # Refusals first: they cost no build.
  expect "a VERSION that is not Cargo.toml's" "is not Cargo.toml's" "$0" 9.9.9 || bad=1
  expect "an unknown target" "is not a release target" "$0" "$version" i386-pc-windows || bad=1
  expect "a target of the other OS" "fix: build it on" "$0" "$version" "$([ "$(host_os)" = linux ] && echo aarch64-apple-darwin || echo aarch64-unknown-linux-musl)" || bad=1
  expect "--formula without SHA256SUMS" "SHA256SUMS is missing" env DIST="$t/none" "$0" --formula "$version" "$t" || bad=1
  # In a clone with this script and one untracked file, so the refusal is tested whatever state this tree is in.
  git clone -q "$ROOT" "$t/clone" && mkdir -p "$t/clone/release" && cp "$0" "$t/clone/release/release.sh" && : >"$t/clone/untracked" ||
    die "cannot clone $ROOT into $t; fix: check TMPDIR"
  expect "a working tree with changes" "the working tree has changes" "$t/clone/release/release.sh" "$version" || bad=1
  # The index anchor: a release refuses one that is not live, a refused (development) key, a missing checkpoint.
  anchor_fixture() { # DIR SIGNERS_LINE CHECKPOINT
    mkdir -p "$1" && printf 'https://example.invalid/index\n' >"$1/x.url" && printf '# signers\n%s\n' "$2" >"$1/x.signers" &&
      cp manager/index/refused.keys "$1/refused.keys" && { [ -z "$3" ] || printf '%s\n' "$3" >"$1/x.checkpoint"; }
  }
  x=$(command grep -v '^#' manager/index/refused.keys | head -n 1)
  anchor_fixture "$t/a-none" "" ""
  anchor_fixture "$t/a-dev" "henki-index namespaces=\"git\" ssh-ed25519 $x dev" "$(printf '%040d' 0)"
  anchor_fixture "$t/a-nock" "henki-index namespaces=\"git\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOther live" ""
  anchor_fixture "$t/a-live" "henki-index namespaces=\"git\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOther live" "$(printf '%040d' 0)"
  expect "an anchor with no key" "so the index is not live" anchor_check "$t/a-none" 0 || bad=1
  expect "an anchor with no key, --index-not-live" pass anchor_check "$t/a-none" 1 || bad=1
  expect "an anchor with a refused key" "holds a refused key" anchor_check "$t/a-dev" 1 || bad=1
  # A quoted option with a space in it must not hide the refused key from the check.
  anchor_fixture "$t/a-dev2" "henki-index namespaces=\"git ssh-ed25519 X\" ssh-ed25519 $x dev" "$(printf '%040d' 0)"
  expect "an anchor with a refused key after a spaced option" "holds a refused key" anchor_check "$t/a-dev2" 1 || bad=1
  expect "an anchor without a checkpoint" "does not hold a full commit" anchor_check "$t/a-nock" 1 || bad=1
  expect "a live anchor" pass anchor_check "$t/a-live" 0 || bad=1
  rm "$t/a-live/refused.keys"
  expect "no refused keys" "refused.keys is missing" anchor_check "$t/a-live" 0 || bad=1
  # In a clean clone whose anchor has no key, a release stops before it builds anything.
  git clone -q "$ROOT" "$t/clean" && cp "$0" "$t/clean/release/release.sh" && printf '# no key\n' >"$t/clean/manager/index/deorlovnis.signers" &&
    git -C "$t/clean" -c user.name=selftest -c user.email=selftest@example.invalid -c commit.gpgsign=false commit -q -a --no-verify -m selftest ||
    die "cannot make a clean clone of $ROOT in $t; fix: check TMPDIR"
  expect "a release whose anchor has no key" "so the index is not live" "$t/clean/release/release.sh" "$version" || bad=1

  DIST="$t/dist"
  release "$version" 1 1 >/dev/null
  [ "$(cd "$DIST" && ls)" = SHA256SUMS$'\n'"$name.tar.gz" ] || { say "selftest: dist holds $(cd "$DIST" && ls | tr '\n' ' ')"; bad=1; }
  [ "$(awk '{ print $1 }' "$DIST/SHA256SUMS")" = "$(sha_of "$DIST/$name.tar.gz")" ] || { say "selftest: SHA256SUMS does not match"; bad=1; }

  # The listing: one top dir, the binaries, the docs, the profiles, every module in mods/ (as git tracks it), nothing
  # else.
  got=$(tar -tzf "$DIST/$name.tar.gz" | sed 's,/$,,' | sort)
  want=$(
    {
      for x in "" /bin /bin/henki /bin/henki-mods /share /share/henki /share/henki/official /share/henki/conform /share/henki/conform/fixtures /share/henki/eval /LICENSE /README.md /CONTRACTS.md; do
        echo "$name$x"
      done
      git ls-files 'profiles/config.*.toml' | sed "s,^profiles/,$name/share/henki/,"
      git ls-files 'manager/conform/*.tsv' 'manager/conform/fixtures/*.json' | sed "s,^manager/conform/,$name/share/henki/conform/,"
      git ls-files eval/general | sed "s,^,$name/share/henki/," | while read -r x; do
        while [ "$x" != "$name/share/henki/eval" ]; do
          echo "$x"
          x=$(dirname "$x")
        done
      done
      for x in /bin/henki-mods-dev /SECURITY.md /EXTENDING.md /share/henki/index /share/henki/mod-profiles; do echo "$name$x"; done
      git ls-files 'manager/index/*' 'manager/mod-profiles/*.lock' | sed "s,^manager/,$name/share/henki/,"
      git ls-files mods | sed "s,^mods/,$name/share/henki/official/," | while read -r x; do
        while [ "$x" != "$name/share/henki/official" ]; do
          echo "$x"
          x=$(dirname "$x")
        done
      done
    } | sort -u
  )
  [ "$got" = "$want" ] || { say "selftest: the tarball holds:"$'\n'"$(diff <(echo "$want") <(echo "$got"))"; bad=1; }

  mkdir -p "$t/x" "$t/home"
  tar -xzf "$DIST/$name.tar.gz" -C "$t/x"
  x="$t/x/$name"
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki" --version) || got="(exit $?)"
  want="henki $version ($commit, $(host_arch)-$(host_os))"
  [ "$got" = "$want" ] || { say "selftest: bin/henki --version said \"$got\", not \"$want\""; bad=1; }
  # Without a config henki names a profile under $(brew --prefix)/share/henki/: the tarball's share/henki/ has it.
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki" run hi 2>&1) && got="(exit 0) $got" || true
  want=$(printf '%s\n' "$got" | sed -n 's,.*\$(brew --prefix)/\(share/henki/[^ ]*\).*,\1,p')
  [ -n "$want" ] && [ -f "$x/$want" ] || { say "selftest: henki without a config said \"$got\", and the tarball has no ${want:-file it names}"; bad=1; }
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" --version 2>&1) || got="(exit $?)"
  [ "$got" = "henki-mods $version" ] || { say "selftest: bin/henki-mods --version said \"$got\""; bad=1; }
  # The official modules are found in the tarball's share/ (no HENKI_SHARE, no repo checkout next to it).
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" list 2>&1) || got="(exit $?) $got"
  case "$got" in *"official, not added: checkpoints clip compact halti itse notify project prompts sandbox sessions "*) ;; *)
    say "selftest: bin/henki-mods list said: $got"
    bad=1
    ;;
  esac
  # sessions, project, compact and sandbox need this henki's version (requires), and the links into .henki/ and cmd/
  # survive the copy.
  for n in itse sessions project compact sandbox; do
    got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" add "$n" --yes 2>&1) || { say "selftest: add $n: $got"; bad=1; }
    [ -L "$t/home/.config/henki/ext/$n" ] || { say "selftest: add $n --yes made no link"; bad=1; }
  done
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" verify 2>&1) || { say "selftest: verify: $got"; bad=1; }
  # The conformance suites are found in the tarball's share/ too, and so is the general eval suite, whole.
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" test --port history std 2>&1) || { say "selftest: test --port history std: $got"; bad=1; }
  # compact, the official context binding, passes context.tsv with the fixtures from share/ (where python3 runs it).
  if PATH=/usr/bin:/bin command -v python3 >/dev/null 2>&1; then
    got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" test --port context "$x/share/henki/official/compact/port/context" 2>&1) ||
      { say "selftest: test --port context compact: $got"; bad=1; }
  fi
  # sandbox, the official exec binding, passes exec/1's checks and binds from the tarball where one of its backends
  # runs.
  x_exec="$x/share/henki/official/sandbox/port/exec"
  if (cd "$t" && env -i HOME="$t/home" PATH=/usr/bin:/bin "$x_exec" /bin/sh -c true) >/dev/null 2>&1; then
    got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" test --port exec "$x_exec" 2>&1 &&
      env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" swap exec sandbox --yes 2>&1) ||
      { say "selftest: test --port exec and swap exec sandbox: $got"; bad=1; }
    [ -L "$t/home/.config/henki/port/exec" ] || { say "selftest: swap exec sandbox --yes made no link"; bad=1; }
  fi
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" eval --check "$x/share/henki/eval/general" 2>&1) || { say "selftest: eval --check: $got"; bad=1; }
  [ "$(cd "$x/share/henki/eval/general" && ls)" = "$(cd eval/general && ls)" ] || { say "selftest: share/henki/eval/general holds other tasks than eval/general"; bad=1; }
  # eval finds that suite: it gets as far as the screen (no terminal to ask on, so nothing runs). --unsandboxed, so
  # this holds where the sandbox has no backend.
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin HENKI_MODS_TTY="$t/no-tty" "$x/bin/henki-mods" eval sessions --suite general --unsandboxed 2>&1) && got="(exit 0) $got"
  case "$got" in *"suite general: 12 tasks"*"no terminal to ask on"*) ;; *)
    say "selftest: eval sessions --suite general said: $got"
    bad=1
    ;;
  esac
  # The module profiles, the index anchor and the author's tools are found beside the tarball's henki-mods.
  got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" init 2>&1) || got="(exit $?) $got"
  case "$got" in *"module profiles"*starter*) ;; *) say "selftest: henki-mods init said: $got" && bad=1 ;; esac
  if ! command grep -qv -e '^#' -e '^[[:space:]]*$' "$x/share/henki/index/deorlovnis.signers"; then
    got=$(env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" search 2>&1) && got="(exit 0) $got"
    case "$got" in *"is not live yet"*) ;; *) say "selftest: henki-mods search said: $got" && bad=1 ;; esac
  fi
  got=$(cd "$t" && env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" new probe --dir "$t/new" 2>&1 &&
    env -i HOME="$t/home" PATH=/usr/bin:/bin "$x/bin/henki-mods" verify --dir "$t/new/probe" 2>&1) || { say "selftest: henki-mods new and verify --dir: $got"; bad=1; }

  # Unpacked and packed again, it is the same bytes: the packing depends on nothing but the files and the commit.
  pack "$t/x" "$name" "$t/again.tar.gz" "$(git log -1 --format=%ct HEAD)"
  [ "$(sha_of "$t/again.tar.gz")" = "$(sha_of "$DIST/$name.tar.gz")" ] || { say "selftest: packing again gave other bytes"; bad=1; }

  mkdir -p "$t/tap"
  formula "$version" "$t/tap" 2>/dev/null
  x="$t/tap/Formula/henki.rb"
  command grep -q "url \"$REPO_URL/releases/download/v$version/$name.tar.gz\"" "$x" &&
    command grep -q "sha256 \"$(sha_of "$DIST/$name.tar.gz")\"" "$x" || { say "selftest: the formula lacks the url or sha256 of $name"; bad=1; }
  # No ruby here to parse it; the blocks must at least close.
  [ "$(command grep -cE '(^| )do$|^ *def |^class ' "$x")" = "$(command grep -cE '^ *end$' "$x")" ] || { say "selftest: the formula's do/def/end do not balance"; bad=1; }
  if command -v ruby >/dev/null 2>&1; then ruby -c "$x" >/dev/null || bad=1; fi

  if [ "$bad" != 0 ]; then
    say "selftest: failed (above)"
    return 1
  fi
  echo "release.sh selftest: $name builds, unpacks, runs, packs again to the same bytes, and gets a formula"
}

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/henki-release.XXXXXX")
trap 'chmod -R u+w "$SCRATCH" 2>/dev/null; rm -rf "$SCRATCH"' EXIT
DIST=${DIST:-$ROOT/dist}

case "${1:-}" in
  --selftest)
    [ $# = 1 ] || die "--selftest takes no arguments"
    selftest
    ;;
  --formula)
    shift
    formula "$@"
    ;;
  --index-not-live)
    [ $# -ge 2 ] || die "--index-not-live needs VERSION; fix: release/release.sh --index-not-live VERSION [TARGET...]"
    v=$2
    shift 2
    release "$v" 0 1 "$@"
    ;;
  "" | -*)
    echo "usage: release/release.sh [--index-not-live] VERSION [TARGET...] | --formula VERSION TAP [BASE_URL] | --selftest" >&2
    exit 2
    ;;
  *)
    v=$1
    shift
    release "$v" 0 0 "$@"
    ;;
esac
