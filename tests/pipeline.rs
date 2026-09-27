mod common;

use common::{alert, config, decision};
use keenwake::backend::request_body;
use keenwake::config::{Config, Mode, Question};
use keenwake::decide::{repeat_floor, route, Kind, Target};
use keenwake::mapping::{extract, Alert, Status};
use keenwake::pipeline::prepare;
use keenwake::redact::{Pattern, Redactor};
use keenwake::store::{DecisionRow, Store};
use proptest::prelude::*;

const WINDOW: i64 = 24 * 3600;

fn cfg(mode: Mode) -> Config {
    config(mode, "http://backend", "http://out")
}
fn red() -> Redactor {
    Redactor::new(&[Pattern::Email, Pattern::Ip, Pattern::Token])
}

/// Prepares an alert at `at`, and if it needs the model, routes it with probability `p` and
/// stores the decision the way `serve` would (every notification in gate is delivered).
fn decide_and_store(c: &Config, s: &Store, status: Status, at: i64, p: f64) -> Option<(i64, Kind)> {
    let pr = prepare(s, &red(), alert(status), at, WINDOW);
    if !pr.needs_model {
        return None;
    }
    let r = route(&c.decision, Some(p), pr.floor);
    s.insert_decision(&DecisionRow {
        mode: Mode::Gate,
        probability: Some(p),
        delivered: r.target != Target::Nothing,
        ..decision(pr.event_seq, at, r.kind)
    });
    Some((pr.event_seq, r.kind))
}

/// The repeat floor `serve` saw when `ev` arrived, recomputed from the store.
fn floor_at(s: &Store, ev: &keenwake::store::Event) -> Option<Kind> {
    let sent = s.episode_sent(&ev.alert.identity, ev.seq, ev.received_at - WINDOW);
    repeat_floor(&sent, ev.received_at, WINDOW)
}

#[test]
fn prepare_redacts_before_storing_and_building_state() {
    let s = Store::memory();
    let a = Alert { summary: "Disk full on ops@example.com".into(), ..alert(Status::Firing) };
    let p = prepare(&s, &red(), a, 1_800_000_000, WINDOW);
    assert!(!p.state.contains("ops@example.com"));
    assert!(!s.events_since(0)[0].alert.summary.contains("ops@example.com"));
    assert!(p.needs_model);
}

#[test]
fn resolved_needs_no_model() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Resolved), 1_800_000_000, WINDOW);
    assert!(!p.needs_model);
}

#[test]
fn resolved_for_unknown_identity_is_harmless() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Resolved), 1_800_000_000, WINDOW);
    assert!(s.episode_sent("id", p.event_seq, 0).is_empty());
    assert_eq!(s.events_since(0).len(), 1);
}

#[test]
fn repeated_firing_after_ping_is_repeat_in_gate() {
    let s = Store::memory();
    let c = cfg(Mode::Gate);
    let p1 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_000, WINDOW);
    let r1 = route(&c.decision, Some(0.9), p1.floor);
    assert_eq!(r1.target, Target::Ping);
    s.insert_decision(&DecisionRow {
        mode: Mode::Gate,
        probability: Some(0.9),
        delivered: true,
        ..decision(p1.event_seq, 1_800_000_000, Kind::Ping)
    });
    let p2 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_060, WINDOW);
    assert_eq!(p2.floor, Some(Kind::Ping));
    assert_eq!(route(&c.decision, Some(0.9), p2.floor).kind, Kind::Repeat);
    assert!(p2.state.contains("for 1 minutes so far"));
}

/// Proves the harness can produce `Kind::Repeat` at all, so the replay property below is not
/// vacuous: firing, firing (repeat), resolved, firing (new episode, pings again).
#[test]
fn replay_reproduces_repeat() {
    let c = cfg(Mode::Gate);
    let s = Store::memory();
    let statuses = [Status::Firing, Status::Firing, Status::Resolved, Status::Firing];
    let p = 0.9;
    let first: Vec<(i64, Kind)> = statuses
        .iter()
        .enumerate()
        .filter_map(|(i, status)| decide_and_store(&c, &s, *status, 1_800_000_000 + 60 * i as i64, p))
        .collect();
    let kinds: Vec<Kind> = first.iter().map(|(_, k)| *k).collect();
    assert_eq!(kinds, vec![Kind::Ping, Kind::Repeat, Kind::Ping]);

    for (seq, kind) in &first {
        let ev = s.events_since(0).into_iter().find(|e| e.seq == *seq).unwrap();
        let floor = floor_at(&s, &ev);
        assert_eq!(route(&c.decision, Some(p), floor).kind, *kind);
    }
}

proptest! {
    // Invariant 1: a secret placed in an unmapped field never reaches the backend request.
    #[test]
    fn unmapped_secret_never_reaches_the_backend(secret in "[A-Za-z]{12,24}", field in "[a-z]{3,10}") {
        // every name a grafana preset field reads is excluded: those are mapped, so they may legitimately reach the model
        prop_assume!(!["status","labels","annotations","fingerprint","alertname","env","environment",
                       "severity","summary","description","message"].contains(&field.as_str()));
        let c = cfg(Mode::Observe);
        let body = serde_json::json!({"alerts": [{
            "status": "firing", "fingerprint": "f1",
            "labels": {"alertname": "A", "env": "prod", "severity": "critical", field.clone(): secret.clone()},
            "annotations": {"summary": "CPU high"},
            field.clone(): secret.clone(),
        }]});
        let alerts = extract("grafana", &c.sources["grafana"], body.to_string().as_bytes()).unwrap();
        let s = Store::memory();
        for a in alerts {
            let p = prepare(&s, &red(), a, 1_800_000_000, WINDOW);
            let req = request_body("m-1", &p.state, &Question::default()).to_string();
            prop_assert!(!req.contains(&secret));
        }
    }

    // Invariant 6: same config, deterministic backend -> same decisions when replayed.
    #[test]
    fn replay_is_deterministic(ps in prop::collection::vec((0.0f64..=1.0, any::<bool>()), 1..30)) {
        let c = cfg(Mode::Gate);
        let s = Store::memory();
        let mut first = Vec::new();
        for (i, (p, firing)) in ps.iter().enumerate() {
            let status = if *firing { Status::Firing } else { Status::Resolved };
            first.extend(decide_and_store(&c, &s, status, 1_800_000_000 + 60 * i as i64, *p));
        }
        for (seq, kind) in first {
            let ev = s.events_since(0).into_iter().find(|e| e.seq == seq).unwrap();
            let p = ps[(seq - 1) as usize].0;
            prop_assert_eq!(route(&c.decision, Some(p), floor_at(&s, &ev)).kind, kind);
        }
    }
}
