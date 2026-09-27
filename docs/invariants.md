# Invariants

Rules the code must never break. Each one has a test; the test name is given.

1. **Whitelist.** Only mapped fields (`status`, `identity`, `summary`, `details`, `env`,
   `severity`) reach a model. A value placed in any other field of the source payload never
   appears in the request sent to the backend.
   `tests/pipeline.rs::unmapped_secret_never_reaches_the_backend`

2. **Redaction.** Text sent to a model contains no email address, IP address or token, and
   redacting text twice gives the same result as redacting it once.
   `tests/redact.rs::output_never_contains_an_email_and_is_idempotent`,
   `tests/redact.rs::output_never_contains_an_ipv4`

3. **Fail-open.** In gate mode with `on_error = "ping"`, a backend failure pings. In observe
   mode, no decision ever suppresses an alert or reaches a team-facing output — only the
   `verdict` webhook, which is explicitly informational.
   `tests/decide.rs::gate_fail_open_always_pings`,
   `tests/decide.rs::observe_never_suppresses_or_sends_to_team_outputs`

4. **Monotonicity.** A higher probability never yields a less urgent decision
   (`digest` < `escalate` < `ping`).
   `tests/decide.rs::higher_probability_is_never_less_urgent`

5. **History.** For any sequence of events, resolved episodes never exceed the number of
   episodes, and a `resolved` event never calls the backend.
   `tests/history.rs::resolved_never_exceeds_episodes`,
   `tests/pipeline.rs::resolved_needs_no_model`

6. **Replay.** Same config and same deterministic backend give the same decisions as the ones
   already stored.
   `tests/pipeline.rs::replay_is_deterministic`

## A known gap these invariants don't cover

alertsift has no way to tell whether an alert resolved on its own or because a human intervened
— both look like the same `resolved` event. The "resolved on its own X% of the time" figure in
the sentence sent to the model (see `src/state.rs`) therefore counts every resolution, not just
the self-healing ones. This is a real limitation of the history signal, not a bug: fixing it
would need a way to distinguish the two, which no alerting tool alertsift talks to reports today.
