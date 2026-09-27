//! Scrubs emails, IP addresses and tokens from the text fields sent to a model.

use regex::Regex;
use serde::Deserialize;

/// A family of secrets to scrub, as named in `redact.patterns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pattern {
    Email,
    Ip,
    Token,
}

pub struct Redactor {
    rules: Vec<(Regex, &'static str)>,
}

fn rules_for(pattern: Pattern) -> Vec<(Regex, &'static str)> {
    let r = |p: &str| Regex::new(p).expect("static regex");
    match pattern {
        Pattern::Email => vec![(r(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"), "[redacted:email]")],
        Pattern::Ip => vec![
            (r(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"), "[redacted:ip]"),
            (
                r(
                    r"\b(?:[0-9A-Fa-f]{1,4}:){3,7}[0-9A-Fa-f]{0,4}\b|\b(?:[0-9A-Fa-f]{1,4}:){1,7}:(?:[0-9A-Fa-f]{1,4}:?){0,6}\b",
                ),
                "[redacted:ip]",
            ),
        ],
        Pattern::Token => vec![
            (r(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]+"), "[redacted:token]"),
            (r(r"\b(?:sk|pk|rk|ghp|gho|ghs|glpat|xox[abprs])[-_][A-Za-z0-9_-]{10,}\b"), "[redacted:token]"),
            (r(r"\b[A-Za-z0-9_]{40,}\b"), "[redacted:token]"),
        ],
    }
}

impl Redactor {
    pub fn new(patterns: &[Pattern]) -> Redactor {
        Redactor { rules: patterns.iter().flat_map(|p| rules_for(*p)).collect() }
    }

    pub fn clean(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (re, repl) in &self.rules {
            out = re.replace_all(&out, *repl).into_owned();
        }
        out
    }
}
