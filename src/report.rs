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
pub struct ReportRow { pub summary: String, pub identity: String, pub kind: String, pub probability: Option<f64> }

#[derive(Debug, Serialize)]
pub struct Report {
    pub total: u64,
    pub by_kind: BTreeMap<String, u64>,
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

pub fn build(store: &Store, since: i64, price_per_mtok: f64) -> Report {
    let rows = store.decisions_since(since);
    let mut by_kind = BTreeMap::new();
    let mut ms: Vec<i64> = Vec::new();
    let (mut tokens, mut errors, mut first_seen, mut avoidable) = (0i64, 0u64, 0u64, 0u64);
    // Per identity: whether a gate would already have pinged for the still-open episode, decided
    // from the STORED kinds (not replayed) in seq order — see `avoidable_pings`'s doc comment.
    let mut pinged: HashMap<String, bool> = HashMap::new();
    let mut out = Vec::new();
    for (e, d) in &rows {
        *by_kind.entry(d.kind.clone()).or_insert(0) += 1;
        if d.kind == "untriaged" { errors += 1; }
        if let Some(m) = d.backend_ms { ms.push(m); }
        tokens += d.input_tokens.unwrap_or(0);
        let before = store.events_for(&e.alert.identity, e.received_at - WINDOW_SECS, e.seq);
        if d.kind != "resolved" && facts(&before, &e.alert, e.received_at).episodes_7d == 0 { first_seen += 1; }
        let flag = pinged.entry(e.alert.identity.clone()).or_insert(false);
        match d.kind.as_str() {
            "resolved" => *flag = false,
            "digest" | "escalate" => avoidable += 1,
            "ping" | "untriaged" => {
                if *flag { avoidable += 1; } else { *flag = true; }
            }
            _ => {}
        }
        out.push(ReportRow { summary: e.alert.summary.clone(), identity: e.alert.identity.clone(), kind: d.kind.clone(), probability: d.probability });
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
        for (k, v) in &self.by_kind { s.push_str(&format!("  {k:<10} {v}\n")); }
        s.push_str(&format!("first seen in 7 days (decided without history): {}\n", self.first_seen));
        s.push_str(&format!("avoidable pings (simulated gate): {}\n", self.avoidable_pings));
        s.push_str(&format!("backend errors: {}\n", self.backend_errors));
        if let Some(m) = self.median_backend_ms { s.push_str(&format!("median backend latency: {m} ms\n")); }
        s.push_str(&format!("input tokens: {} (about ${:.4} at Jev list price)\n", self.input_tokens, self.est_cost_usd));
        s
    }
}

/// Re-decides stored events with the current config and backend. This replays **decisions**,
/// not mapping: a change to a source's field extraction does not apply to already-stored events.
/// Repeat suppression (an episode's second ping) is simulated purely from the kinds this replay
/// itself produces, per identity, in seq order — never from `Store::episode_pinged`, whose
/// `delivered` flag observe-mode history (the usual source for a replay) never sets.
pub async fn replay(app: &App, since: i64) -> Vec<(i64, String, String, String)> {
    let mut changed = Vec::new();
    let mut pinged: HashMap<String, bool> = HashMap::new();
    for (e, old) in app.store.decisions_since(since) {
        let flag = pinged.entry(e.alert.identity.clone()).or_insert(false);
        if old.kind == "resolved" {
            *flag = false;
            continue;
        }
        let before = app.store.events_for(&e.alert.identity, e.received_at - WINDOW_SECS, e.seq);
        let f = facts(&before, &e.alert, e.received_at);
        let p = Prepared {
            event_seq: e.seq,
            alert: e.alert.clone(),
            state: sentence(&e.alert, &f),
            facts: f,
            already_pinged: *flag,
            needs_model: true,
        };
        let outcome = app.backend.ask(&p.state, &app.cfg.question).await.map(|a| a.probability).map_err(|e| e.to_string());
        let new_kind = finish(&app.cfg, &p, outcome).kind;
        if matches!(new_kind, Kind::Ping | Kind::Untriaged) { *flag = true; }
        let new = new_kind.as_str().to_string();
        if new != old.kind { changed.push((e.seq, e.alert.summary.clone(), old.kind.clone(), new)); }
    }
    changed
}
