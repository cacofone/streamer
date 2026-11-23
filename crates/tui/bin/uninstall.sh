#!/usr/bin/env bash
# Wrapper: see scripts/uninstall.sh for the options.
set -euo pipefail
exec bash "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/uninstall.sh" streamer-tui "$@"
