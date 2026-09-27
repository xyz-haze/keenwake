//! Probability + thresholds + mode -> what happens. Pure: no I/O, no clock.

use crate::config::{DecisionCfg, Mode, OnError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ping,
    Escalate,
    Digest,
    Untriaged,
    Repeat,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ping => "ping",
            Kind::Escalate => "escalate",
            Kind::Digest => "digest",
            Kind::Untriaged => "untriaged",
            Kind::Repeat => "repeat",
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
        Kind::Untriaged | Kind::Repeat => Target::Nothing,
    };
    Routing { kind, target }
}
