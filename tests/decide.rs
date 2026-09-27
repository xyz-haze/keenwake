use keenwake::config::{DecisionCfg, Mode, OnError};
use keenwake::decide::{classify, repeat_floor, resolved_target, route, Kind, Sent, Target};
use proptest::prelude::*;

fn cfg(mode: Mode, on_error: OnError) -> DecisionCfg {
    DecisionCfg {
        mode,
        on_error,
        ping: 0.55,
        digest: 0.30,
        digest_at: "08:00".parse().unwrap(),
        repeat_window_hours: 24,
    }
}

#[test]
fn classify_uses_both_thresholds() {
    assert_eq!(classify(0.55, 0.55, 0.30), Kind::Ping);
    assert_eq!(classify(0.54, 0.55, 0.30), Kind::Escalate);
    assert_eq!(classify(0.30, 0.55, 0.30), Kind::Escalate);
    assert_eq!(classify(0.29, 0.55, 0.30), Kind::Digest);
}

#[test]
fn gate_routes_each_kind() {
    let c = cfg(Mode::Gate, OnError::Ping);
    assert_eq!(route(&c, Some(0.9), None).target, Target::Ping);
    assert_eq!(route(&c, Some(0.4), None).target, Target::Escalate);
    assert_eq!(route(&c, Some(0.1), None).target, Target::DigestQueue);
    let e = route(&c, None, None);
    assert_eq!((e.kind, e.target), (Kind::Untriaged, Target::Ping));
    let e = route(&cfg(Mode::Gate, OnError::Drop), None, None);
    assert_eq!((e.kind, e.target), (Kind::Untriaged, Target::Nothing));
}

#[test]
fn gate_does_not_ping_twice_in_one_episode() {
    let c = cfg(Mode::Gate, OnError::Ping);
    let r = route(&c, Some(0.9), Some(Kind::Ping));
    assert_eq!((r.kind, r.target), (Kind::Repeat, Target::Nothing));
    let r = route(&c, None, Some(Kind::Untriaged));
    assert_eq!((r.kind, r.target), (Kind::Repeat, Target::Nothing));
    let r = route(&cfg(Mode::Gate, OnError::Drop), None, Some(Kind::Ping));
    assert_eq!((r.kind, r.target), (Kind::Repeat, Target::Nothing));
}

#[test]
fn only_a_strictly_more_urgent_decision_gets_through() {
    let c = cfg(Mode::Gate, OnError::Ping);
    let kind = |p: Option<f64>, floor| route(&c, p, Some(floor)).kind;
    assert_eq!(kind(Some(0.4), Kind::Ping), Kind::Repeat);
    assert_eq!(kind(Some(0.1), Kind::Ping), Kind::Repeat);
    assert_eq!(kind(Some(0.4), Kind::Escalate), Kind::Repeat);
    assert_eq!(kind(Some(0.1), Kind::Escalate), Kind::Repeat);
    assert_eq!(kind(Some(0.1), Kind::Digest), Kind::Repeat);
    assert_eq!(kind(Some(0.9), Kind::Escalate), Kind::Ping);
    assert_eq!(kind(None, Kind::Escalate), Kind::Untriaged);
    assert_eq!(kind(Some(0.4), Kind::Digest), Kind::Escalate);
    assert_eq!(route(&c, None, Some(Kind::Escalate)).target, Target::Ping, "when in doubt, ping");
}

#[test]
fn repeat_floor_is_the_most_urgent_notification_still_counting() {
    const W: i64 = 100;
    let s = |at, kind| Sent { at, kind };
    assert_eq!(repeat_floor(&[], 1000, W), None);
    assert_eq!(repeat_floor(&[s(900, Kind::Ping)], 1000, W), Some(Kind::Ping), "exactly at the window start");
    assert_eq!(repeat_floor(&[s(899, Kind::Ping)], 1000, W), None, "a ping before the window no longer counts");
    assert_eq!(repeat_floor(&[s(899, Kind::Escalate)], 1000, W), None);
    assert_eq!(repeat_floor(&[s(0, Kind::Digest)], 1000, W), Some(Kind::Digest), "a digest counts all episode");
    let mixed = [s(0, Kind::Digest), s(950, Kind::Escalate), s(800, Kind::Ping)];
    assert_eq!(repeat_floor(&mixed, 1000, W), Some(Kind::Escalate));
    assert_eq!(repeat_floor(&[s(950, Kind::Untriaged), s(960, Kind::Escalate)], 1000, W), Some(Kind::Untriaged));
}

#[test]
fn resolved_goes_where_the_episode_was_notified() {
    let g = cfg(Mode::Gate, OnError::Ping);
    assert_eq!(resolved_target(&g, Some(Kind::Ping)), Target::Ping);
    assert_eq!(resolved_target(&g, Some(Kind::Untriaged)), Target::Ping);
    assert_eq!(resolved_target(&g, Some(Kind::Escalate)), Target::Escalate);
    assert_eq!(resolved_target(&g, Some(Kind::Digest)), Target::Nothing);
    assert_eq!(resolved_target(&g, None), Target::Nothing);
    assert_eq!(resolved_target(&cfg(Mode::Observe, OnError::Ping), Some(Kind::Ping)), Target::Nothing);
}

proptest! {
    #[test]
    fn observe_never_suppresses_or_sends_to_team_outputs(p in 0.0f64..=1.0, err in any::<bool>(), floor in prop::option::of(prop::sample::select(vec![Kind::Ping, Kind::Escalate, Kind::Digest])), drop in any::<bool>()) {
        let c = cfg(Mode::Observe, if drop { OnError::Drop } else { OnError::Ping });
        let outcome = if err { None } else { Some(p) };
        prop_assert_eq!(route(&c, outcome, floor).target, Target::Verdict);
    }

    #[test]
    fn gate_fail_open_always_pings(digest in 0.0f64..=1.0, ping in 0.0f64..=1.0) {
        let mut c = cfg(Mode::Gate, OnError::Ping);
        (c.digest, c.ping) = if digest <= ping { (digest, ping) } else { (ping, digest) };
        let r = route(&c, None, None);
        prop_assert_eq!(r.target, Target::Ping);
    }

    #[test]
    fn higher_probability_is_never_less_urgent(a in 0.0f64..=1.0, b in 0.0f64..=1.0, digest in 0.0f64..=1.0, ping in 0.0f64..=1.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let (d, p) = if digest <= ping { (digest, ping) } else { (ping, digest) };
        prop_assert!(classify(lo, p, d).urgency() <= classify(hi, p, d).urgency());
    }
}
