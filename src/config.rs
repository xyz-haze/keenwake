//! Loads triage.toml: backend, thresholds, outputs, and the source mappings.

use crate::mapping::SourceSpec;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Observe,
    Gate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    #[default]
    Ping,
    Drop,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub instructions: String,
    pub criteria_true: String,
    pub criteria_false: String,
}

impl Default for Question {
    fn default() -> Self {
        Question {
            instructions: "Should the on-call engineer be interrupted right now because of this alert?".into(),
            criteria_true: "A real, ongoing problem in production that harms users or money and will not fix itself soon.".into(),
            criteria_false: "Noise: not production, already resolved, self-healing as usual, planned maintenance, or something that can wait for business hours.".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendCfg {
    pub url: String,
    pub model: String,
    pub api_key_env: Option<String>,
    #[serde(default = "d_timeout")]
    pub timeout_ms: u64,
}
fn d_timeout() -> u64 {
    2000
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionCfg {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub on_error: OnError,
    #[serde(default = "d_ping")]
    pub ping: f64,
    #[serde(default = "d_digest")]
    pub digest: f64,
    #[serde(default = "d_digest_at")]
    pub digest_at: String,
    /// A delivered ping (or untriaged ping) suppresses later ones for the same identity only if it
    /// was decided within this many hours before the new event, so a lost `resolved` cannot
    /// silence an identity forever.
    #[serde(default = "d_repeat_window_hours")]
    pub repeat_window_hours: u64,
}
fn d_ping() -> f64 {
    0.55
}
fn d_digest() -> f64 {
    0.30
}
fn d_digest_at() -> String {
    "08:00".into()
}
fn d_repeat_window_hours() -> u64 {
    24
}
impl DecisionCfg {
    pub fn repeat_window_secs(&self) -> i64 {
        i64::try_from(self.repeat_window_hours.saturating_mul(3600)).unwrap_or(i64::MAX)
    }
}
impl Default for DecisionCfg {
    fn default() -> Self {
        DecisionCfg {
            mode: Mode::Observe,
            on_error: OnError::Ping,
            ping: d_ping(),
            digest: d_digest(),
            digest_at: d_digest_at(),
            repeat_window_hours: d_repeat_window_hours(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OutputsCfg {
    pub ping: Option<String>,
    pub escalate: Option<String>,
    pub digest: Option<String>,
    pub verdict: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactCfg {
    #[serde(default = "d_patterns")]
    pub patterns: Vec<String>,
}
fn d_patterns() -> Vec<String> {
    vec!["email".into(), "ip".into(), "token".into()]
}
impl Default for RedactCfg {
    fn default() -> Self {
        RedactCfg { patterns: d_patterns() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerCfg {
    #[serde(default = "d_listen")]
    pub listen: String,
}
fn d_listen() -> String {
    "0.0.0.0:8080".into()
}
impl Default for ServerCfg {
    fn default() -> Self {
        ServerCfg { listen: d_listen() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreCfg {
    #[serde(default = "d_db")]
    pub path: String,
    #[serde(default = "d_undelivered")]
    pub undelivered: String,
}
fn d_db() -> String {
    "keenwake.db".into()
}
fn d_undelivered() -> String {
    "undelivered.jsonl".into()
}
impl Default for StoreCfg {
    fn default() -> Self {
        StoreCfg { path: d_db(), undelivered: d_undelivered() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    backend: BackendCfg,
    #[serde(default)]
    decision: DecisionCfg,
    #[serde(default)]
    outputs: OutputsCfg,
    #[serde(default)]
    redact: RedactCfg,
    #[serde(default)]
    server: ServerCfg,
    #[serde(default)]
    store: StoreCfg,
    question: Option<Question>,
    #[serde(default)]
    source: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub backend: BackendCfg,
    pub decision: DecisionCfg,
    pub outputs: OutputsCfg,
    pub redact: RedactCfg,
    pub server: ServerCfg,
    pub store: StoreCfg,
    pub question: Question,
    pub sources: BTreeMap<String, SourceSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config: {0}")]
    Parse(String),
    #[error("invalid config: {0}")]
    Invalid(String),
}

const PRESETS: &[(&str, &str)] = &[
    ("grafana", include_str!("../presets/grafana.toml")),
    ("alertmanager", include_str!("../presets/alertmanager.toml")),
];

pub fn preset(name: &str) -> Option<&'static str> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

fn invalid(msg: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(msg.into())
}

/// Overlays `user` on `base`: top-level keys replace, `fields` merges key by key.
fn overlay(mut base: toml::Table, mut user: toml::Table) -> toml::Table {
    if let Some(toml::Value::Table(uf)) = user.remove("fields") {
        let bf = base.entry("fields").or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(bf) = bf {
            for (k, v) in uf {
                bf.insert(k, v);
            }
        }
    }
    for (k, v) in user {
        base.insert(k, v);
    }
    base
}

fn source_spec(name: &str, mut table: toml::Table) -> Result<SourceSpec, ConfigError> {
    let named = match table.remove("preset") {
        Some(v) => Some(v),
        None if preset(name).is_some() => Some(toml::Value::String(name.to_string())),
        None => None,
    };
    let merged = match named {
        Some(toml::Value::String(p)) => {
            let text = preset(&p).ok_or_else(|| invalid(format!("source {name}: unknown preset {p:?}")))?;
            let base: toml::Table = toml::from_str(text).map_err(|e| invalid(format!("preset {p}: {e}")))?;
            overlay(base, table)
        }
        Some(_) => return Err(invalid(format!("source {name}: preset must be a string"))),
        None => table,
    };
    toml::Value::Table(merged)
        .try_into()
        .map_err(|e| invalid(format!("source {name}: {e}")))
}

fn valid_hhmm(s: &str) -> bool {
    let Some((h, m)) = s.split_once(':') else { return false };
    matches!((h.parse::<u32>(), m.parse::<u32>()), (Ok(h), Ok(m)) if h < 24 && m < 60 && s.len() == 5)
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        Config::from_toml(&std::fs::read_to_string(path)?)
    }

    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let mut sources = BTreeMap::new();
        for (name, _) in PRESETS {
            let mut t = toml::Table::new();
            t.insert("preset".into(), toml::Value::String((*name).into()));
            sources.insert((*name).to_string(), source_spec(name, t)?);
        }
        for (name, table) in raw.source {
            sources.insert(name.clone(), source_spec(&name, table)?);
        }
        let c = Config {
            backend: raw.backend,
            decision: raw.decision,
            outputs: raw.outputs,
            redact: raw.redact,
            server: raw.server,
            store: raw.store,
            question: raw.question.unwrap_or_default(),
            sources,
        };
        c.validate()?;
        Ok(c)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.backend.model.contains("latest") {
            return Err(invalid("backend.model must pin a version, not \"latest\""));
        }
        let d = &self.decision;
        if !(0.0..=1.0).contains(&d.ping) || !(0.0..=1.0).contains(&d.digest) {
            return Err(invalid("decision.ping and decision.digest must be between 0 and 1"));
        }
        if d.digest > d.ping {
            return Err(invalid("decision.digest must be lower than or equal to decision.ping"));
        }
        if !valid_hhmm(&d.digest_at) {
            return Err(invalid("decision.digest_at must be HH:MM, UTC"));
        }
        if d.repeat_window_hours == 0 {
            return Err(invalid("decision.repeat_window_hours must be greater than 0"));
        }
        if d.mode == Mode::Gate && self.outputs.ping.is_none() {
            return Err(invalid("mode = \"gate\" requires outputs.ping"));
        }
        if d.mode == Mode::Gate && self.outputs.escalate.is_none() {
            return Err(invalid("mode = \"gate\" requires outputs.escalate"));
        }
        if d.mode == Mode::Gate && self.outputs.digest.is_none() {
            return Err(invalid("mode = \"gate\" requires outputs.digest"));
        }
        for p in &self.redact.patterns {
            if !["email", "ip", "token"].contains(&p.as_str()) {
                return Err(invalid(format!("redact.patterns: unknown pattern {p:?}")));
            }
        }
        Ok(())
    }
}
