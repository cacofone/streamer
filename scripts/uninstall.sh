#!/usr/bin/env bash
# Remove a binary that was installed with install.sh.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: uninstall.sh <binary> [options]

Removes <prefix>/bin/<binary>, but only if it was installed by install.sh.

Options:
  --prefix DIR   the prefix used for the install (default: $PREFIX, or ~/.local)
  --dry-run      print what would happen and change nothing
  -h, --help     show this help
USAGE
}

die() { echo "error: $*" >&2; exit 2; }

case "${1:-}" in -h | --help) usage; exit 0 ;; esac
[ $# -ge 1 ] || { usage >&2; exit 2; }
binary=$1
shift

prefix=${PREFIX:-$HOME/.local}
dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) [ $# -ge 2 ] || die "--prefix needs a value"; prefix=$2; shift 2 ;;
    --dry-run) dry_run=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

dest=$prefix/bin/$binary
share=$prefix/share/cacofone-streamer
marker=$share/$binary.installed

if [ ! -f "$marker" ]; then
  echo "nothing to uninstall: $binary was not installed by these scripts under $prefix"
  exit 0
fi

if [ "$dry_run" -eq 1 ]; then
  echo "dry run, nothing will be changed"
  echo "would remove: $dest"
  echo "would remove: $marker"
  exit 0
fi

rm -f "$dest" "$marker"
rmdir "$share" 2>/dev/null || true
echo "removed $dest"
