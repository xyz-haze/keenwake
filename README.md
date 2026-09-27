# alertsift

Decide which alerts deserve to wake a human, and prove it with numbers.

## How it works

1. Your alerting tool posts a webhook to `/hook/{source}`. `/hook` has no authentication in V1:
   keep alertsift on an internal network. `{source}` is either a built-in preset
   (`grafana`, `alertmanager`) or a `[source.name]` section in your `alertsift.toml`. An
   unrecognised source gets a `404` and is counted in `/metrics`.
2. The handler queues the body and replies `200` immediately — a slow backend never makes the
   source wait. One worker decides queued bodies one at a time, in arrival order, so a firing is
   fully decided before its `resolved` is looked at. If the queue (10 000 bodies) is full, the
   handler replies `503` so the source retries later. On SIGINT or SIGTERM, alertsift stops
   accepting webhooks and gives the worker up to 8 seconds to finish the queue.
3. The mapping extracts a fixed set of fields from the payload — `status`, `identity`, `summary`,
   `details`, `env`, `severity` — using five primitives (`path`, `first_of`, `map`, `const`,
   `template`). This is a whitelist: anything not mapped never leaves this step. See
   `docs/add-a-source.md`.
4. `redact` strips emails, IP addresses and tokens from `summary` and `details` before anything
   is stored or sent anywhere. `identity`, `env` and `severity` are stored and sent as received.
5. The event goes into SQLite. If the alert is `resolved`, its episode is closed and no model is
   called — ever.
6. Otherwise, alertsift computes facts about this identity's last 7 days from stored events: how
   many times it fired, what fraction resolved on its own, the median time to resolve, how long
   the current episode has run.
7. Those facts, in plain English, go ahead of the alert text in one sentence sent to the backend.
   Example, a Grafana alert with history behind it:

   > Environment: prod. The alert is still firing, for 3 minutes so far. It fired 22 times in the
   > last 7 days and resolved on its own 100% of the time, usually within about 3 minutes. This
   > time it looks like its usual pattern so far. Alert: CPU above 90% on etl-2. CPU at 92% for
   > 2m. Severity label: critical.

   alertsift cannot tell a self-resolution from a human fix — both look like the same `resolved`
   event — so that "resolved on its own" percentage counts every resolution.

8. The backend (Jev or Laya — same API either way) answers one question, `page_now`: a
   probability that a human should be paged right now.
9. `decide` turns that probability into `ping`, `escalate` or `digest` using two thresholds, or
   `untriaged` if the backend failed. A higher probability never yields a less urgent decision.
10. `output` sends the result according to the mode below. Every decision is recorded regardless
    of mode, so `report` and `replay` always have something to work with.

## Two modes

Observe is the default and the recommended way to start: zero risk, since your alerting tool
keeps sending alerts wherever it already sends them, and alertsift just gets a copy.

```
observe:  your alerting tool ─┬─→ your usual destination   (unchanged)
                               └─→ alertsift                (decides, records, sends nothing)

gate:     your alerting tool ──→ alertsift ──→ webhooks     (only what deserves a ping)
```

