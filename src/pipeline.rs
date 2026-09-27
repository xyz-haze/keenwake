//! The first half of `serve`'s decision path: store the alert, then compute its history and the
//! state sent to the model. `decide::route` is the second half.

use crate::decide::{repeat_floor, Kind};
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
    /// `decide::repeat_floor` of the open episode, as `route` and `resolved_target` take it.
    pub floor: Option<Kind>,
    pub needs_model: bool,
}

/// `repeat_window` (seconds): how far back a delivered notification still counts as sent.
pub fn prepare(store: &Store, redactor: &Redactor, mut alert: Alert, now: i64, repeat_window: i64) -> Prepared {
    alert.summary = redactor.clean(&alert.summary);
    alert.details = redactor.clean(&alert.details);
    let event_seq = store.insert_event(&alert, now);
    let before = store.events_for(&alert.identity, now - WINDOW_SECS, event_seq);
    let f = facts(&before, &alert, now);
    let state = sentence(&alert, &f);
    let since = now.saturating_sub(repeat_window);
    let floor = repeat_floor(&store.episode_sent(&alert.identity, event_seq, since), now, repeat_window);
    let needs_model = alert.status == Status::Firing;
    Prepared { event_seq, alert, facts: f, state, floor, needs_model }
}

pub fn facts_line(f: &Facts) -> String {
    match f.median_minutes {
        None => "first time in 7 days".into(),
        Some(m) => format!("{} times in 7 days, median {m} min", f.episodes_7d),
    }
}
