mod common;

use common::{alert, app, config, decision, system_one_from_state, FakeHttp, T0};
use keenwake::config::Mode;
use keenwake::decide::Kind;
use keenwake::mapping::{Alert, Status};
use keenwake::report::{build, parse_since, replay};
use keenwake::server::App;
use keenwake::store::{DecisionRow, Store};

fn ev(s: &Store, summary: &str, kind: Kind, p: Option<f64>, tokens: i64) {
    let seq = s.insert_event(&Alert { identity: summary.into(), summary: summary.into(), ..alert(Status::Firing) }, T0);
    s.insert_decision(&DecisionRow {
        probability: p,
        backend_ms: Some(100),
        input_tokens: Some(tokens),
        ..decision(seq, T0, kind)
    });
}

/// Inserts one event for `identity` at `at` with a decision of the given stored `kind`. The
/// alert's own status mirrors the kind only for `resolved` (real resolved alerts skip the model).
/// Its summary makes the fake backend answer 0.90.
fn ev_kind_at(s: &Store, identity: &str, kind: Kind, at: i64) -> i64 {
    let status = if kind == Kind::Resolved { Status::Resolved } else { Status::Firing };
    let seq = s.insert_event(&Alert { identity: identity.into(), summary: "p=0.90 disk".into(), ..alert(status) }, at);
    s.insert_decision(&DecisionRow { probability: Some(0.9), ..decision(seq, at, kind) });
    seq
}

/// An app replaying in gate mode against the fake `backend`, with the clock at `now`.
fn gate_replay_app(backend: &FakeHttp, store: Store, now: fn() -> i64) -> (App, tempfile::TempDir) {
    app(config(Mode::Gate, &backend.url, "http://out"), store, now)
}

#[test]
fn since_parses_units() {
    assert_eq!(parse_since("7d").unwrap(), 7 * 86_400);
    assert_eq!(parse_since("12h").unwrap(), 12 * 3600);
    assert_eq!(parse_since("30m").unwrap(), 1800);
    assert!(parse_since("7x").is_err());
}

#[test]
fn since_rejects_non_ascii_negative_and_overflowing_input() {
    for bad in ["7é", "é", "é7d", "", "d", "-7d", "+7d", " 7d", "7 d", "99999999999999999d"] {
        assert!(parse_since(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[test]
fn report_counts_and_costs() {
    let s = Store::memory();
    ev(&s, "a", Kind::Ping, Some(0.9), 500_000);
    ev(&s, "b", Kind::Digest, Some(0.1), 500_000);
    ev(&s, "c", Kind::Untriaged, None, 0);
    let r = build(&s, 0, 0.042, 24 * 3600);
    assert_eq!(r.total, 3);
    assert_eq!(r.by_kind["ping"], 1);
    assert_eq!(r.by_kind["digest"], 1);
    assert_eq!(r.backend_errors, 1);
    assert_eq!(r.input_tokens, 1_000_000);
    assert!((r.est_cost_usd - 0.042).abs() < 1e-9);
    assert_eq!(r.first_seen, 3);
    assert!(r.to_string().contains("ping"));
}

#[test]
fn report_counts_avoidable_pings() {
    let s = Store::memory();
    for (identity, kind) in [
        ("id-a", Kind::Ping),
        ("id-a", Kind::Ping),
        ("id-a", Kind::Resolved),
        ("id-a", Kind::Ping),
        ("id-b", Kind::Digest),
        ("id-b", Kind::Escalate),
    ] {
        ev_kind_at(&s, identity, kind, T0);
    }
    let r = build(&s, 0, 0.042, 24 * 3600);
    assert_eq!(r.avoidable_pings, 3);
}

#[tokio::test]
async fn replay_simulates_repeat_from_observe_history() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let s = Store::memory();
    ev_kind_at(&s, "id1", Kind::Ping, T0);
    let seq2 = ev_kind_at(&s, "id1", Kind::Ping, T0 + 60);
    let (app, _d) = gate_replay_app(&fake, s, || T0 + 120);

    let changed = replay(&app, 0).await;
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].seq, seq2);
    assert_eq!(changed[0].old, Kind::Ping);
    assert_eq!(changed[0].new, Kind::Repeat);
}

#[test]
fn report_avoidable_pings_respect_the_repeat_window() {
    let s = Store::memory();
    ev_kind_at(&s, "id-a", Kind::Ping, T0);
    ev_kind_at(&s, "id-a", Kind::Ping, T0 + 30 * 86_400); // lost resolved: a new incident, not avoidable
    ev_kind_at(&s, "id-b", Kind::Ping, T0);
    ev_kind_at(&s, "id-b", Kind::Ping, T0 + 3600); // a repeat within 24 h: avoidable
    assert_eq!(build(&s, 0, 0.042, 24 * 3600).avoidable_pings, 1);
}

#[tokio::test]
async fn replay_repeat_suppression_is_bounded_by_the_window() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let s = Store::memory();
    ev_kind_at(&s, "id1", Kind::Ping, T0);
    ev_kind_at(&s, "id1", Kind::Ping, T0 + 30 * 86_400);
    let (app, _d) = gate_replay_app(&fake, s, || T0 + 31 * 86_400);
    assert_eq!(replay(&app, 0).await, vec![]);
}

