#!/usr/bin/env bash
# One flap of the same CPU alert: fires, prints keenwake's decision, then clears.
# Usage: ./flap.sh <n>. Used by keenwake.tape.
set -euo pipefail
# Decisions on firing alerts only: the previous flap's `resolved` may land after we count.
fired() { keenwake --config keenwake.toml report --since 1h --json | jq '[.rows[] | select(.kind != "resolved")]'; }
rows() { fired | jq length; }
before=$(rows)
./alert.sh firing
for _ in $(seq 50); do [ "$(rows)" -gt "$before" ] && break; sleep 0.1; done
read -r kind p < <(fired | jq -r '.[-1] | "\(.kind) \(.probability)"')
case $kind in
  ping) verdict=$'\e[1;31mPING  \e[0m' ;;
  digest) verdict=$'\e[2mdigest\e[0m' ;;
  *) verdict=$kind ;;
esac
note=""; [ "$1" = 1 ] && note=$'  \e[2m(no history yet)\e[0m'
printf 'flap %s  CPU above 90%% on worker-1  ->  %s  p=%.2f%s\n' "$1" "$verdict" "$p" "$note"
sleep 1
./alert.sh resolved
