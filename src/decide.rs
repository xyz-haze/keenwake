//! Probability + thresholds + mode -> what happens. Pure: no I/O, no clock.

use crate::config::{DecisionCfg, Mode, OnError};
use crate::UnknownValue;
use std::str::FromStr;

/// What was decided for one event. `Resolved` is recorded for a resolved alert, which skips the
/// model; `route` never returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ping,
    Escalate,
    Digest,
    Untriaged,
    Repeat,
    Resolved,
}

impl Kind {
    /// The name stored in `decisions.kind` and sent in outgoing messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ping => "ping",
            Kind::Escalate => "escalate",
            Kind::Digest => "digest",
            Kind::Untriaged => "untriaged",
            Kind::Repeat => "repeat",
            Kind::Resolved => "resolved",
        }
    }
    /// Ranks what `classify` returns, least urgent first. An untriaged decision ranks with a ping:
    /// when in doubt, ping.
    pub fn urgency(self) -> u8 {
        match self {
            Kind::Digest => 0,
            Kind::Escalate => 1,
            _ => 2,
        }
    }
}

impl FromStr for Kind {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Kind, UnknownValue> {
        Ok(match s {
            "ping" => Kind::Ping,
            "escalate" => Kind::Escalate,
            "digest" => Kind::Digest,
            "untriaged" => Kind::Untriaged,
            "repeat" => Kind::Repeat,
            "resolved" => Kind::Resolved,
            _ => return Err(UnknownValue(s.to_string())),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Ping,
    Escalate,
    DigestQueue,
    Verdict,
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Routing {
    pub kind: Kind,
    pub target: Target,
}

pub fn classify(p: f64, ping: f64, digest: f64) -> Kind {
    if p >= ping {
        Kind::Ping
    } else if p >= digest {
        Kind::Escalate
    } else {
        Kind::Digest
    }
}

/// A notification the team already got in the open episode: a delivered ping, untriaged or
/// escalate, or a digest entry queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub at: i64,
    pub kind: Kind,
}

/// The most urgent notification of the open episode that still counts at `now`, if any. A ping,
/// untriaged or escalate counts for `window` seconds, so a lost `resolved` cannot silence an
/// identity forever; a digest entry counts for the whole episode, so it is queued only once.
pub fn repeat_floor(sent: &[Sent], now: i64, window: i64) -> Option<Kind> {
    sent.iter()
        .filter(|s| s.kind == Kind::Digest || s.at >= now.saturating_sub(window))
        .map(|s| s.kind)
        .max_by_key(|k| k.urgency())
}

/// `probability` is `None` when the backend gave no usable answer. `floor` is `repeat_floor` of
/// the episode: in gate, a decision no more urgent than it becomes a `Repeat`, sent nowhere.
pub fn route(d: &DecisionCfg, probability: Option<f64>, floor: Option<Kind>) -> Routing {
    let kind = match probability {
        Some(p) => classify(p, d.ping, d.digest),
        None => Kind::Untriaged,
    };
    if d.mode == Mode::Observe {
        return Routing { kind, target: Target::Verdict };
    }
    if floor.is_some_and(|f| kind.urgency() <= f.urgency()) {
        return Routing { kind: Kind::Repeat, target: Target::Nothing };
    }
    let target = match kind {
        Kind::Ping => Target::Ping,
        Kind::Escalate => Target::Escalate,
        Kind::Digest => Target::DigestQueue,
        Kind::Untriaged if d.on_error == OnError::Ping => Target::Ping,
        Kind::Untriaged | Kind::Repeat | Kind::Resolved => Target::Nothing,
    };
    Routing { kind, target }
}

/// Where a `resolved` event goes: in gate, to the output that carried the episode's most urgent
/// notification still within the window. A digest-only episode gets no resolved message.
pub fn resolved_target(d: &DecisionCfg, floor: Option<Kind>) -> Target {
    match (d.mode, floor) {
        (Mode::Gate, Some(Kind::Ping | Kind::Untriaged)) => Target::Ping,
        (Mode::Gate, Some(Kind::Escalate)) => Target::Escalate,
        _ => Target::Nothing,
    }
}
