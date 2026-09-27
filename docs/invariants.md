# Invariants

Rules the code must never break. Each one has a test; the test name is given.

1. **Whitelist.** Only mapped fields (`status`, `identity`, `summary`, `details`, `env`,
   `severity`) reach a model. A value placed in any other field of the source payload never
   appears in the request sent to the backend.
   `tests/pipeline.rs::unmapped_secret_never_reaches_the_backend`

2. **Redaction.** `summary` and `details` are redacted before they are stored or sent to a
   model; `identity`, `env` and `severity` are stored and sent as received. Property tests prove
   that the output never contains an email address (and that redacting twice equals redacting
   once) or an IPv4 address; IPv6 addresses and tokens are covered by examples only.
   `tests/redact.rs::output_never_contains_an_email_and_is_idempotent`,
   `tests/redact.rs::output_never_contains_an_ipv4`,
   `tests/redact.rs::scrubs_each_kind` (IPv6, bearer and prefixed tokens),
   `tests/pipeline.rs::prepare_redacts_before_storing_and_building_state`

3. **Fail-open.** In gate mode with `on_error = "ping"`, a backend failure pings. In observe
   mode, no decision ever suppresses an alert or reaches a team-facing output — only the
   `verdict` webhook, which is explicitly informational. End to end, a backend error, timeout
   or HTTP 429 in gate sends an `untriaged` ping.
   `tests/decide.rs::gate_fail_open_always_pings`,
   `tests/decide.rs::observe_never_suppresses_or_sends_to_team_outputs`,
   `tests/server.rs::gate_backend_down_fails_open`,
   `tests/server.rs::gate_backend_timeout_pings_untriaged`,
   `tests/server.rs::gate_backend_quota_429_pings_untriaged`

4. **Monotonicity.** A higher probability never yields a less urgent decision
   (`digest` < `escalate` < `ping`).
   `tests/decide.rs::higher_probability_is_never_less_urgent`

5. **History.** A `resolved` event never calls the backend: it closes its episode and is
   recorded without a model decision. This is the tested, meaningful part.
   `tests/pipeline.rs::resolved_needs_no_model`,
   `tests/server.rs::repeat_notification_does_not_ping_twice_and_resolved_follows_ping`
   (the backend receives the two firings, never the resolved).
   `tests/history.rs::resolved_never_exceeds_episodes` also exists, but it holds by
   construction: `resolved_7d` and `episodes_7d` are counted from the same list of ended
   episodes, so the property cannot fail as the code is written.

6. **Replay.** Same config and same deterministic backend give the same decisions as the ones
   already stored. The shipped `replay` (the `keenwake replay` command), run on a gate history
   recorded by `serve`, reports no change.
   `tests/server.rs::replay_of_recorded_gate_history_changes_nothing`,
   `tests/pipeline.rs::replay_is_deterministic` (property over the shared pipeline)

## A known gap these invariants don't cover

keenwake has no way to tell whether an alert resolved on its own or because a human intervened
— both look like the same `resolved` event. The "resolved on its own X% of the time" figure in
the sentence sent to the model (see `src/state.rs`) therefore counts every resolution, not just
the self-healing ones. This is a real limitation of the history signal, not a bug: fixing it
would need a way to distinguish the two, which no alerting tool keenwake talks to reports today.
