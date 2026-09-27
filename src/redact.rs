//! Scrubs emails, IP addresses and tokens from the text fields sent to a model.

use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

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

type Rules = Vec<(Regex, &'static str)>;

// Compiled once per process: a `Redactor` is cheap to build (cloning a `Regex` shares it).
static CONTEXTUAL: LazyLock<Rules> = LazyLock::new(contextual_token_rules);
static EMAIL: LazyLock<Rules> = LazyLock::new(|| rules_for(Pattern::Email));
static IP: LazyLock<Rules> = LazyLock::new(|| rules_for(Pattern::Ip));
static TOKENS: LazyLock<Rules> = LazyLock::new(|| rules_for(Pattern::Token));

const TOKEN: &str = "[redacted:token]";

/// Keys whose value is a secret, as in `api_key=...`, `password: ...` or `"token": "..."`.
const SECRET_KEYS: &str = "aws_secret_access_key|aws_session_token|api[_-]?key|apikey|access[_-]?token|auth[_-]?token\
                           |client[_-]?secret|token|secret|password|passwd|pwd";

fn r(p: &str) -> Regex {
    Regex::new(p).expect("static regex")
}

/// Token rules that read the context around a secret (a URL, a header, a key). They run before
/// every other rule: the email rule would otherwise eat `pass@host` and lose the host.
fn contextual_token_rules() -> Rules {
    vec![
        // scheme://user:pass@host keeps scheme, user and host.
        (r(r"(?i)\b([a-z][a-z0-9+.-]*://[^\s:/@]*):[^\s@/]+@"), "$1:[redacted:token]@"),
        (
            r(r"(?i)\b(https?://hooks\.slack\.com/)(?:services|workflows|triggers)/[A-Za-z0-9/_-]+"),
            "${1}[redacted:token]",
        ),
        (
            r(r"(?i)\b(https?://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/webhooks/)\d+/[A-Za-z0-9_-]+"),
            "${1}[redacted:token]",
        ),
        (r(r"(?i)\b(authorization\s*[:=]\s*)(?:basic|bearer|token)\s+[A-Za-z0-9._~+/=-]+"), "${1}[redacted:token]"),
        (r(&format!(r#"(?i)\b((?:{SECRET_KEYS})["']?\s*[=:]\s*["']?)[^\s"'&,;]+"#)), "${1}[redacted:token]"),
    ]
}

fn rules_for(pattern: Pattern) -> Rules {
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
            (r(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]+"), TOKEN),
            // AWS access key ids: long-term (AKIA) and temporary (ASIA), plus user and role ids.
            (r(r"\b(?:AKIA|ASIA|AIDA|AROA)[A-Z0-9]{16}\b"), TOKEN),
            // A JWT, or its header alone: base64url of `{"`.
            (r(r"\beyJ[A-Za-z0-9_-]{10,}(?:\.[A-Za-z0-9_-]+){0,2}"), TOKEN),
            (r(r"\b(?:sk|pk|rk|ghp|gho|ghs|glpat|xox[abprs])[-_][A-Za-z0-9_-]{10,}\b"), TOKEN),
            (r(r"\b[A-Za-z0-9_]{40,}\b"), TOKEN),
        ],
    }
}

impl Redactor {
    pub fn new(patterns: &[Pattern]) -> Redactor {
        let compiled = |p: &Pattern| -> &'static Rules {
            match p {
                Pattern::Email => &EMAIL,
                Pattern::Ip => &IP,
                Pattern::Token => &TOKENS,
            }
        };
        let early: &[_] = if patterns.contains(&Pattern::Token) { &CONTEXTUAL } else { &[] };
        Redactor { rules: early.iter().chain(patterns.iter().flat_map(compiled)).cloned().collect() }
    }

    pub fn clean(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (re, repl) in &self.rules {
            out = re.replace_all(&out, *repl).into_owned();
        }
        out
    }
}
