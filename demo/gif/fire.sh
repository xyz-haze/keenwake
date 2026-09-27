#!/usr/bin/env bash
# Sends the CPU alert as firing and prints keenwake's decision. Usage: ./fire.sh <label> <cpu %>
set -euo pipefail
# Decisions on firing alerts only: a `resolved` may land after we count.
fired() { keenwake --config keenwake.toml report --since 1h --json | jq '[.rows[] | select(.kind != "resolved")]'; }
before=$(fired | jq length)
./alert.sh firing "$2"
for _ in $(seq 50); do [ "$(fired | jq length)" -gt "$before" ] && break; sleep 0.1; done
read -r kind p < <(fired | jq -r '.[-1] | "\(.kind) \(.probability)"')
case $kind in
  ping) verdict=$'\e[1;31mPING  \e[0m' ;;
  digest) verdict=$'\e[2mdigest\e[0m' ;;
  *) verdict=$kind ;;
esac
printf '%-22s CPU at %s%%  ->  %s  p=%.2f\n' "$1" "$2" "$verdict" "$p"
