//! The sentence sent to the model: history conclusions first, then the alert text.
//! The wording is guarded by the slow corpus test (tests/corpus.rs): change it only with that test.

use crate::history::Facts;
use crate::mapping::{Alert, UNKNOWN};

pub const MAX_FIELD_CHARS: usize = 1000;

fn cut(s: &str) -> String {
    s.chars().take(MAX_FIELD_CHARS).collect()
}

fn trim_dot(s: &str) -> &str {
    s.trim().trim_end_matches('.')
}

pub fn sentence(a: &Alert, f: &Facts) -> String {
    let mut parts = vec![format!("Environment: {}.", f.env)];
    // An unmapped env says nothing about production: only an explicit other env earns the clause.
    if f.env != "prod" && f.env != "production" && f.env != UNKNOWN {
        parts[0].push_str(" This is not production.");
    }
    if f.firing {
        parts.push(format!("The alert is still firing, for {} minutes so far.", f.minutes));
    } else {
        parts.push(format!("The alert is already resolved after {} minutes.", f.minutes));
    }
    // No claim about *how* past episodes ended: a self-resolution and a human fix look the same.
    match f.median_minutes {
        None => parts.push("This alert has never fired before in the last 7 days.".into()),
        Some(med) => {
            let n = f.episodes_7d;
            // Durations are whole minutes, so flaps that end within the minute have a median of 0.
            if med == 0 {
                parts.push(format!("It fired {n} times in the last 7 days, and each time it ended within a minute."));
            } else {
                parts.push(format!(
                    "It fired {n} times in the last 7 days, and each time it ended, usually within about {med} minutes."
                ));
            }
            if f.firing {
                if f.minutes > 3 * med.max(1) {
                    parts.push("This time it has lasted much longer than usual.".into());
                } else {
                    parts.push("This time it looks like its usual pattern so far.".into());
                }
            }
        }
    }
    parts.push(format!("Alert: {}.", trim_dot(&cut(&a.summary))));
    if !a.details.trim().is_empty() {
        parts.push(format!("{}.", trim_dot(&cut(&a.details))));
    }
    parts.push(format!("Severity label: {}.", trim_dot(&cut(&a.severity))));
    parts.join(" ")
}
