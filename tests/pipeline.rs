use keenwake::backend::request_body;
use keenwake::config::{Config, Mode, Question};
use keenwake::decide::{route, Kind, Target};
use keenwake::mapping::{extract, Alert, Status};
use keenwake::pipeline::prepare;
use keenwake::redact::{Pattern, Redactor};
use keenwake::store::{DecisionRow, Store};
use proptest::prelude::*;

const WINDOW: i64 = 24 * 3600;

fn cfg(mode: &str) -> Config {
    Config::from_toml(&format!(
        "[backend]\nurl='http://x'\nmodel='m-1'\n[decision]\nmode='{mode}'\n[outputs]\nping='http://p'\nescalate='http://e'\ndigest='http://d'\n"
    )).unwrap()
}
fn red() -> Redactor {
    Redactor::new(&[Pattern::Email, Pattern::Ip, Pattern::Token])
}
fn alert(status: Status) -> Alert {
    Alert {
        source: "s".into(),
        status,
        identity: "id".into(),
        summary: "Disk full on ops@example.com".into(),
        details: "".into(),
        env: "prod".into(),
        severity: "critical".into(),
    }
}

#[test]
fn prepare_redacts_before_storing_and_building_state() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Firing), 1_800_000_000, WINDOW);
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
    assert!(!s.episode_pinged("id", p.event_seq, 0));
    assert_eq!(s.events_since(0).len(), 1);
}

#[test]
fn repeated_firing_after_ping_is_repeat_in_gate() {
    let s = Store::memory();
    let c = cfg("gate");
    let p1 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_000, WINDOW);
    let r1 = route(&c.decision, Ok(0.9), p1.already_pinged);
    assert_eq!(r1.target, Target::Ping);
    s.insert_decision(&DecisionRow {
        event_seq: p1.event_seq,
        decided_at: 1_800_000_000,
        mode: Mode::Gate,
        kind: Kind::Ping,
        probability: Some(0.9),
        reason: "".into(),
        delivered: true,
        backend_ms: None,
        input_tokens: None,
    });
    let p2 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_060, WINDOW);
    assert!(p2.already_pinged);
    assert_eq!(route(&c.decision, Ok(0.9), p2.already_pinged).kind, Kind::Repeat);
    assert!(p2.state.contains("for 1 minutes so far"));
}

/// Fixed-sequence sanity check for the replay property below: firing, firing (repeat), resolved,
/// firing (fresh episode, pings again). Confirms the harness can actually produce `Kind::Repeat`
/// and that replaying it from stored events and decisions reproduces the same kinds.
#[test]
fn replay_reproduces_repeat() {
    let c = cfg("gate");
    let s = Store::memory();
    let statuses = [Status::Firing, Status::Firing, Status::Resolved, Status::Firing];
    let p = 0.9;
    let mut first = Vec::new();
    for (i, status) in statuses.iter().enumerate() {
        let pr = prepare(&s, &red(), alert(*status), 1_800_000_000 + 60 * i as i64, WINDOW);
        if pr.needs_model {
            let r = route(&c.decision, Ok(p), pr.already_pinged);
            s.insert_decision(&DecisionRow {
                event_seq: pr.event_seq,
                decided_at: 1_800_000_000 + 60 * i as i64,
                mode: Mode::Gate,
                kind: r.kind,
                probability: Some(p),
                reason: String::new(),
                delivered: r.target == Target::Ping,
                backend_ms: None,
                input_tokens: None,
            });
            first.push((pr.event_seq, r.kind));
        }
    }
    let kinds: Vec<Kind> = first.iter().map(|(_, k)| *k).collect();
    assert_eq!(kinds, vec![Kind::Ping, Kind::Repeat, Kind::Ping]);

    for (seq, kind) in &first {
        let ev = s.events_since(0).into_iter().find(|e| e.seq == *seq).unwrap();
        let already_pinged = s.episode_pinged(&ev.alert.identity, *seq, ev.received_at - WINDOW);
        assert_eq!(route(&c.decision, Ok(p), already_pinged).kind, *kind);
    }
}

proptest! {
    // Invariant 1: a secret placed in an unmapped field never reaches the backend request.
    #[test]
    fn unmapped_secret_never_reaches_the_backend(secret in "[A-Za-z]{12,24}", field in "[a-z]{3,10}") {
        // every name a grafana preset field reads is excluded: those are mapped, so they may legitimately reach the model
        prop_assume!(!["status","labels","annotations","fingerprint","alertname","env","environment",
                       "severity","summary","description","message"].contains(&field.as_str()));
        let c = cfg("observe");
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
        let c = cfg("gate");
        let s = Store::memory();
        let mut first = Vec::new();
        for (i, (p, firing)) in ps.iter().enumerate() {
            let at = 1_800_000_000 + 60 * i as i64;
            let pr = prepare(&s, &red(), alert(if *firing { Status::Firing } else { Status::Resolved }), at, WINDOW);
            if pr.needs_model {
                let r = route(&c.decision, Ok(*p), pr.already_pinged);
                s.insert_decision(&DecisionRow { event_seq: pr.event_seq, decided_at: at, mode: Mode::Gate,
                    kind: r.kind, probability: Some(*p), reason: String::new(),
                    delivered: r.target == Target::Ping, backend_ms: None, input_tokens: None });
                first.push((pr.event_seq, r.kind));
            }
        }
        for (seq, kind) in first {
            let ev = s.events_since(0).into_iter().find(|e| e.seq == seq).unwrap();
            let already_pinged = s.episode_pinged(&ev.alert.identity, seq, ev.received_at - WINDOW);
            let p = ps[(seq - 1) as usize].0;
            prop_assert_eq!(route(&c.decision, Ok(p), already_pinged).kind, kind);
        }
    }
}
