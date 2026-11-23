#!/usr/bin/env bash
# Test one crate, and smoke-test its binary.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: test.sh <crate> <binary|-> [options]

Runs the crate's tests, formatting and lint checks. For a crate with a
binary it also checks that the binary runs. Use "-" for a library crate.

Options:
  --installed    smoke-test the installed binary instead of a fresh build
  --prefix DIR   where to look for the installed binary (default: search PATH)
  -h, --help     show this help
USAGE
}

die() { echo "error: $*" >&2; exit 2; }

case "${1:-}" in -h | --help) usage; exit 0 ;; esac
[ $# -ge 2 ] || { usage >&2; exit 2; }
crate=$1
binary=$2
shift 2

installed=0
prefix=
while [ $# -gt 0 ]; do
  case "$1" in
    --installed) installed=1; shift ;;
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
target_dir=${CARGO_TARGET_DIR:-$root/target}

fails=0
passes=0
run_step() {
  local name=$1
  shift
  echo
  echo "== $name"
  if "$@"; then
    echo "PASS: $name"
    passes=$((passes + 1))
  else
    echo "FAIL: $name"
    fails=$((fails + 1))
  fi
}

run_step "cargo test" cargo test -p "$crate" --locked
run_step "rustfmt" cargo fmt -p "$crate" -- --check
run_step "clippy" cargo clippy -p "$crate" --all-targets --locked -- -D warnings

if [ "$binary" != "-" ]; then
  expected=$(cargo pkgid -p "$crate" | sed 's/.*[#@]//')

  if [ "$installed" -eq 1 ]; then
    if [ -n "$prefix" ]; then
      binpath=$prefix/bin/$binary
    else
      binpath=$(command -v "$binary" || true)
    fi
    if [ -z "$binpath" ] || [ ! -x "$binpath" ]; then
      echo "error: installed binary '$binary' not found (install it first, or pass --prefix)" >&2
      exit 1
    fi
  else
    run_step "build" cargo build -p "$crate" --locked
    binpath=$target_dir/debug/$binary
  fi
  echo
  echo "testing binary: $binpath (expected version $expected)"

  smoke_version() {
    local out
    out=$("$binpath" --version) || return 1
    case $out in
      *"$expected"*) echo "$out" ;;
      *) echo "unexpected --version output: $out"; return 1 ;;
    esac
  }
  smoke_help() { "$binpath" --help > /dev/null; }

  run_step "smoke: --version" smoke_version
  run_step "smoke: --help" smoke_help
fi

echo
echo "summary: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
