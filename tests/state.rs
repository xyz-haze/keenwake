mod common;

use common::alert;
use keenwake::history::Facts;
use keenwake::mapping::{Alert, Status};
use keenwake::state::sentence;

/// The alert of the spike whose wording `sentence` mirrors.
fn a(env: &str) -> Alert {
    Alert {
        summary: "CPU usage above 90% on etl-runner-2".into(),
        details: "CPU at 92% for 2m".into(),
        env: env.into(),
        ..alert(Status::Firing)
    }
}

#[test]
fn matches_spike_wording_with_history() {
    let f = Facts { env: "prod".into(), firing: true, minutes: 3, episodes_7d: 22, median_minutes: Some(3) };
    assert_eq!(sentence(&a("prod"), &f),
        "Environment: prod. The alert is still firing, for 3 minutes so far. It fired 22 times in the last 7 days, and each time it ended, usually within about 3 minutes. This time it looks like its usual pattern so far. Alert: CPU usage above 90% on etl-runner-2. CPU at 92% for 2m. Severity label: critical.");
}

#[test]
fn longer_than_usual_and_not_production() {
    let f = Facts { env: "staging".into(), firing: true, minutes: 64, episodes_7d: 4, median_minutes: Some(5) };
    let s = sentence(&a("staging"), &f);
    assert!(s.starts_with(
        "Environment: staging. This is not production. The alert is still firing, for 64 minutes so far."
    ));
    assert!(s.contains("It fired 4 times in the last 7 days, and each time it ended, usually within about 5 minutes."));
    assert!(!s.contains("on its own"), "keenwake cannot tell self-resolution from a human fix");
    assert!(s.contains("This time it has lasted much longer than usual."));
}

#[test]
fn first_time() {
    let f = Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, median_minutes: None };
    assert!(sentence(&a("prod"), &f).contains("This alert has never fired before in the last 7 days."));
}

#[test]
fn empty_details_are_skipped() {
    let mut x = a("prod");
    x.details = "".into();
    let f = Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, median_minutes: None };
    assert!(sentence(&x, &f).ends_with("Alert: CPU usage above 90% on etl-runner-2. Severity label: critical."));
}

#[test]
fn huge_fields_are_truncated_on_char_boundaries() {
    let mut x = a("prod");
    x.summary = "é".repeat(10_000);
    x.details = "x".repeat(10_000);
    let f = Facts { env: "prod".into(), firing: true, minutes: 0, episodes_7d: 0, median_minutes: None };
    let s = sentence(&x, &f);
    assert!(s.chars().count() < 2_400);
}

/// Flaps that end within the minute have a median of 0: one more minute must not read as
/// "much longer than usual", and the sentence must not say "about 0 minutes".
#[test]
fn sub_minute_median() {
    let f = Facts { env: "prod".into(), firing: true, minutes: 1, episodes_7d: 6, median_minutes: Some(0) };
    let s = sentence(&a("prod"), &f);
    assert!(s.contains("It fired 6 times in the last 7 days, and each time it ended within a minute."), "{s}");
    assert!(s.contains("This time it looks like its usual pattern so far."), "{s}");
    let f = Facts { minutes: 4, ..f };
    assert!(sentence(&a("prod"), &f).contains("This time it has lasted much longer than usual."));
}
