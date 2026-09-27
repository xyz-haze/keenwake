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
