use keenwake::history::facts;
use keenwake::mapping::{Alert, Status};
use keenwake::store::{DecisionRow, Store};
use proptest::prelude::*;

fn alert(status: Status) -> Alert {
    Alert { source: "s".into(), status, identity: "id".into(), summary: "x".into(),
            details: "".into(), env: "prod".into(), severity: "critical".into() }
}

const MIN: i64 = 60;

#[test]
fn counts_episodes_and_median() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    // three episodes of 2, 4 and 6 minutes, with a repeated firing inside the second
    for (start, dur) in [(0, 2), (60, 4), (120, 6)] {
        s.insert_event(&alert(Status::Firing), t0 + start * MIN);
        if start == 60 { s.insert_event(&alert(Status::Firing), t0 + (start + 1) * MIN); }
        s.insert_event(&alert(Status::Resolved), t0 + (start + dur) * MIN);
    }
    let now = t0 + 200 * MIN;
    let seq = s.insert_event(&alert(Status::Firing), now);
    let before = s.events_for("id", now - 7 * 24 * 3600, seq);
    let f = facts(&before, &alert(Status::Firing), now);
    assert_eq!(f.episodes_7d, 3);
    assert_eq!(f.resolved_7d, 3);
    assert_eq!(f.median_minutes, Some(4));
    assert_eq!(f.minutes, 0);
    assert!(f.firing);
}

#[test]
fn repeated_firing_measures_time_since_episode_start() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    s.insert_event(&alert(Status::Firing), t0);
    let seq = s.insert_event(&alert(Status::Firing), t0 + 17 * MIN);
    let before = s.events_for("id", t0 - 1, seq);
    let f = facts(&before, &alert(Status::Firing), t0 + 17 * MIN);
    assert_eq!(f.minutes, 17);
    assert_eq!(f.episodes_7d, 0, "the open episode is the current one, not history");
}

#[test]
fn first_time_has_no_history() {
    let f = facts(&[], &alert(Status::Firing), 1_800_000_000);
    assert_eq!((f.episodes_7d, f.resolved_7d, f.median_minutes), (0, 0, None));
}

#[test]
fn old_episodes_fall_out_of_the_window() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    s.insert_event(&alert(Status::Firing), t0);
    s.insert_event(&alert(Status::Resolved), t0 + MIN);
    let now = t0 + 8 * 24 * 3600;
    let seq = s.insert_event(&alert(Status::Firing), now);
    let f = facts(&s.events_for("id", now - 7 * 24 * 3600, seq), &alert(Status::Firing), now);
    assert_eq!(f.episodes_7d, 0);
}

#[test]
fn episode_pinged_sees_only_the_open_episode() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    let e1 = s.insert_event(&alert(Status::Firing), t0);
    s.insert_decision(&DecisionRow { event_seq: e1, decided_at: t0, mode: "gate".into(), kind: "ping".into(),
        probability: Some(0.9), reason: "".into(), delivered: true, backend_ms: Some(5), input_tokens: Some(10) });
    let e2 = s.insert_event(&alert(Status::Firing), t0 + MIN);
    assert!(s.episode_pinged("id", e2, 0));
    assert!(s.episode_pinged("id", e2, t0), "a ping decided exactly at the window start still counts");
    assert!(!s.episode_pinged("id", e2, t0 + 1), "a ping decided before the window no longer counts");
    s.insert_event(&alert(Status::Resolved), t0 + 2 * MIN);
    let e4 = s.insert_event(&alert(Status::Firing), t0 + 3 * MIN);
    assert!(!s.episode_pinged("id", e4, 0), "a new episode starts clean");
}

#[test]
fn store_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let p = path.to_str().unwrap();
    { let s = Store::open(p).unwrap(); s.insert_event(&alert(Status::Firing), 1); }
    let s = Store::open(p).unwrap();
    assert_eq!(s.events_since(0).len(), 1);
}

const DAY: i64 = 24 * 3600;

#[test]
fn episode_straddling_the_window_is_not_counted() {
    let s = Store::memory();
    let now = 1_800_000_000 + 10 * DAY;
    s.insert_event(&alert(Status::Firing), now - 10 * DAY);
    s.insert_event(&alert(Status::Firing), now - 3 * DAY);
    s.insert_event(&alert(Status::Resolved), now - 1 * DAY);
    let seq = s.insert_event(&alert(Status::Firing), now);
    let before = s.events_for("id", now - 7 * DAY, seq);
    let f = facts(&before, &alert(Status::Firing), now);
    assert_eq!(f.episodes_7d, 0);
    assert_eq!(f.resolved_7d, 0);
    assert_eq!(f.median_minutes, None);
}

#[test]
fn open_episode_keeps_its_true_start() {
    let s = Store::memory();
    let now = 1_800_000_000 + 10 * DAY;
    s.insert_event(&alert(Status::Firing), now - 8 * DAY);
    let seq = s.insert_event(&alert(Status::Firing), now);
    let before = s.events_for("id", now - 7 * DAY, seq);
    let f = facts(&before, &alert(Status::Firing), now);
    assert_eq!(f.minutes, 8 * 24 * 60);
    assert_eq!(f.episodes_7d, 0);
}

#[test]
fn even_count_median_is_lower_middle() {
    let s = Store::memory();
    let t0 = 1_800_000_000;
    for (start, dur) in [(0, 2), (10, 4), (20, 6), (30, 8)] {
        s.insert_event(&alert(Status::Firing), t0 + start * MIN);
        s.insert_event(&alert(Status::Resolved), t0 + (start + dur) * MIN);
    }
    let now = t0 + 100 * MIN;
    let seq = s.insert_event(&alert(Status::Firing), now);
    let before = s.events_for("id", now - 7 * DAY, seq);
    let f = facts(&before, &alert(Status::Firing), now);
    assert_eq!(f.median_minutes, Some(4));
}

proptest! {
    #[test]
    fn resolved_never_exceeds_episodes(ops in prop::collection::vec((any::<bool>(), 1i64..600), 0..60)) {
        let s = Store::memory();
        let mut t = 1_800_000_000;
        for (firing, gap) in &ops {
            t += gap;
            s.insert_event(&alert(if *firing { Status::Firing } else { Status::Resolved }), t);
        }
        t += 1;
        let seq = s.insert_event(&alert(Status::Firing), t);
        let f = facts(&s.events_for("id", t - 7 * 24 * 3600, seq), &alert(Status::Firing), t);
        prop_assert!(f.resolved_7d <= f.episodes_7d);
        prop_assert!(f.minutes >= 0);
        if let Some(m) = f.median_minutes { prop_assert!(m >= 0); }
    }
}
