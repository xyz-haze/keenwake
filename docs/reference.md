# keenwake reference

What happens to an alert, every configuration key, and what keenwake does when something fails.
The rules the code must never break are in [invariants.md](invariants.md), each backed by a test.

## What happens to an alert

1. Your alerting tool posts a webhook to `/hook/{source}`. `{source}` is either a built-in preset
   (`grafana`, `alertmanager`) or a `[source.name]` section in your `keenwake.toml`. An
   unrecognised source gets a `404` and is counted in `/metrics`. `/hook` has no authentication:
   keep keenwake on an internal network.
2. The handler queues the body and replies `200` immediately, so a slow backend never makes the
   source wait. One worker decides queued bodies one at a time, in arrival order, so a firing is
   fully decided before its `resolved` is looked at. The queue holds at most 10 000 bodies and
   64 MiB; past either limit the handler replies `503` so the source retries later.
3. The mapping extracts a fixed set of fields from each alert in the body: `status`, `identity`,
   `summary`, `details`, `env`, `severity`, using five primitives (`path`, `first_of`, `map`,
   `const`, `template`). This is a whitelist: anything not mapped never leaves this step. See
   [add-a-source.md](add-a-source.md). A malformed alert is handled on its own as unreadable; the
   other alerts of the same body go on normally. A body with more than
   `server.max_alerts_per_body` alerts (default 500) is not sent to the backend at all.
