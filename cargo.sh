#!/usr/bin/env bash
# Runs cargo in a throwaway container: neither cargo nor rustc is installed here.
# -u keeps target/ and Cargo.lock owned by the user, not root.
# CARGO_HOME lives in the mounted dir because a named volume is born root.
set -euo pipefail
cd "$(dirname "$0")"
# Forward ALERTSIFT_* and TYPESAFE_API_KEY by name only: values never appear in the command line.
envs=()
while IFS='=' read -r k _; do envs+=(-e "$k"); done < <(env | grep -E '^(ALERTSIFT_[A-Z_]+|TYPESAFE_API_KEY)=' || true)
exec docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/w/.cargo-cache \
  ${envs[@]+"${envs[@]}"} \
  --network host \
  -v "$PWD":/w -w /w \
  "${RUST_IMAGE:-rust:1-bookworm}" cargo "$@"
