#!/usr/bin/env bash
# End to end: run the demo, wait for chaos to finish, then require that every injected real
# incident was paged: a ping or untriaged decision that reached the sink. Exit code is the verdict.
#
# Runs entirely inside the "keenwake-demo" compose project (see the `name:` key in
# docker-compose.yml) so it never touches any other container on this machine. No host ports
# are published: everything talks over the compose network.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out
rm -f out/truth.jsonl out/sink.jsonl out/report.json out/compose.log

# Torn down on every exit path (success, failure, Ctrl-C), so no keenwake-demo container is ever
# left running. The Laya model volume is kept across runs (its first download is slow, see the
# sidecar Dockerfile); only the keenwake store is dropped, so each run starts from a clean history.
cleanup() {
  docker compose logs --no-color > out/compose.log 2>&1 || true
  docker compose down --remove-orphans || true
  docker volume rm -f keenwake-demo_keenwake-data >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker compose up -d --build

echo "waiting for chaos to finish (first run downloads the Laya checkpoint from Hugging Face, can take ~15 min)..."
for _ in $(seq 1 220); do
  docker compose logs chaos 2>/dev/null | grep -q "chaos done" && break
  sleep 10
done
docker compose logs chaos | grep -q "chaos done" || { echo "chaos never finished"; exit 1; }

# keenwake replies 200 before triaging (background processing): give the last decision and its
# webhook a moment to land before reading the report.
sleep 15

docker compose exec -T keenwake keenwake --config /etc/keenwake/keenwake.toml report --since 1h --json > out/report.json

# A real incident passes only if a page actually reached the sink's /ping: a `ping` or an
# `untriaged` (sent to the ping output, when in doubt), not just a decision in the report.
python3 - <<'EOF'
import json, os, sys
PAGES = {"ping", "untriaged"}
report = json.load(open("out/report.json"))
paged = []  # what reached /ping as a page: the alert summary, or the raw body of an unreadable one
if os.path.exists("out/sink.jsonl"):
    for line in open("out/sink.jsonl"):
        s = json.loads(line)
        k = s["body"].get("keenwake", {})
        if s["path"] == "/ping" and k.get("decision") in PAGES:
            paged.append(k.get("summary") or k.get("raw", ""))
missing = []
for line in open("out/truth.jsonl"):
    t = json.loads(line)
    summary = t["summary"].replace(" (stuck)", "")
    if t["page"] == 1 and not any(summary in p for p in paged):
        missing.append(summary)
noise_pinged = [r for r in report["rows"] if r["kind"] in PAGES and r["summary"].startswith("Disk")]
print(json.dumps({"decisions": report["by_kind"], "real_incidents_not_paged": missing,
                  "staging_noise_pinged": len(noise_pinged)}, indent=2))
sys.exit(1 if missing else 0)
EOF