4. `redact` strips emails, IP addresses and secrets from `summary` and `details` before anything
   is stored or sent anywhere. See [Redaction](#redaction) for what it covers.
5. The event goes into SQLite. If the alert is `resolved`, its episode is closed and no model is
   called.
6. Otherwise keenwake computes facts about this identity's last 7 days from stored events: how
   many episodes ended, their median duration, and how long the current one has run.
7. Those facts, in plain English, go ahead of the alert text in one sentence sent to the backend:

   > Environment: prod. The alert is still firing, for 3 minutes so far. It fired 22 times in the
   > last 7 days, and each time it ended, usually within about 3 minutes. This time it looks like
   > its usual pattern so far. Alert: CPU above 90% on etl-2. CPU at 92% for 2m. Severity label:
   > critical.

   keenwake cannot tell a self-resolution from a human fix: both arrive as the same `resolved`
   event. The sentence says an episode ended, not how.
8. The backend answers one question, `page_now`: a probability that a human should be paged now.
9. `decide` turns it into `ping` (at or above `decision.ping`), `digest` (below
   `decision.digest`) or `escalate` (in between), or `untriaged` if the backend failed. A higher
   probability never yields a less urgent decision.
10. `output` acts according to the mode. Every decision is recorded in both modes, so `report`
    and `replay` always have something to work with.

## Modes

- `observe` (default): keenwake gets a copy of your alerts and sends nothing, except an optional
  `outputs.verdict` webhook that receives every verdict if you want to watch it live.
- `gate`: keenwake is the only receiver and sends what it decides. `outputs.ping`,
  `outputs.escalate` and `outputs.digest` are required; keenwake refuses to start without them.

## Repeats (gate)

Alerting tools resend a firing alert on a schedule (Alertmanager's `repeat_interval`). Urgency is
ordered `ping` = `untriaged` > `escalate` > `digest`. Within `decision.repeat_window_hours` (default
24) of the last delivered notification of the same episode, a decision of equal or lower urgency is
recorded as `repeat` and sent nowhere; a strictly more urgent one goes through. After the window a
still-firing alert notifies again, so a lost `resolved` cannot silence it forever. An episode goes
into the digest at most once. A `resolved` is sent, to the same webhook, when the episode had a
delivered `ping`, `untriaged` or `escalate` within the window.

## Configuration

```toml
[backend]
url = "https://api.typesafe.ai"
model = "jev-1.13.0"          # must pin a version: a value containing "latest" is refused
api_key_env = "TYPESAFE_API_KEY"
timeout_ms = 2000

[decision]
mode = "observe"              # or "gate"
on_error = "ping"             # in gate: "ping" or "drop"
ping = 0.55
digest = 0.30
digest_at = "08:00"           # UTC
repeat_window_hours = 24

[outputs]
ping = "https://example.org/ping"
escalate = "https://example.org/escalate"
digest = "https://example.org/digest"
verdict = "https://example.org/verdict"   # optional, useful in observe

[redact]
patterns = ["email", "ip", "token"]

[server]
listen = "0.0.0.0:8080"
max_alerts_per_body = 500

[store]
path = "keenwake.db"
undelivered = "undelivered.jsonl"

[source.homelab]
alerts = ""
[source.homelab.fields]
status = { path = "/state", map = { KO = "firing", OK = "resolved" } }
identity = { path = "/check" }
summary = { template = "{check} failed: {line}" }
env = { const = "prod" }
```

## Backends

- **Jev**: hosted by TypeSafe. `api_key_env` names the environment variable holding the key.
- **Laya**: a local sidecar ([`sidecar/`](../sidecar)), no data leaves the machine. Runs the
  `typed-decisions` checkpoint, pinned to one revision, on CPU or GPU.

Both speak `POST {url}/v1/systemone`, so switching is a change of `url` and `model`. Thresholds are
not portable between them: Laya's scores sit in a narrower band than Jev's. After changing
backend, model or thresholds, run `keenwake replay --since 7d` to see which past decisions would
change before trusting the new numbers.

## What leaves the machine

Sent to the backend: the sentence above, built from `env`, `severity`, the redacted `summary` and
`details`, and the history facts. Sent to your output webhooks: the decision, the redacted text,
`identity` and the facts. `identity`, `env` and `severity` are never redacted. With Laya, nothing
leaves the machine except your own output webhooks.

## Redaction

Best effort, on `summary`, `details`, raw excerpts in untriaged pings and mapping error messages:

- `email`: email addresses.
- `ip`: IPv4 and IPv6 addresses.
- `token`: bearer tokens and JWTs, AWS access key ids and `aws_secret_access_key=`, the password in
  `scheme://user:password@host`, `key=value` and `"key": "value"` secrets (`api_key`, `apikey`,
  `token`, `secret`, `password`, `passwd`, `pwd`, `client_secret`, `access_token`),
  `Authorization: Basic|Bearer|Token` headers, Slack and Discord webhook URLs.

It over-redacts rather than under-redacts: `token: expired` becomes `token: [redacted:token]`.
Anything it does not recognise goes through, so do not put secrets you cannot afford to send in
alert text.

## Failure behaviour

In `gate`, when in doubt keenwake pings, unless `on_error = "drop"`. In `observe`, a failure never
changes what the team sees: it is recorded and counted.

| Failure | In gate mode |
|---|---|
| Backend times out, errors, or hits a quota | `on_error = "ping"` (default): pings as `untriaged`, reason `"not triaged: backend unavailable (...)"`. `on_error = "drop"`: nothing is sent, the decision is still recorded as `untriaged`. |
| An alert is unreadable, or its `status` is missing or unknown | Always pings that alert as `untriaged` with its redacted raw JSON (first 4000 characters), whatever `on_error` says. Logged to stderr and counted in `keenwake_mapping_errors_total`. |
| More than `max_alerts_per_body` alerts in one body | No backend call. One `untriaged` ping with a redacted excerpt; counted in `keenwake_oversized_bodies_total`. |
| An output webhook fails | 3 attempts with backoff, then a line in `undelivered.jsonl` (created owner-only) and `keenwake_undelivered_total`. |
| SQLite fails, or processing panics | Contained to that body: gate pings its redacted raw excerpt as `untriaged`, observe only counts it. `keenwake_internal_errors_total` goes up and the next webhook is processed normally. |
| The queue is full (10 000 bodies or 64 MiB) | The source gets `503` and should retry; counted in `keenwake_queue_full_total`. |
| keenwake itself is down | The source stops getting `200`s. Point a direct fallback (a Slack or PagerDuty webhook) at your alerting tool for this case: keenwake does not provide one. |

`/metrics` exposes Prometheus counters for all of the above plus alerts per source, decisions per
kind and backend latency. `/healthz` is a plain liveness check.

## Known limitations

- On SIGINT or SIGTERM keenwake stops accepting webhooks and gives the worker 8 seconds to finish
  the queue. Bodies still queued after that are lost, although their source got a `200`.
- A source that never sends `resolved` (Grafana with resolved notifications off) makes each
  identity one endless episode: the history facts say nothing useful, and gate pings it again once
  per repeat window.
- History starts empty. For the first days most alerts "never fired before", which pushes the
  model toward paging. That is why `observe` comes first.
