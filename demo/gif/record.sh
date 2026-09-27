#!/usr/bin/env bash
# Re-renders docs/img/demo.gif from keenwake.tape. Needs Docker and TYPESAFE_API_KEY
# (a handful of Jev calls, well under a cent). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/../.."
scripts/cargo.sh build --release
cp target/release/keenwake demo/gif/keenwake
trap 'rm -f demo/gif/keenwake' EXIT
# The vhs image has no curl or jq; add them once.
if ! docker image inspect keenwake-vhs >/dev/null 2>&1; then
  printf 'FROM ghcr.io/charmbracelet/vhs\nRUN apt-get update && apt-get install -y --no-install-recommends curl jq && rm -rf /var/lib/apt/lists/*\n' \
    | docker build -q -t keenwake-vhs - >&2
fi
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e TYPESAFE_API_KEY -v "$PWD":/vhs -w /vhs keenwake-vhs demo/gif/keenwake.tape
