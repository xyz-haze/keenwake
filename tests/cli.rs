mod common;

use common::{system_one_from_state, FakeHttp};
use keenwake::config::{Config, Mode};
use keenwake::decide::Kind;
use keenwake::mapping::{Alert, Status};
use keenwake::report::{build, parse_since, replay};
use keenwake::server::App;
use keenwake::store::{DecisionRow, Store};

fn ev(s: &Store, summary: &str, kind: Kind, p: Option<f64>, tokens: i64) {
    let a = Alert {
        source: "g".into(),
        status: Status::Firing,
        identity: summary.into(),
        summary: summary.into(),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    };
    let seq = s.insert_event(&a, 1_800_000_000);
    s.insert_decision(&DecisionRow {
        event_seq: seq,
        decided_at: 1_800_000_000,
        mode: Mode::Observe,
        kind,
        probability: p,
        reason: "".into(),
        delivered: false,
        backend_ms: Some(100),
        input_tokens: Some(tokens),
    });
}

/// Inserts one event for `identity` with a decision of the given stored `kind`. The alert's own
/// status mirrors the kind only for `resolved` (real resolved alerts skip the model); the other
/// fields don't matter for `avoidable_pings`, which is computed purely from `d.kind`.
fn ev_kind(s: &Store, identity: &str, kind: Kind) {
    let status = if kind == Kind::Resolved { Status::Resolved } else { Status::Firing };
    let a = Alert {
        source: "g".into(),
        status,
        identity: identity.into(),
        summary: format!("{identity}-{}", kind.as_str()),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    };
    let seq = s.insert_event(&a, 1_800_000_000);
    s.insert_decision(&DecisionRow {
        event_seq: seq,
        decided_at: 1_800_000_000,
        mode: Mode::Observe,
        kind,
        probability: Some(0.9),
        reason: "".into(),
        delivered: false,
        backend_ms: Some(100),
        input_tokens: Some(0),
    });
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
    assert!(r.to_text().contains("ping"));
}

#[test]
fn report_counts_avoidable_pings() {
    let s = Store::memory();
    ev_kind(&s, "id-a", Kind::Ping);
    ev_kind(&s, "id-a", Kind::Ping);
    ev_kind(&s, "id-a", Kind::Resolved);
    ev_kind(&s, "id-a", Kind::Ping);
    ev_kind(&s, "id-b", Kind::Digest);
    ev_kind(&s, "id-b", Kind::Escalate);
    let r = build(&s, 0, 0.042, 24 * 3600);
    assert_eq!(r.avoidable_pings, 3);
}

#[tokio::test]
async fn replay_simulates_repeat_from_observe_history() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let s = Store::memory();
    let alert = Alert {
        source: "g".into(),
        status: Status::Firing,
        identity: "id1".into(),
        summary: "p=0.90 disk".into(),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    };
    let seq1 = s.insert_event(&alert, 1_800_000_000);
    s.insert_decision(&DecisionRow {
        event_seq: seq1,
        decided_at: 1_800_000_000,
        mode: Mode::Observe,
        kind: Kind::Ping,
        probability: Some(0.9),
        reason: "".into(),
        delivered: false,
        backend_ms: None,
        input_tokens: None,
    });
    let seq2 = s.insert_event(&alert, 1_800_000_060);
    s.insert_decision(&DecisionRow {
        event_seq: seq2,
        decided_at: 1_800_000_060,
        mode: Mode::Observe,
        kind: Kind::Ping,
        probability: Some(0.9),
        reason: "".into(),
        delivered: false,
        backend_ms: None,
        input_tokens: None,
    });

    let cfg = Config::from_toml(&format!(
        "[backend]\nurl='{}'\nmodel='m-1'\n[decision]\nmode='gate'\n[outputs]\nping='http://p'\nescalate='http://e'\ndigest='http://d'\n",
        fake.url
    )).unwrap();
    let app = App::new(cfg, s, || 1_800_000_120).unwrap();

    let changed = replay(&app, 0).await;
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].seq, seq2);
    assert_eq!(changed[0].old, Kind::Ping);
    assert_eq!(changed[0].new, Kind::Repeat);
}

fn ev_kind_at(s: &Store, identity: &str, kind: Kind, at: i64) {
    let a = Alert {
        source: "g".into(),
        status: Status::Firing,
        identity: identity.into(),
        summary: "p=0.90 disk".into(),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    };
    let seq = s.insert_event(&a, at);
    s.insert_decision(&DecisionRow {
        event_seq: seq,
        decided_at: at,
        mode: Mode::Observe,
        kind,
        probability: Some(0.9),
        reason: "".into(),
        delivered: false,
        backend_ms: None,
        input_tokens: None,
    });
}

#[test]
fn report_avoidable_pings_respect_the_repeat_window() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    ev_kind_at(&s, "id-a", Kind::Ping, t0);
    ev_kind_at(&s, "id-a", Kind::Ping, t0 + 30 * 86_400); // lost resolved: a new incident, not avoidable
    ev_kind_at(&s, "id-b", Kind::Ping, t0);
    ev_kind_at(&s, "id-b", Kind::Ping, t0 + 3600); // a repeat within 24 h: avoidable
    assert_eq!(build(&s, 0, 0.042, 24 * 3600).avoidable_pings, 1);
}

#[tokio::test]
async fn replay_repeat_suppression_is_bounded_by_the_window() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let s = Store::memory();
    let t0 = 1_800_000_000;
    ev_kind_at(&s, "id1", Kind::Ping, t0);
    ev_kind_at(&s, "id1", Kind::Ping, t0 + 30 * 86_400);
    let cfg = Config::from_toml(&format!(
        "[backend]\nurl='{}'\nmodel='m-1'\n[decision]\nmode='gate'\n[outputs]\nping='http://p'\nescalate='http://e'\ndigest='http://d'\n",
        fake.url
    )).unwrap();
    let app = App::new(cfg, s, || 1_800_000_000 + 31 * 86_400).unwrap();
    assert_eq!(replay(&app, 0).await, vec![]);
}
