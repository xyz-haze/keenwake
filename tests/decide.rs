use alertsift::config::{DecisionCfg, Mode, OnError};
use alertsift::decide::{classify, route, Kind, Target};
use proptest::prelude::*;

fn cfg(mode: Mode, on_error: OnError) -> DecisionCfg {
    DecisionCfg { mode, on_error, ping: 0.55, digest: 0.30, digest_at: "08:00".into() }
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
    assert_eq!(route(&c, Ok(0.9), false).target, Target::Ping);
    assert_eq!(route(&c, Ok(0.4), false).target, Target::Escalate);
    assert_eq!(route(&c, Ok(0.1), false).target, Target::DigestQueue);
    let e = route(&c, Err("timeout".into()), false);
    assert_eq!((e.kind, e.target), (Kind::Untriaged, Target::Ping));
    let e = route(&cfg(Mode::Gate, OnError::Drop), Err("timeout".into()), false);
    assert_eq!((e.kind, e.target), (Kind::Untriaged, Target::Nothing));
}

#[test]
fn gate_does_not_ping_twice_in_one_episode() {
    let c = cfg(Mode::Gate, OnError::Ping);
    let r = route(&c, Ok(0.9), true);
    assert_eq!((r.kind, r.target), (Kind::Repeat, Target::Nothing));
    let r = route(&c, Err("x".into()), true);
    assert_eq!((r.kind, r.target), (Kind::Repeat, Target::Nothing));
    assert_eq!(route(&c, Ok(0.4), true).target, Target::Escalate, "escalation still flows");
}

proptest! {
    #[test]
    fn observe_never_suppresses_or_sends_to_team_outputs(p in 0.0f64..=1.0, err in any::<bool>(), pinged in any::<bool>(), drop in any::<bool>()) {
        let c = cfg(Mode::Observe, if drop { OnError::Drop } else { OnError::Ping });
        let outcome = if err { Err("e".to_string()) } else { Ok(p) };
        prop_assert_eq!(route(&c, outcome, pinged).target, Target::Verdict);
    }

    #[test]
    fn gate_fail_open_always_pings(msg in ".{0,20}") {
        let r = route(&cfg(Mode::Gate, OnError::Ping), Err(msg), false);
        prop_assert_eq!(r.target, Target::Ping);
    }

    #[test]
    fn higher_probability_is_never_less_urgent(a in 0.0f64..=1.0, b in 0.0f64..=1.0, digest in 0.0f64..=1.0, ping in 0.0f64..=1.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let (d, p) = if digest <= ping { (digest, ping) } else { (ping, digest) };
        prop_assert!(classify(lo, p, d).urgency() <= classify(hi, p, d).urgency());
    }
}
