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

pub fn route(d: &DecisionCfg, outcome: Result<f64, String>, already_pinged: bool) -> Routing {
    let kind = match outcome {
        Ok(p) => classify(p, d.ping, d.digest),
        Err(_) => Kind::Untriaged,
    };
    if d.mode == Mode::Observe {
        return Routing { kind, target: Target::Verdict };
    }
    let wants_ping = kind == Kind::Ping || (kind == Kind::Untriaged && d.on_error == OnError::Ping);
    if already_pinged && (wants_ping || kind == Kind::Untriaged) {
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
