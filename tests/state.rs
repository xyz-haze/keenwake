use keenwake::history::Facts;
use keenwake::mapping::{Alert, Status};
use keenwake::state::sentence;

fn a(env: &str) -> Alert {
    Alert {
        source: "s".into(),
        status: Status::Firing,
        identity: "i".into(),
        summary: "CPU usage above 90% on etl-runner-2".into(),
        details: "CPU at 92% for 2m".into(),
        env: env.into(),
        severity: "critical".into(),
    }
}

#[test]
fn matches_spike_wording_with_history() {
    let f = Facts {
        env: "prod".into(),
        firing: true,
        minutes: 3,
        episodes_7d: 22,
        resolved_7d: 22,
        median_minutes: Some(3),
    };
    assert_eq!(sentence(&a("prod"), &f),
        "Environment: prod. The alert is still firing, for 3 minutes so far. It fired 22 times in the last 7 days and resolved on its own 100% of the time, usually within about 3 minutes. This time it looks like its usual pattern so far. Alert: CPU usage above 90% on etl-runner-2. CPU at 92% for 2m. Severity label: critical.");
}

#[test]
fn longer_than_usual_and_not_production() {
    let f = Facts {
        env: "staging".into(),
        firing: true,
        minutes: 64,
        episodes_7d: 4,
        resolved_7d: 2,
        median_minutes: Some(5),
    };
    let s = sentence(&a("staging"), &f);
    assert!(s.starts_with(
        "Environment: staging. This is not production. The alert is still firing, for 64 minutes so far."
    ));
    assert!(s.contains("resolved on its own 50% of the time, usually within about 5 minutes."));
    assert!(s.contains("This time it has lasted much longer than usual."));
}

#[test]
fn first_time() {
    let f =
        Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, resolved_7d: 0, median_minutes: None };
    assert!(sentence(&a("prod"), &f).contains("This alert has never fired before in the last 7 days."));
}

#[test]
fn fired_but_never_resolved_has_no_median() {
    let f =
        Facts { env: "prod".into(), firing: true, minutes: 1, episodes_7d: 3, resolved_7d: 0, median_minutes: None };
    let s = sentence(&a("prod"), &f);
    assert!(s.contains("It fired 3 times in the last 7 days and never resolved on its own."));
    assert!(!s.contains("usual"));
}

#[test]
fn empty_details_are_skipped() {
    let mut x = a("prod");
    x.details = "".into();
    let f =
        Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, resolved_7d: 0, median_minutes: None };
    assert!(sentence(&x, &f).ends_with("Alert: CPU usage above 90% on etl-runner-2. Severity label: critical."));
}

#[test]
fn huge_fields_are_truncated_on_char_boundaries() {
    let mut x = a("prod");
    x.summary = "é".repeat(10_000);
    x.details = "x".repeat(10_000);
    let f =
        Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, resolved_7d: 0, median_minutes: None };
    let s = sentence(&x, &f);
    assert!(s.chars().count() < 2_400);
}