const MIN_TOML: &str = r#"
[backend]
url = "https://api.typesafe.ai"
model = "jev-1.13.0"

[source.probe]
[source.probe.fields]
status = { const = "firing" }
summary = { template = "{probe/name}: {result/eror}" }
env = { path = "/labels/stage" }
"#;

/// Runs the real binary on `payload` with a `probe` source, and returns (exit ok, stdout, stderr).
fn check_source(payload: &str) -> (bool, String, String) {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("keenwake.toml");
    let body = d.path().join("payload.json");
    std::fs::write(&cfg, MIN_TOML).unwrap();
    std::fs::write(&body, payload).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_keenwake"))
        .arg("--config")
        .arg(&cfg)
        .args(["check-source", "--source", "probe"])
        .arg(&body)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

/// A typo in a template key or an env path used to yield "api-health: " and "unknown" silently.
#[test]
fn check_source_warns_and_fails_on_references_that_resolve_to_nothing() {
    let (ok, stdout, stderr) = check_source(r#"{"probe": {"name": "api-health"}, "result": {"error": "timeout"}}"#);
    assert!(!ok, "a warning must fail check-source\n{stderr}");
    assert!(stderr.contains("warning: alert 0, field summary: /result/eror resolved to nothing"), "{stderr}");
    assert!(stderr.contains("warning: alert 0, field env: /labels/stage resolved to nothing"), "{stderr}");
    assert!(stdout.contains("state sent to the model"), "the extraction is still shown\n{stdout}");
}

#[test]
fn check_source_is_quiet_and_succeeds_when_everything_resolves() {
    let (ok, _, stderr) = check_source(
        r#"{"probe": {"name": "api-health"}, "result": {"eror": "timeout"}, "labels": {"stage": "prod"}}"#,
    );
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("warning"), "{stderr}");
}

/// Inserts `n` alerts one minute apart from T0, alternating digest (p=0.12) and ping (p=0.91).
fn minutes_of_alerts(s: &Store, n: i64) {
    for i in 0..n {
        let (kind, p) = if i % 2 == 0 { (Kind::Digest, 0.12) } else { (Kind::Ping, 0.91) };
        let at = T0 + i * 60;
        let seq = s.insert_event(&Alert { summary: format!("alert {i}"), ..alert(Status::Firing) }, at);
        s.insert_decision(&DecisionRow { probability: Some(p), ..decision(seq, at, kind) });
    }
}

/// Counts alone do not tell which alerts would have been held back: the text report lists them.
#[test]
fn report_text_lists_each_decision_oldest_first() {
    let s = Store::memory();
    minutes_of_alerts(&s, 2);
    let s = build(&s, 0, 0.042, 24 * 3600).to_string();
    let a = s.find("2027-01-15 08:00 UTC  digest     p=0.12  alert 0").expect(&s);
    let b = s.find("2027-01-15 08:01 UTC  ping       p=0.91  alert 1").expect(&s);
    assert!(a < b, "most recent last\n{s}");
    assert!(!s.contains("more, use --json"), "{s}");
}

#[test]
fn report_text_keeps_the_last_30_decisions() {
    let s = Store::memory();
    minutes_of_alerts(&s, 32);
    let s = build(&s, 0, 0.042, 24 * 3600).to_string();
    assert!(s.contains("  ... 2 more, use --json\n"), "{s}");
    assert!(!s.contains("  alert 1\n"), "{s}");
    assert!(s.contains("  alert 2\n"), "{s}");
    assert!(s.contains("  alert 31\n"), "{s}");
}

#[test]
fn report_json_rows_keep_their_fields() {
    let s = Store::memory();
    minutes_of_alerts(&s, 1);
    let v = serde_json::to_value(build(&s, 0, 0.042, 24 * 3600)).unwrap();
    let keys: Vec<&String> = v["rows"][0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["identity", "kind", "probability", "summary"]);
}

/// The date is computed without a date crate: a leap day and a non-leap century are the traps.
#[test]
fn report_text_dates_are_utc_calendar_dates() {
    let s = Store::memory();
    for at in [1_835_481_599, 4_107_542_400] {
        let seq = s.insert_event(&alert(Status::Firing), at);
        s.insert_decision(&decision(seq, at, Kind::Untriaged));
    }
    let s = build(&s, 0, 0.042, 24 * 3600).to_string();
    assert!(s.contains("  2028-02-29 23:59 UTC  untriaged  p=-     x\n"), "{s}");
    assert!(s.contains("  2100-03-01 00:00 UTC  untriaged  p=-     x\n"), "{s}");
}
