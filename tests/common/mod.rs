//! Shared test fixtures. Each test file uses only part of them.
#![allow(dead_code, unused_imports)]

mod http;

pub use http::*;

use keenwake::config::{Config, Mode};
use keenwake::decide::Kind;
use keenwake::mapping::{Alert, Status};
use keenwake::server::App;
use keenwake::store::{DecisionRow, Store};
use tempfile::TempDir;

/// A fixed "now" for tests that do not move the clock.
pub const T0: i64 = 1_800_000_000;

/// A prod alert; override fields with `Alert { summary: "..".into(), ..alert(status) }`.
pub fn alert(status: Status) -> Alert {
    Alert {
        source: "s".into(),
        status,
        identity: "id".into(),
        summary: "x".into(),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    }
}

/// An observe-mode decision with no optional field set; override like `alert`.
pub fn decision(event_seq: i64, decided_at: i64, kind: Kind) -> DecisionRow {
    DecisionRow {
        event_seq,
        decided_at,
        mode: Mode::Observe,
        kind,
        probability: None,
        reason: "".into(),
        delivered: false,
        backend_ms: None,
        input_tokens: None,
    }
}

/// A valid config: the backend at `backend`, every output posting to `{out}/<output name>`.
pub fn config(mode: Mode, backend: &str, out: &str) -> Config {
    Config::from_toml(&format!(
        "[backend]\nurl = '{backend}'\nmodel = 'm-1'\n[decision]\nmode = '{}'\n\
         [outputs]\nping = '{out}/ping'\nescalate = '{out}/escalate'\ndigest = '{out}/digest'\nverdict = '{out}/verdict'\n",
        mode.as_str()
    ))
    .unwrap()
}

/// An `App` whose undelivered file lives in the returned temp dir, which must outlive it.
pub fn app(mut cfg: Config, store: Store, clock: fn() -> i64) -> (App, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    cfg.store.undelivered = dir.path().join("undelivered.jsonl").display().to_string();
    (App::new(cfg, store, clock).unwrap(), dir)
}

/// A Grafana webhook body carrying one alert.
pub fn grafana(summary: &str, fingerprint: &str, status: &str) -> Vec<u8> {
    serde_json::json!({"alerts": [{"status": status, "fingerprint": fingerprint,
        "labels": {"alertname": "A", "env": "prod", "severity": "critical"},
        "annotations": {"summary": summary}}]})
    .to_string()
    .into_bytes()
}
