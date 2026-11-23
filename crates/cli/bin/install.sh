#!/usr/bin/env bash
# Wrapper: see scripts/install.sh for the options.
set -euo pipefail
exec bash "$(dirname "${BASH_SOURCE[0]}")/../../../scripts/install.sh" cacofone-streamer-cli streamer "$@"
