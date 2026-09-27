#!/usr/bin/env bash
# Runs cargo in a throwaway container: neither cargo nor rustc is installed here.
# -u keeps target/ and Cargo.lock owned by the user, not root.
# CARGO_HOME lives in the mounted dir because a named volume is born root.
set -euo pipefail
cd "$(dirname "$0")/.."
# rust:1-bookworm ships without clippy and rustfmt: build a local image with both, once.
# `docker rmi keenwake-rust` picks up a newer toolchain on the next run.
image="${RUST_IMAGE:-keenwake-rust}"
if [ "$image" = keenwake-rust ] && ! docker image inspect keenwake-rust >/dev/null 2>&1; then
  printf 'FROM rust:1-bookworm\nRUN rustup component add clippy rustfmt\n' | docker build -q -t keenwake-rust - >&2
fi
# Forward KEENWAKE_* and TYPESAFE_API_KEY by name only: values never appear in the command line.
envs=()
while IFS='=' read -r k _; do envs+=(-e "$k"); done < <(env | grep -E '^(KEENWAKE_[A-Z_]+|TYPESAFE_API_KEY)=' || true)
exec docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/w/.cargo-cache \
  ${envs[@]+"${envs[@]}"} \
  --network host \
  -v "$PWD":/w -w /w \
  "$image" cargo "$@"
