//! The decision path shared by `serve` and `replay`: prepare (store, history, state), then finish (route).

use crate::config::Config;
use crate::decide::{route, Routing};
use crate::history::{facts, Facts, WINDOW_SECS};
use crate::mapping::{Alert, Status};
use crate::redact::Redactor;
use crate::state::sentence;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct Prepared {
    pub event_seq: i64,
    pub alert: Alert,
    pub facts: Facts,
    pub state: String,
    pub already_pinged: bool,
    pub needs_model: bool,
}

/// `repeat_window` (seconds): how far back a delivered ping still counts as "already pinged".
pub fn prepare(store: &Store, redactor: &Redactor, mut alert: Alert, now: i64, repeat_window: i64) -> Prepared {
    alert.summary = redactor.clean(&alert.summary);
    alert.details = redactor.clean(&alert.details);
    let event_seq = store.insert_event(&alert, now);
    let before = store.events_for(&alert.identity, now - WINDOW_SECS, event_seq);
    let f = facts(&before, &alert, now);
    let state = sentence(&alert, &f);
    let already_pinged = store.episode_pinged(&alert.identity, event_seq, now.saturating_sub(repeat_window));
    let needs_model = alert.status == Status::Firing;
    Prepared { event_seq, alert, facts: f, state, already_pinged, needs_model }
}

pub fn finish(cfg: &Config, p: &Prepared, outcome: Result<f64, String>) -> Routing {
    route(&cfg.decision, outcome, p.already_pinged)
}

pub fn facts_line(f: &Facts) -> String {
    match (f.episodes_7d, f.median_minutes) {
        (0, _) => "first time in 7 days".into(),
        (n, Some(m)) => format!("{n} times in 7 days, {} resolved, median {m} min", f.resolved_7d),
        (n, None) => format!("{n} times in 7 days, never resolved"),
    }
}
