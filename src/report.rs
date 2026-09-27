//! Observe-mode report and replay diff.

use crate::config::Mode;
use crate::decide::{repeat_floor, route, Kind, Sent, Target};
use crate::history::{facts, Facts, WINDOW_SECS};
use crate::server::App;
use crate::state::sentence;
use crate::store::{DecisionRow, Event, Store};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

pub const JEV_USD_PER_MTOK: f64 = 0.042;

/// How many decisions the text report lists; `--json` has them all.
const LISTED_ROWS: usize = 30;

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
    /// When the alert arrived. Text output only: `--json` rows keep their original fields.
    #[serde(skip)]
    pub received_at: i64,
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

/// A `--since` duration (`7d`, `12h`, `30m`) in seconds: ASCII digits, then one unit.
pub fn parse_since(s: &str) -> Result<i64, BadSince> {
    let unit: i64 = match s.chars().last() {
        Some('d') => 86_400,
        Some('h') => 3600,
        Some('m') => 60,
        _ => return Err(BadSince),
    };
    // The unit is one ASCII byte, so this slice is on a char boundary.
    let n = &s[..s.len() - 1];
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BadSince);
    }
    n.parse::<i64>().ok().and_then(|n| n.checked_mul(unit)).ok_or(BadSince)
}

/// Repeat suppression over simulated decisions: per identity, the notifications sent in the
/// still-open episode, fed to the same `decide::repeat_floor` rule `serve` applies to what
/// `Store::episode_sent` returns.
struct RepeatSim {
    window: i64,
    sent: HashMap<String, Vec<Sent>>,
    seeded: HashSet<String>,
}

impl RepeatSim {
    fn new(window: i64) -> RepeatSim {
        RepeatSim { window, sent: HashMap::new(), seeded: HashSet::new() }
    }

    /// On an identity's first event in the replayed range, loads what its open episode had
    /// already sent before that range, so a `--since` shorter than the window still sees it.
    fn seed(&mut self, store: &Store, e: &Event) {
        let id = &e.alert.identity;
        if self.seeded.insert(id.clone()) {
            let before = store.episode_sent(id, e.seq, e.received_at.saturating_sub(self.window));
            self.sent.insert(id.clone(), before);
        }
    }

    /// A resolved event ends the episode.
    fn resolved(&mut self, identity: &str) {
        self.sent.remove(identity);
    }

    /// `repeat_floor` for a decision about `identity` at `at`.
    fn floor(&self, identity: &str, at: i64) -> Option<Kind> {
        repeat_floor(self.sent.get(identity).map_or(&[], Vec::as_slice), at, self.window)
    }

    fn notified(&mut self, identity: &str, at: i64, kind: Kind) {
        self.sent.entry(identity.to_string()).or_default().push(Sent { at, kind });
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
                if sim.floor(id, e.received_at).is_some_and(|f| f.urgency() >= d.kind.urgency()) {
                    avoidable += 1;
                } else {
                    sim.notified(id, e.received_at, d.kind);
                }
            }
            Kind::Repeat => {}
        }
        out.push(ReportRow {
            received_at: e.received_at,
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

/// `ts` as `YYYY-MM-DD HH:MM UTC`. Days to civil date after Howard Hinnant's `civil_from_days`,
/// to avoid a date crate for one line of output.
fn utc_minute(ts: i64) -> String {
    let (days, secs) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
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
        writeln!(f, "input tokens: {} (about ${:.4} at Jev list price)", self.input_tokens, self.est_cost_usd)?;
        // The counts do not say which alerts were held back: list the latest decisions, oldest first.
        if self.rows.is_empty() {
            return Ok(());
        }
        writeln!(f, "decisions, most recent last:")?;
        let hidden = self.rows.len().saturating_sub(LISTED_ROWS);
        if hidden > 0 {
            writeln!(f, "  ... {hidden} more, use --json")?;
        }
        for r in &self.rows[hidden..] {
            let p = r.probability.map_or("p=-   ".to_string(), |p| format!("p={p:.2}"));
            writeln!(f, "  {}  {:<9}  {p}  {}", utc_minute(r.received_at), r.kind, r.summary)?;
        }
        Ok(())
    }
}

/// Whether a replayed decision counts as sent for repeat suppression. The same decision `serve`
/// made in gate is taken as it really went (a failed delivery was not sent); anything else, such
/// as observe-mode history replayed with a gate config, is assumed delivered if it has a target.
fn replay_sent(old: &DecisionRow, new_kind: Kind, target: Target) -> bool {
    if old.mode == Mode::Gate && old.kind == new_kind {
        return old.delivered;
    }
    matches!(target, Target::Ping | Target::Escalate | Target::DigestQueue)
}

/// Re-decides stored events with the current config and backend. This replays **decisions**,
/// not mapping: a change to a source's field extraction does not apply to already-stored events.
/// Repeat suppression (a notification no more urgent than one already sent in the episode) is
/// simulated per identity, in seq order, using event times, from what was sent before `since`
/// (`Store::episode_sent`) and then from the replayed decisions (`replay_sent`).
pub async fn replay(app: &App, since: i64) -> Vec<Change> {
    let mut changed = Vec::new();
    let mut sim = RepeatSim::new(app.cfg.decision.repeat_window_secs());
    for (e, old) in app.store.decisions_since(since) {
        let id = &e.alert.identity;
        sim.seed(&app.store, &e);
        if old.kind == Kind::Resolved {
            sim.resolved(id);
            continue;
        }
        let state = sentence(&e.alert, &stored_facts(&app.store, &e));
        let p = app.backend.ask(&state, &app.cfg.question).await.ok().map(|a| a.probability);
        let r = route(&app.cfg.decision, p, sim.floor(id, e.received_at));
        if replay_sent(&old, r.kind, r.target) {
            sim.notified(id, e.received_at, r.kind);
        }
        let new = r.kind;
        if new != old.kind {
            changed.push(Change { seq: e.seq, summary: e.alert.summary, old: old.kind, new });
        }
    }
    changed
}