In `observe`, an optional `outputs.verdict` webhook receives every verdict ("probably noise,
0.12") if you want to watch it work live. In `gate`, the same decision is followed by an actual
send. The intended path is: run `observe` for a week or two, read `alertsift report`, then switch
to `gate`.

## Quick start

```
cd demo
./e2e.sh
```

This builds and runs the full demo stack — Prometheus, Alertmanager, three services that flake on
purpose, a chaos script that injects real incidents and background noise, alertsift running
against the local Laya backend, and a `sink` that logs every outgoing webhook — waits for the
chaos script to finish, then checks that every injected real incident got a `ping`. The exit code
is the verdict. First run downloads the Laya checkpoint from Hugging Face; expect about 10 minutes
end to end (measured: 592 s).

To watch it by hand instead:

```
cd demo
docker compose up
```

and follow the `sink` container's logs — every webhook alertsift sends out is printed there.

## Configuration

```toml
[backend]
url = "https://api.typesafe.ai"
model = "jev-1.13.0"
api_key_env = "TYPESAFE_API_KEY"
timeout_ms = 2000

[decision]
mode = "observe"        # or "gate"
on_error = "ping"       # in gate: "ping" or "drop"
ping = 0.55
digest = 0.30
digest_at = "08:00"
repeat_window_hours = 24  # in gate: how long a delivered ping suppresses repeats

[outputs]
ping = "https://example.org/ping"
escalate = "https://example.org/escalate"
digest = "https://example.org/digest"
verdict = "https://example.org/verdict"   # optional, useful in observe

[redact]
patterns = ["email", "ip", "token"]

[source.homelab]
alerts = ""
[source.homelab.fields]
status = { path = "/state", map = { KO = "firing", OK = "resolved" } }
identity = { path = "/check" }
summary = { template = "{check} failed: {line}" }
env = { const = "prod" }
```

In `gate` mode, `outputs.ping`, `outputs.escalate` and `outputs.digest` are all required —
alertsift refuses to start otherwise. `model` must pin a version; a value containing `"latest"`
is refused too.

## Backends

- **Jev** — hosted. `api_key_env` names the environment variable that holds the API key.
- **Laya** — a local sidecar (`sidecar/`), no data leaves the machine. Runs the
  `typed-decisions` checkpoint, on CPU or GPU.

Both speak the same `POST {url}/v1/systemone` API, so switching is a change of `url` and `model`,
nothing else. Thresholds are not portable between backends, though: in the corpus below, Laya's
scores sat in a narrower band (roughly 0.40-0.68) than Jev's, so a `ping`/`digest` pair tuned for
one backend is not guaranteed to make sense for the other. After changing backend, model or
thresholds, run `alertsift replay --since 7d` to see which past decisions would change before you
trust the new numbers.

## Numbers

### Corpus (synthetic, a ceiling — not a production result)

224 alerts, written by an LLM from 16 templates, run through the real sentence builder and the
real backend; the history facts (episode counts, resolved fraction, median duration) are the
corpus's own precomputed, synthetic fields, not recomputed from a simulated event history.
Resolved alerts are skipped, since alertsift never sends them to a model.

- **Laya** (`typed-decisions`, via the sidecar, on a local RTX 3080): **AUC 0.987 over 210 firing
  alerts** (`tests/corpus.rs`, run with `--ignored`).
- **Jev**: not measured yet.

For 5 of the 210 alerts, the resolved-percentage figure baked into the sentence differs slightly
from the source data, due to rounding. That percentage, here and in production, counts every
resolution alertsift sees — self-healed or human-fixed alike, since it has no way to tell them
apart.

### Demo (measured — `demo/e2e.sh`, Laya on CPU in Docker, gate mode, one run)

- Decisions: `ping` 5, `escalate` 6, `repeat` (suppressed) 10, `resolved` 6.
- Both injected real incidents were pinged.
- One staging-noise alert was pinged anyway. Noise suppression in this demo is not perfect: the
  model only had a few minutes of history to learn "this one usually clears itself" from.
- Total run time: about 10 minutes (592 s), including model load.
- Jev through alertsift: latency and cost not measured yet.

### Fuzzing

60 seconds, 8 532 runs against the webhook mapping and pipeline, no crash.

## Failure behaviour

The principle in `gate` mode: **when in doubt, ping**, unless `on_error = "drop"`. In `observe`
mode, a failure never changes what the team sees — it is only recorded and counted.

| Failure | In gate mode |
|---|---|
| Backend times out, errors, or hits a quota | If `on_error = "ping"` (default): pings, decision kind `untriaged`, reason `"not triaged: backend unavailable (...)"`. If `on_error = "drop"`: nothing is sent, but the decision is still recorded as `untriaged`. |
| Payload unreadable, or `status` missing or unrecognised | Always pings — regardless of `on_error` — with the redacted raw body (first 4000 characters) and text `"[untriaged] unreadable alert from {source}: {error}"`. The mapping error is also logged to stderr and counted in `alertsift_mapping_errors_total`. |
| An output webhook fails | 3 attempts with backoff, then a line in `undelivered.jsonl` plus the `alertsift_undelivered_total` metric. |
| SQLite fails, or processing an alert panics for any other reason | Store operations still panic rather than degrade to "decide without history". The panic is contained to that webhook body: in gate it pings the redacted raw body (first 4000 characters) as `untriaged` with text `"[untriaged] internal error while processing an alert from {source}"`; in observe it is only counted. Either way `alertsift_internal_errors_total` goes up, the error is logged to stderr without the body, and the next webhook is processed normally. A panic while sending the daily digest is logged and counted; the digest loop keeps running. |
| The webhook queue is full | The source gets `503` and should retry; counted in `alertsift_queue_full_total`. |
| alertsift itself is down | The source stops getting `200`s. Point a direct fallback contact (e.g. a Slack or PagerDuty webhook) at your alerting tool for this case — alertsift does not provide one. |

Repeated notifications of an already-pinged episode — including an untriaged ping while the
backend is down — do not ping again in gate mode for `decision.repeat_window_hours` (default 24)
after the last delivered ping; after that window a still-firing identity pings again, so a lost
`resolved` cannot silence it forever. A `resolved` event goes through when the episode had a
delivered ping within that window. `/metrics` exposes Prometheus counters (alerts received per source, decisions per kind,
mapping, backend and internal errors, backend latency, undelivered webhooks, full queue); `/healthz` is a plain
liveness check.

## Adding a source

See `docs/add-a-source.md`: save a real payload, write a handful of lines of TOML with the five
mapping primitives, check it with `check-source` before pointing your tool at alertsift. A Claude
Code skill that does this end to end lives in `.claude/skills/add-source/`.

## Invariants

The rules the code must never break — whitelist, redaction, fail-open, monotonicity, history,
replay — are in `docs/invariants.md`, each backed by a test.

## License

MIT OR Apache-2.0.
