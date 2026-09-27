//! Facts about an alert's past, computed in code so the model gets conclusions, not numbers to compare.
//!
//! An episode starts with a firing event when the identity is not already open and ends with the
//! next resolved event. Repeated firing events while open belong to the same episode. An episode
//! can straddle the 7-day window: `events_before` (via `Store::events_for`) may include an episode's
//! true first event even though it lies before the window, so that its real start is known.
//!
//! `episodes_7d` counts episodes whose start is on or after `current_at - WINDOW_SECS`, excluding
//! the current one (the episode, if any, still open when `current` arrives — there is at most one,
//! since episodes are sequential per identity). `resolved_7d` is how many of those ended. Both are
//! counted from the same in-window, ended-episode list, so an episode that started before the
//! window but ended inside it counts toward neither. `median_minutes` is the median duration of
//! those ended episodes, rounded down; for an even count, the lower of the two middle values.
//! `minutes` is, for the current event, minutes since the *true* start of its still-open episode
//! (0 if none is open, i.e. it opens now), even when that start lies outside the window.

use crate::mapping::{Alert, Status};
use crate::store::Event;

pub const WINDOW_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub env: String,
    pub firing: bool,
    pub minutes: i64,
    pub episodes_7d: u32,
    pub resolved_7d: u32,
    pub median_minutes: Option<i64>,
}

pub fn facts(events_before: &[Event], current: &Alert, current_at: i64) -> Facts {
    let window_start = current_at - WINDOW_SECS;
    let mut open_since: Option<i64> = None;
    // (start, duration_minutes) for each ended episode, regardless of window, filtered below.
    let mut ended: Vec<(i64, i64)> = Vec::new();
    for e in events_before {
        match (e.alert.status, open_since) {
            (Status::Firing, None) => {
                open_since = Some(e.received_at);
            }
            (Status::Firing, Some(_)) => {}
            (Status::Resolved, Some(start)) => {
                ended.push((start, (e.received_at - start).max(0) / 60));
                open_since = None;
            }
            (Status::Resolved, None) => {}
        }
    }
    let firing = current.status == Status::Firing;
    let minutes = match open_since {
        Some(start) => (current_at - start).max(0) / 60,
        None => 0,
    };
    let mut durations: Vec<i64> =
        ended.into_iter().filter(|(start, _)| *start >= window_start).map(|(_, d)| d).collect();
    let episodes_7d = u32::try_from(durations.len()).unwrap_or(u32::MAX);
    durations.sort_unstable();
    let median_minutes = if durations.is_empty() { None } else { Some(durations[(durations.len() - 1) / 2]) };
    Facts {
        env: current.env.clone(),
        firing,
        minutes,
        episodes_7d,
        // Only ended episodes are counted, so each of them resolved.
        resolved_7d: episodes_7d,
        median_minutes,
    }
}
