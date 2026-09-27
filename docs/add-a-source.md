# Add a source

A source is one alerting tool, not one alert. All the alerts a tool sends share its webhook
format, even if that tool has hundreds of different alert rules behind it.

## A source only you need

1. Save a real payload from your tool, e.g. `payload.json`. Don't write one from memory or copy
   field names out of a vendor's docs — what a tool actually sends can differ from what it
   documents. Capture one.

2. Add a section to your `alertsift.toml`. Example: a small shell health-check script that posts
   `{"check": "backup", "state": "KO"|"OK", "line": "..."}` for each check it runs.

   ```toml
   [source.healthcheck]
   alerts = ""                     # "" = one alert per webhook, or a JSON pointer to an array
   [source.healthcheck.fields]
   status = { path = "/state", map = { KO = "firing", OK = "resolved" } }
   identity = { path = "/check" }
   summary = { template = "{check} failed: {line}" }
   env = { const = "prod" }
   ```

3. Check it without sending anything, and without calling a model:

   ```
   alertsift check-source --source healthcheck payload.json
   ```

   This prints the `Alert` alertsift extracted from your payload and the exact sentence it would
   send to the backend, so you can see the mapping worked before wiring anything up for real.

4. Point your tool at `http://alertsift:8080/hook/healthcheck`.

## The five primitives

- `path` — read one field, by JSON Pointer: `{ path = "/annotations/summary" }`.
- `first_of` — the first pointer that is present: `{ first_of = ["/title", "/message"] }`.
- `map` — translate values: `{ path = "/state", map = { KO = "firing", OK = "resolved" } }`.
- `const` — a fixed value: `{ const = "prod" }`.
- `template` — assemble text from other fields: `{ template = "{check} failed: {line}" }`
  (a nested field is `{labels/environment}` or `{/labels/environment}`).

There are no conditions and no calculations, on purpose. A format that needs logic goes through a
small script placed in front of alertsift, not a language embedded in the TOML. If a tool's
`status` can't be expressed as `path` + `map`, that's the sign it needs a script in front rather
than a source section.

Six fields exist after mapping: `status`, `identity`, `summary`, `details`, `env`, `severity`.
Anything else in the payload is never read again — see `docs/invariants.md`, invariant 1.
`identity` and `env` are optional: a missing `identity` falls back to a hash of
`source + summary` with digits stripped (so "CPU at 92%" and "CPU at 95%" count as the same
alert); a missing `env` falls back to `"unknown"`.

## Ship a preset for everyone

A preset ships inside the alertsift binary, so anyone using that tool gets it without writing any
TOML:

1. Add `presets/<tool>.toml` — the same `alerts` + `[fields]` shape as above, without the
   `[source.name]` wrapper (see `presets/grafana.toml` for the shape).
2. Add a real payload, with anything private removed, to `tests/fixtures/<tool>.json`.
3. Register the preset in the `PRESETS` list in `src/config.rs`.
4. Add a test to `tests/config.rs` asserting `status`, `identity`, `summary` and `env` on the
   fixture.
5. Run `./cargo.sh test --test config` and see the new test fail before the preset exists, then
   pass once it does.
