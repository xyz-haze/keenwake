# Invariants

Rules the code must never break. Each one has a test; the test name is given.

1. **Whitelist.** Only mapped fields (`status`, `identity`, `summary`, `details`, `env`,
   `severity`) reach a model. A value placed in any other field of the source payload never
   appears in the request sent to the backend.
   `tests/pipeline.rs::unmapped_secret_never_reaches_the_backend`

2. **Redaction.** `summary` and `details` are redacted before they are stored or sent to a
   model; `identity`, `env` and `severity` are stored and sent as received. Property tests check
   that the output never contains an email address (and that redacting twice equals redacting
   once), an IPv4 address, a secret after a known key (`api_key=`, `token:`, `password=`, ...)
   or a password in a URL (`scheme://user:pass@host`); IPv6 addresses and the other token kinds
   (AWS key ids, `Authorization` headers, Slack and Discord webhook URLs, JWTs) are covered by
   examples only. The mapping error quoted in an untriaged ping is redacted too.
   `tests/redact.rs::output_never_contains_an_email_and_is_idempotent`,
   `tests/redact.rs::output_never_contains_an_ipv4`,
   `tests/redact.rs::a_secret_after_a_known_key_never_survives`,
   `tests/redact.rs::a_url_password_never_survives`,
   `tests/redact.rs::scrubs_each_kind` (IPv6, bearer and prefixed tokens),
   `tests/redact.rs::scrubs_credentials_in_realistic_alert_text`,
   `tests/server.rs::bad_status_value_is_redacted_in_the_untriaged_text`,
   `tests/pipeline.rs::prepare_redacts_before_storing_and_building_state`

3. **Fail-open.** In gate mode with `on_error = "ping"`, a backend failure pings. In observe
   mode, no decision ever suppresses an alert or reaches a team-facing output, only the
   informational `verdict` webhook. End to end, a backend error, timeout
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
   recorded without a model decision.
   `tests/pipeline.rs::resolved_needs_no_model`,
   `tests/server.rs::repeat_notification_does_not_ping_twice_and_resolved_follows_ping`
   (the backend receives the two firings, never the resolved).
   `tests/history.rs::median_exists_exactly_when_episodes_do` checks that the history sentence
   never has a past episode without a duration to report.

6. **Replay.** Same config and same deterministic backend give the same decisions as the ones
   already stored. The shipped `replay` (the `keenwake replay` command), run on a gate history
   recorded by `serve`, reports no change, including when a notification failed delivery, when
   `on_error = "drop"` sent an untriaged decision nowhere, and when `--since` is shorter than the
   repeat window.
   `tests/server.rs::replay_of_recorded_gate_history_changes_nothing`,
   `tests/server.rs::replay_follows_a_failed_delivery`,
   `tests/server.rs::replay_of_dropped_untriaged_changes_nothing`,
   `tests/server.rs::replay_since_inside_the_repeat_window_knows_the_earlier_ping`,
   `tests/pipeline.rs::replay_is_deterministic` (property over the shared pipeline)

## A known gap these invariants don't cover

keenwake cannot tell whether an alert resolved on its own or because a human fixed it: both
arrive as the same `resolved` event. So the sentence sent to the model (`src/state.rs`) says how
often the alert fired and how long its episodes lasted, never how they ended. No alerting tool
keenwake reads reports the difference today.
