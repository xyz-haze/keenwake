use alertsift::mapping::{Alert, Status};
use alertsift::report::{build, parse_since};
use alertsift::store::{DecisionRow, Store};

fn ev(s: &Store, summary: &str, kind: &str, p: Option<f64>, tokens: i64) {
    let a = Alert { source: "g".into(), status: Status::Firing, identity: summary.into(), summary: summary.into(),
                    details: "".into(), env: "prod".into(), severity: "critical".into() };
    let seq = s.insert_event(&a, 1_800_000_000);
    s.insert_decision(&DecisionRow { event_seq: seq, decided_at: 1_800_000_000, mode: "observe".into(), kind: kind.into(),
        probability: p, reason: "".into(), delivered: false, backend_ms: Some(100), input_tokens: Some(tokens) });
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
    ev(&s, "a", "ping", Some(0.9), 500_000);
    ev(&s, "b", "digest", Some(0.1), 500_000);
    ev(&s, "c", "untriaged", None, 0);
    let r = build(&s, 0, 0.042);
    assert_eq!(r.total, 3);
    assert_eq!(r.by_kind["ping"], 1);
    assert_eq!(r.by_kind["digest"], 1);
    assert_eq!(r.backend_errors, 1);
    assert_eq!(r.input_tokens, 1_000_000);
    assert!((r.est_cost_usd - 0.042).abs() < 1e-9);
    assert_eq!(r.first_seen, 3);
    assert!(r.to_text().contains("ping"));
}
