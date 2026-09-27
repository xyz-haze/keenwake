//! Observe-mode report and replay diff.

use crate::decide::Kind;
use crate::history::{facts, WINDOW_SECS};
use crate::pipeline::{finish, Prepared};
use crate::server::App;
use crate::state::sentence;
use crate::store::Store;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

pub const JEV_USD_PER_MTOK: f64 = 0.042;

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

pub fn parse_since(s: &str) -> anyhow::Result<i64> {
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = n.parse().map_err(|_| anyhow::anyhow!("--since expects e.g. 7d, 12h, 30m"))?;
    Ok(n * match unit {
        "d" => 86_400,
        "h" => 3600,
        "m" => 60,
        _ => anyhow::bail!("--since expects e.g. 7d, 12h, 30m"),
    })
}

/// True if a ping decided at `last` still suppresses a new one for an event at `at`: the same
/// time-bounded rule as `Store::episode_pinged`, applied to simulated decisions.
fn within(last: Option<i64>, at: i64, repeat_window: i64) -> bool {
    last.is_some_and(|t| t >= at.saturating_sub(repeat_window))
}

pub fn build(store: &Store, since: i64, price_per_mtok: f64, repeat_window: i64) -> Report {
    let rows = store.decisions_since(since);
    let mut by_kind = BTreeMap::new();
    let mut ms: Vec<i64> = Vec::new();
    let (mut tokens, mut errors, mut first_seen, mut avoidable) = (0i64, 0u64, 0u64, 0u64);
    // Per identity: when a gate would last have pinged for the still-open episode, decided from
    // the STORED kinds (not replayed) in seq order, with the same repeat window as `serve`.
    let mut pinged: HashMap<String, Option<i64>> = HashMap::new();
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
        let before = store.events_for(&e.alert.identity, e.received_at - WINDOW_SECS, e.seq);
        if d.kind != Kind::Resolved && facts(&before, &e.alert, e.received_at).episodes_7d == 0 {
            first_seen += 1;
        }
        let last = pinged.entry(e.alert.identity.clone()).or_insert(None);
        match d.kind {
            Kind::Resolved => *last = None,
            Kind::Digest | Kind::Escalate => avoidable += 1,
            Kind::Ping | Kind::Untriaged => {
                if within(*last, e.received_at, repeat_window) {
                    avoidable += 1;
                } else {
                    *last = Some(e.received_at);
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

impl Report {
    pub fn to_text(&self) -> String {
        let mut s = format!("alerts decided: {}\n", self.total);
        for (k, v) in &self.by_kind {
            s.push_str(&format!("  {k:<10} {v}\n"));
        }
        s.push_str(&format!("first seen in 7 days (decided without history): {}\n", self.first_seen));
        s.push_str(&format!("avoidable pings (simulated gate): {}\n", self.avoidable_pings));
        s.push_str(&format!("backend errors: {}\n", self.backend_errors));
        if let Some(m) = self.median_backend_ms {
            s.push_str(&format!("median backend latency: {m} ms\n"));
        }
        s.push_str(&format!(
            "input tokens: {} (about ${:.4} at Jev list price)\n",
            self.input_tokens, self.est_cost_usd
        ));
        s
    }
}

/// Re-decides stored events with the current config and backend. This replays **decisions**,
/// not mapping: a change to a source's field extraction does not apply to already-stored events.
/// Repeat suppression (an episode's second ping within `decision.repeat_window_hours`) is
/// simulated purely from the kinds this replay itself produces, per identity, in seq order, using
/// event times — never from `Store::episode_pinged`, whose `delivered` flag observe-mode history
/// (the usual source for a replay) never sets.
pub async fn replay(app: &App, since: i64) -> Vec<(i64, String, String, String)> {
    let mut changed = Vec::new();
    let window = app.cfg.decision.repeat_window_secs();
    let mut pinged: HashMap<String, Option<i64>> = HashMap::new();
    for (e, old) in app.store.decisions_since(since) {
        let last = pinged.entry(e.alert.identity.clone()).or_insert(None);
        if old.kind == Kind::Resolved {
            *last = None;
            continue;
        }
        let before = app.store.events_for(&e.alert.identity, e.received_at - WINDOW_SECS, e.seq);
        let f = facts(&before, &e.alert, e.received_at);
        let p = Prepared {
            event_seq: e.seq,
            alert: e.alert.clone(),
            state: sentence(&e.alert, &f),
            facts: f,
            already_pinged: within(*last, e.received_at, window),
            needs_model: true,
        };
        let outcome =
            app.backend.ask(&p.state, &app.cfg.question).await.map(|a| a.probability).map_err(|e| e.to_string());
        let new_kind = finish(&app.cfg, &p, outcome).kind;
        if matches!(new_kind, Kind::Ping | Kind::Untriaged) {
            *last = Some(e.received_at);
        }
        if new_kind != old.kind {
            changed.push((
                e.seq,
                e.alert.summary.clone(),
                old.kind.as_str().to_string(),
                new_kind.as_str().to_string(),
            ));
        }
    }
    changed
}
