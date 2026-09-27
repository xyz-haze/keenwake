//! Observe-mode report and replay diff.

use crate::decide::{route, Kind};
use crate::history::{facts, Facts, WINDOW_SECS};
use crate::server::App;
use crate::state::sentence;
use crate::store::{Event, Store};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub const JEV_USD_PER_MTOK: f64 = 0.042;

/// A stored event whose decision `replay` would now make differently.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub seq: i64,
    pub summary: String,
    pub old: Kind,
    pub new: Kind,
}

#[derive(Debug, Serialize)]
pub struct ReportRow {
    pub summary: String,
    pub identity: String,
    pub kind: &'static str,
    pub probability: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub total: u64,
    pub by_kind: BTreeMap<&'static str, u64>,
    pub first_seen: u64,
    pub backend_errors: u64,
    pub median_backend_ms: Option<i64>,
    pub input_tokens: i64,
    pub est_cost_usd: f64,
    pub rows: Vec<ReportRow>,
    pub avoidable_pings: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("expected e.g. 7d, 12h, 30m")]
pub struct BadSince;

/// A `--since` duration (`7d`, `12h`, `30m`) in seconds.
pub fn parse_since(s: &str) -> Result<i64, BadSince> {
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = n.parse().map_err(|_| BadSince)?;
    Ok(n * match unit {
        "d" => 86_400,
        "h" => 3600,
        "m" => 60,
        _ => return Err(BadSince),
    })
}

/// Repeat suppression over simulated decisions: per identity, when the team was last notified
/// (ping or untriaged) in the still-open episode. The same time-bounded rule as
/// `Store::episode_pinged`, which only sees decisions `serve` actually delivered.
struct RepeatSim {
    window: i64,
    last: HashMap<String, i64>,
}

impl RepeatSim {
    fn new(window: i64) -> RepeatSim {
        RepeatSim { window, last: HashMap::new() }
    }

    /// A resolved event ends the episode.
    fn resolved(&mut self, identity: &str) {
        self.last.remove(identity);
    }

    /// True if a notification at `at` would repeat one sent within the window.
    fn suppresses(&self, identity: &str, at: i64) -> bool {
        self.last.get(identity).is_some_and(|&t| t >= at.saturating_sub(self.window))
    }

    fn notified(&mut self, identity: &str, at: i64) {
        self.last.insert(identity.to_string(), at);
    }
}

/// History facts for a stored event, as `serve` computed them when it arrived.
fn stored_facts(store: &Store, e: &Event) -> Facts {
    let before = store.events_for(&e.alert.identity, e.received_at - WINDOW_SECS, e.seq);
    facts(&before, &e.alert, e.received_at)
}

pub fn build(store: &Store, since: i64, price_per_mtok: f64, repeat_window: i64) -> Report {
    let rows = store.decisions_since(since);
    let mut by_kind = BTreeMap::new();
    let mut ms: Vec<i64> = Vec::new();
    let (mut tokens, mut errors, mut first_seen, mut avoidable) = (0i64, 0u64, 0u64, 0u64);
    // Simulated from the STORED kinds (not replayed) in seq order, with the same window as `serve`.
    let mut sim = RepeatSim::new(repeat_window);
    let mut out = Vec::new();
    for (e, d) in &rows {
        *by_kind.entry(d.kind.as_str()).or_insert(0) += 1;
        if d.kind == Kind::Untriaged {
            errors += 1;
        }
        if let Some(m) = d.backend_ms {
            ms.push(m);
        }
        tokens += d.input_tokens.unwrap_or(0);
        if d.kind != Kind::Resolved && stored_facts(store, e).episodes_7d == 0 {
            first_seen += 1;
        }
        let id = &e.alert.identity;
        match d.kind {
            Kind::Resolved => sim.resolved(id),
            Kind::Digest | Kind::Escalate => avoidable += 1,
            Kind::Ping | Kind::Untriaged => {
                if sim.suppresses(id, e.received_at) {
                    avoidable += 1;
                } else {
                    sim.notified(id, e.received_at);
                }
            }
            Kind::Repeat => {}
        }
        out.push(ReportRow {
            summary: e.alert.summary.clone(),
            identity: e.alert.identity.clone(),
            kind: d.kind.as_str(),
            probability: d.probability,
        });
    }
    ms.sort_unstable();
    Report {
        total: rows.len() as u64,
        by_kind,
        first_seen,
        backend_errors: errors,
        median_backend_ms: ms.get(ms.len().saturating_sub(1) / 2).copied().filter(|_| !ms.is_empty()),
        input_tokens: tokens,
        est_cost_usd: tokens as f64 * price_per_mtok / 1e6,
        rows: out,
        avoidable_pings: avoidable,
    }
}

/// The human-readable report printed by `keenwake report`.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "alerts decided: {}", self.total)?;
        for (k, v) in &self.by_kind {
            writeln!(f, "  {k:<10} {v}")?;
        }
        writeln!(f, "first seen in 7 days (decided without history): {}", self.first_seen)?;
        writeln!(f, "avoidable pings (simulated gate): {}", self.avoidable_pings)?;
        writeln!(f, "backend errors: {}", self.backend_errors)?;
        if let Some(m) = self.median_backend_ms {
            writeln!(f, "median backend latency: {m} ms")?;
        }
        writeln!(f, "input tokens: {} (about ${:.4} at Jev list price)", self.input_tokens, self.est_cost_usd)
    }
}

/// Re-decides stored events with the current config and backend. This replays **decisions**,
/// not mapping: a change to a source's field extraction does not apply to already-stored events.
/// Repeat suppression (an episode's second ping within `decision.repeat_window_hours`) is
/// simulated purely from the kinds this replay itself produces, per identity, in seq order, using
/// event times — never from `Store::episode_pinged`, whose `delivered` flag observe-mode history
/// (the usual source for a replay) never sets.
pub async fn replay(app: &App, since: i64) -> Vec<Change> {
    let mut changed = Vec::new();
    let mut sim = RepeatSim::new(app.cfg.decision.repeat_window_secs());
    for (e, old) in app.store.decisions_since(since) {
        let id = &e.alert.identity;
        if old.kind == Kind::Resolved {
            sim.resolved(id);
            continue;
        }
        let state = sentence(&e.alert, &stored_facts(&app.store, &e));
        let p = app.backend.ask(&state, &app.cfg.question).await.ok().map(|a| a.probability);
        let new = route(&app.cfg.decision, p, sim.suppresses(id, e.received_at)).kind;
        if matches!(new, Kind::Ping | Kind::Untriaged) {
            sim.notified(id, e.received_at);
        }
        if new != old.kind {
            changed.push(Change { seq: e.seq, summary: e.alert.summary, old: old.kind, new });
        }
    }
    changed
}
