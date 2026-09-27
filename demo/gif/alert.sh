#!/usr/bin/env bash
# Posts one Alertmanager webhook for the api-1 health check. Usage: ./alert.sh firing|resolved
set -euo pipefail
curl -s -o /dev/null -X POST -H 'Content-Type: application/json' localhost:8080/hook/alertmanager -d @- <<JSON
{"alerts": [{"status": "$1", "fingerprint": "healthcheck-api-1",
  "labels": {"alertname": "HealthCheckFailing", "instance": "api-1", "env": "prod", "severity": "critical"},
  "annotations": {"summary": "Health check failing on api-1", "description": "GET /healthz timed out"}}]}
JSON
