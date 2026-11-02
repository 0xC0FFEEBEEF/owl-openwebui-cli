#!/bin/bash

# Point your editor debugger at this script to always run the freshest locally built
# owl binary (scripts/debug-codex.sh) instead of a released one.


set -euo pipefail

CODEX_RS_DIR=$(realpath "$(dirname "$0")/../codex-rs")
(cd "$CODEX_RS_DIR" && cargo run --quiet --bin owl -- "$@")