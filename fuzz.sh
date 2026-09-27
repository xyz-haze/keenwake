#!/usr/bin/env bash
# Fuzzes the webhook entry point for 10 minutes. Needs nightly, so it runs in its own image
# and is not part of CI. Usage: ./fuzz.sh [seconds]
set -euo pipefail
cd "$(dirname "$0")"
exec docker run --rm -u "$(id -u):$(id -g)" -e CARGO_HOME=/w/.cargo-cache -v "$PWD":/w -w /w \
  rustlang/rust:nightly bash -c "cargo install cargo-fuzz --locked --root /w/.cargo-cache && \
  /w/.cargo-cache/bin/cargo-fuzz run hook -- -max_total_time=${1:-600}"
