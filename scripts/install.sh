#!/usr/bin/env bash
# Build one crate's binary and install it, without `cargo install`.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: install.sh <crate> <binary> [options]

Builds <crate> with cargo and copies <binary> to <prefix>/bin.

Options:
  --prefix DIR   install under DIR (default: $PREFIX, or ~/.local)
  --debug        install the debug build instead of the release build
  --dry-run      print what would happen and change nothing
  --force        overwrite a binary that was not installed by these scripts
  -h, --help     show this help
USAGE
}

die() { echo "error: $*" >&2; exit 2; }
fail() { echo "error: $*" >&2; exit 1; }

case "${1:-}" in -h | --help) usage; exit 0 ;; esac
[ $# -ge 2 ] || { usage >&2; exit 2; }
crate=$1
binary=$2
shift 2

prefix=${PREFIX:-$HOME/.local}
profile=release
dry_run=0
force=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    --debug) profile=debug; shift ;;
    --dry-run) dry_run=1; shift ;;
    --force) force=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
target_dir=${CARGO_TARGET_DIR:-$root/target}
src=$target_dir/$profile/$binary
dest=$prefix/bin/$binary
marker=$prefix/share/cacofone-streamer/$binary.installed

if [ -e "$dest" ] && [ ! -f "$marker" ] && [ "$force" -eq 0 ]; then
  fail "$dest exists and was not installed by these scripts (use --force to overwrite)"
fi

if [ "$dry_run" -eq 1 ]; then
  echo "dry run, nothing will be changed"
  echo "would build:   cargo build --locked -p $crate ($profile)"
  echo "would install: $src -> $dest"
  echo "would write:   $marker"
  exit 0
fi

cd "$root"
echo "building $crate ($profile)"
if [ "$profile" = release ]; then
  cargo build --locked --release -p "$crate"
else
  cargo build --locked -p "$crate"
fi
[ -f "$src" ] || fail "the build did not produce $src"

mkdir -p "$prefix/bin" "$prefix/share/cacofone-streamer"
install -m 755 "$src" "$dest"
commit=$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo unknown)
printf 'crate=%s\nbinary=%s\ncommit=%s\n' "$crate" "$binary" "$commit" > "$marker"
echo "installed $dest"

version=$("$dest" --version) || fail "the installed binary does not run"
echo "$version"

case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) echo "note: $prefix/bin is not on your PATH, add it with: export PATH=\"$prefix/bin:\$PATH\"" ;;
esac
