# Add a source

A source is one alerting tool, not one alert: every alert a tool sends shares its webhook format.
These steps work the same for a person or a coding agent.

## For your own setup

1. **Capture a real payload.** Start a one-shot listener that saves the next webhook it receives
   to `payload.json`, then exits:

   ```sh
   python3 -c '
   import http.server as h
   class H(h.BaseHTTPRequestHandler):
       def do_POST(s):
           open("payload.json", "wb").write(s.rfile.read(int(s.headers["Content-Length"])))
           s.send_response(200); s.end_headers()
   h.HTTPServer(("", 9000), H).handle_request()'
   ```

   Point the tool's webhook at `http://<this-machine>:9000` and send a test notification (most
   tools have a "Test" button). Don't write a payload from memory or from vendor docs: what a
   tool sends often differs from what it documents.

2. **Map it in your `keenwake.toml`.** Example: a health-check script that posts
   `{"check": "backup", "state": "KO", "line": "..."}`.

   ```toml
   [source.healthcheck]
   alerts = ""                     # "" = one alert per webhook, or a JSON pointer to an array
   [source.healthcheck.fields]
   status = { path = "/state", map = { KO = "firing", OK = "resolved" } }
   identity = { path = "/check" }
   summary = { template = "{check} failed: {line}" }
   env = { const = "prod" }
   ```

3. **Check it.** No network, no model call:

   ```sh
   keenwake --config keenwake.toml check-source --source healthcheck payload.json
   ```

   It prints the extracted fields and the exact sentence the model would get.

4. **Point the tool** at `http://keenwake:8080/hook/healthcheck`.

## Mapping rules

Six fields: `status` (required, must end up `firing` or `resolved`), `identity`, `summary`,
`details`, `env`, `severity`. Nothing else in the payload is ever read (invariant 1 in
[invariants.md](invariants.md)). Without `identity`, keenwake hashes `source + summary` with
digits stripped, so "CPU at 92%" and "CPU at 95%" count as the same alert. Without `env` or
`severity`, it uses `unknown`.

Five primitives, nothing else:

| Primitive | Does | Example |
|---|---|---|
| `path` | Reads one field (JSON Pointer) | `{ path = "/annotations/summary" }` |
| `first_of` | First pointer that is present | `{ first_of = ["/title", "/message"] }` |
| `map` | Translates values | `{ path = "/state", map = { KO = "firing", OK = "resolved" } }` |
| `const` | Fixed value | `{ const = "prod" }` |
| `template` | Builds text from fields | `{ template = "{check} failed: {line}" }`, nested: `{labels/env}` |

No conditions, no calculations, on purpose. If `status` cannot be expressed with `path` + `map`,
put a small script in front of keenwake instead of a source section.

## Ship a preset for everyone

A preset is built into the binary, so users of that tool write no TOML.

1. Save a real payload, with anything private removed, to `tests/fixtures/<tool>.json`.
2. Add a test to `tests/config.rs` asserting `status`, `identity`, `summary` and `env` on that
   fixture. Run `scripts/cargo.sh test --test config` and see it fail.
3. Write `presets/<tool>.toml`: the `alerts` + `[fields]` shape above, without the
   `[source.name]` wrapper (see `presets/grafana.toml`). Register it in `PRESETS` in
   `src/config.rs`.
4. Run the test again and see it pass. Then check the full output:

   ```sh
   scripts/cargo.sh run -- --config demo/keenwake.toml check-source --source <tool> tests/fixtures/<tool>.json
   ```

   Any valid config works here, since `check-source` never calls the backend.
