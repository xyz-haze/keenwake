#!/usr/bin/env bash
# One blip: the health check fails, then passes a second later. Usage: ./flap.sh <n>
set -euo pipefail
./fire.sh "blip $1: api-1 down"
sleep 1
./alert.sh resolved
