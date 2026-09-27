#!/usr/bin/env bash
# Posts one Alertmanager webhook for a flapping CPU alert. Usage: ./alert.sh firing|resolved
set -euo pipefail
curl -s -o /dev/null -X POST -H 'Content-Type: application/json' localhost:8080/hook/alertmanager -d @- <<JSON
{"alerts": [{"status": "$1", "fingerprint": "cpu-worker-1",
  "labels": {"alertname": "HighCPU", "instance": "worker-1", "env": "prod", "severity": "critical"},
  "annotations": {"summary": "CPU above 90% on worker-1", "description": "CPU at 95% for 1m"}}]}
JSON
