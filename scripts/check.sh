#!/usr/bin/env bash
# Format check, lint and tests. Run on Linux (WSL Debian or the target):
#   bash scripts/check.sh
# From Windows:
#   wsl -d Debian --cd /mnt/c/repository/ltentp --exec bash -l scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# /mnt/c is slow for build output; keep it on the Linux filesystem.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/ltentp-target}"

cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
