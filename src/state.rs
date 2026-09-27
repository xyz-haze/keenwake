//! The sentence sent to the model: history conclusions first, then the alert text.
//! Wording mirrors the spike on which Laya reached AUC 0.994; change it only with the slow test.

use crate::history::Facts;
use crate::mapping::Alert;

pub const MAX_FIELD_CHARS: usize = 1000;

fn cut(s: &str) -> String {
    s.chars().take(MAX_FIELD_CHARS).collect()
}

fn trim_dot(s: &str) -> &str {
    s.trim().trim_end_matches('.')
}

pub fn sentence(a: &Alert, f: &Facts) -> String {
    let mut parts = vec![format!("Environment: {}.", f.env)];
    if f.env != "prod" && f.env != "production" {
        parts[0].push_str(" This is not production.");
    }
    if f.firing {
        parts.push(format!("The alert is still firing, for {} minutes so far.", f.minutes));
    } else {
        parts.push(format!("The alert is already resolved after {} minutes.", f.minutes));
    }
    match (f.episodes_7d, f.median_minutes) {
        (0, _) => parts.push("This alert has never fired before in the last 7 days.".into()),
        (n, None) => parts.push(format!("It fired {n} times in the last 7 days and never resolved on its own.")),
        (n, Some(med)) => {
            let pct = (f.resolved_7d * 100) / n;
            parts.push(format!("It fired {n} times in the last 7 days and resolved on its own {pct}% of the time, usually within about {med} minutes."));
            if f.firing {
                if f.minutes > 3 * med {
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
