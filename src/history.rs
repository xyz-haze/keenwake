//! Facts about an alert's past, computed in code so the model gets conclusions, not numbers to compare.
//!
//! An episode starts with a firing event when the identity is not already open and ends with the
//! next resolved event. Repeated firing events while open belong to the same episode.
//! `episodes_7d` counts episodes that started in the 7 days before `current_at`, excluding the
//! current one. `resolved_7d` is how many of those ended. `median_minutes` is the median duration
//! of those ended episodes, rounded down. `minutes` is, for the current firing event, minutes
//! since the start of its open episode (0 if it opens now).

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
    let mut open_since: Option<i64> = None;
    let mut durations: Vec<i64> = Vec::new();
    let mut episodes: u32 = 0;
    for e in events_before {
        match (e.alert.status, open_since) {
            (Status::Firing, None) => { open_since = Some(e.received_at); episodes += 1; }
            (Status::Firing, Some(_)) => {}
            (Status::Resolved, Some(start)) => { durations.push((e.received_at - start).max(0) / 60); open_since = None; }
            (Status::Resolved, None) => {}
        }
    }
    let firing = current.status == Status::Firing;
    let (minutes, episodes_7d) = match (firing, open_since) {
        (true, Some(start)) => ((current_at - start).max(0) / 60, episodes - 1),
        (false, Some(start)) => ((current_at - start).max(0) / 60, episodes - 1),
        (_, None) => (0, episodes),
    };
    durations.sort_unstable();
    let median_minutes = if durations.is_empty() { None } else { Some(durations[(durations.len() - 1) / 2]) };
    Facts { env: current.env.clone(), firing, minutes, episodes_7d, resolved_7d: durations.len() as u32, median_minutes }
}
