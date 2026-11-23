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
  --live         also run a real search on your network and ask you to confirm
                 the result (only for the streamer binary, never run in CI)
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
live=0
prefix=
while [ $# -gt 0 ]; do
  case "$1" in
    --installed) installed=1; shift ;;
    --live) live=1; shift ;;
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

if [ "$live" -eq 1 ] && [ "$binary" != "streamer" ]; then
  die "--live is only available for the streamer binary"
fi

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

run_step "cargo test" cargo test -p "$crate" --locked --no-fail-fast
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

  if [ "$binary" = "streamer" ]; then
    smoke_discover_help() { "$binpath" discover --help > /dev/null; }
    # Must work with no devices and no multicast: it just finds nothing.
    # stdout must hold nothing but JSON lines.
    smoke_discover_json() {
      local out
      out=$("$binpath" discover --timeout 2 --json 2> /dev/null) || return 1
      [ -n "$out" ] || return 0
      if command -v jq > /dev/null 2>&1; then
        printf '%s\n' "$out" | jq . > /dev/null
      elif command -v python3 > /dev/null 2>&1; then
        printf '%s\n' "$out" | python3 -c 'import sys, json; [json.loads(l) for l in sys.stdin if l.strip()]'
      else
        echo "notice: neither jq nor python3 found, skipping JSON validation"
      fi
    }
    run_step "smoke: discover --help" smoke_discover_help
    run_step "smoke: discover --timeout 2 --json" smoke_discover_json

    if [ "$live" -eq 1 ]; then
      live_check() {
        "$binpath" discover --timeout 5 || return 1
        printf '\nAre the devices you expected listed above? [y/N] '
        local answer=n
        read -r answer || answer=n
        case $answer in [yY]*) return 0 ;; *) return 1 ;; esac
      }
      run_step "live: discover on the real network" live_check
    fi
  fi
fi

echo
echo "summary: $passes passed, $fails failed"
[ "$fails" -eq 0 ]
