use alertsift::backend::request_body;
use alertsift::config::{Config, Question};
use alertsift::decide::{Kind, Target};
use alertsift::mapping::{extract, Alert, Status};
use alertsift::pipeline::{finish, prepare};
use alertsift::redact::Redactor;
use alertsift::store::{DecisionRow, Store};
use proptest::prelude::*;

fn cfg(mode: &str) -> Config {
    Config::from_toml(&format!("[backend]\nurl='http://x'\nmodel='m-1'\n[decision]\nmode='{mode}'\n[outputs]\nping='http://p'\n")).unwrap()
}
fn red() -> Redactor { Redactor::new(&["email".into(), "ip".into(), "token".into()]) }
fn alert(status: Status) -> Alert {
    Alert { source: "s".into(), status, identity: "id".into(), summary: "Disk full on ops@example.com".into(),
            details: "".into(), env: "prod".into(), severity: "critical".into() }
}

#[test]
fn prepare_redacts_before_storing_and_building_state() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Firing), 1_800_000_000);
    assert!(!p.state.contains("ops@example.com"));
    assert!(!s.events_since(0)[0].alert.summary.contains("ops@example.com"));
    assert!(p.needs_model);
}

#[test]
fn resolved_needs_no_model() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Resolved), 1_800_000_000);
    assert!(!p.needs_model);
}

#[test]
fn resolved_for_unknown_identity_is_harmless() {
    let s = Store::memory();
    let p = prepare(&s, &red(), alert(Status::Resolved), 1_800_000_000);
    assert!(!s.episode_pinged("id", p.event_seq));
    assert_eq!(s.events_since(0).len(), 1);
}

#[test]
fn repeated_firing_after_ping_is_repeat_in_gate() {
    let s = Store::memory();
    let c = cfg("gate");
    let p1 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_000);
    let r1 = finish(&c, &p1, Ok(0.9));
    assert_eq!(r1.target, Target::Ping);
    s.insert_decision(&DecisionRow { event_seq: p1.event_seq, decided_at: 0, mode: "gate".into(), kind: "ping".into(),
        probability: Some(0.9), reason: "".into(), delivered: true, backend_ms: None, input_tokens: None });
    let p2 = prepare(&s, &red(), alert(Status::Firing), 1_800_000_060);
    assert!(p2.already_pinged);
    assert_eq!(finish(&c, &p2, Ok(0.9)).kind, Kind::Repeat);
    assert!(p2.state.contains("for 1 minutes so far"));
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
            let p = prepare(&s, &red(), a, 1_800_000_000);
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
            let pr = prepare(&s, &red(), alert(if *firing { Status::Firing } else { Status::Resolved }), 1_800_000_000 + 60 * i as i64);
            if pr.needs_model { first.push((pr.event_seq, finish(&c, &pr, Ok(*p)).kind)); }
        }
        for (seq, kind) in first {
            let ev = s.events_since(0).into_iter().find(|e| e.seq == seq).unwrap();
            let before = s.events_for(&ev.alert.identity, ev.received_at - alertsift::history::WINDOW_SECS, seq);
            let f = alertsift::history::facts(&before, &ev.alert, ev.received_at);
            let pr = alertsift::pipeline::Prepared { event_seq: seq, alert: ev.alert.clone(), facts: f,
                state: String::new(), already_pinged: s.episode_pinged(&ev.alert.identity, seq), needs_model: true };
            let p = ps[(seq - 1) as usize].0;
            prop_assert_eq!(finish(&c, &pr, Ok(p)).kind, kind);
        }
    }
}
