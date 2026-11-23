#!/usr/bin/env bash
# Wrapper: see scripts/test.sh for the options.
set -euo pipefail
exec bash "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/test.sh" cacofone-streamer-cli streamer "$@"
