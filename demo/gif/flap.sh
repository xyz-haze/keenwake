#!/usr/bin/env bash
# One flap: the CPU alert fires, then clears a second later. Usage: ./flap.sh <n> <cpu %>
set -euo pipefail
./fire.sh "flap $1" "$2"
sleep 1
./alert.sh resolved "$2"
