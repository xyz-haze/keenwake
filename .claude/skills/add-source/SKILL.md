---
name: add-source
description: Add an alerting tool to keenwake from a sample webhook payload. Use when someone wants keenwake to read a new tool's webhooks (Datadog, Sentry, Uptime Kuma, a custom script).
---

# Add a source to keenwake

1. Ask for a real payload from the tool (not one from memory, not one copied from vendor docs).
   Save it to `tests/fixtures/<tool>.json` after removing anything private.
2. Read `docs/add-a-source.md` and map the six fields (`status`, `identity`, `summary`, `details`,
   `env`, `severity`) with the five primitives only (`path`, `first_of`, `map`, `const`,
   `template`). No conditions, no calculations. If `status` cannot be expressed with `path` +
   `map`, stop and say the tool needs a small script in front of keenwake instead of a source.
3. Write `presets/<tool>.toml` (the `alerts` + `[fields]` shape only, no `[source.name]`
   wrapper — see `presets/grafana.toml`), register it in the `PRESETS` list in `src/config.rs`,
   and add a test to `tests/config.rs` asserting `status`, `identity`, `summary` and `env` on the
   fixture.
4. Run `./cargo.sh test --test config` and confirm the new test fails before the preset exists,
   then passes once it does. Paste both outputs.
5. Run `./cargo.sh run -- --config demo/keenwake.toml check-source --source <tool>
   tests/fixtures/<tool>.json` and show the output. (`check-source` never touches the network,
   so any valid config works here — the repo itself has no `keenwake.toml` at its root, hence
   naming one explicitly.)
