//! Turns a raw webhook body into alerts, reading only the fields the spec names.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status { Firing, Resolved }

impl Status {
    pub fn as_str(self) -> &'static str {
        match self { Status::Firing => "firing", Status::Resolved => "resolved" }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s { "firing" => Some(Status::Firing), "resolved" => Some(Status::Resolved), _ => None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub source: String,
    pub status: Status,
    pub identity: String,
    pub summary: String,
    pub details: String,
    pub env: String,
    pub severity: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged, deny_unknown_fields)]
pub enum FieldSpec {
    Path { path: String, #[serde(default)] map: Option<BTreeMap<String, String>> },
    FirstOf { first_of: Vec<String>, #[serde(default)] map: Option<BTreeMap<String, String>> },
    Const { #[serde(rename = "const")] value: String },
    Template { template: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    pub status: FieldSpec,
    pub identity: Option<FieldSpec>,
    pub summary: FieldSpec,
    pub details: Option<FieldSpec>,
    pub env: Option<FieldSpec>,
    pub severity: Option<FieldSpec>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    #[serde(default)]
    pub alerts: String,
    pub fields: Fields,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MapError {
    #[error("body is not JSON")]
    NotJson,
    #[error("alerts pointer {0} does not point to an array")]
    NoAlerts(String),
    #[error("required field {0} is missing")]
    Missing(&'static str),
    #[error("status {0:?} is neither firing nor resolved")]
    BadStatus(String),
}

fn pointer(p: &str) -> String {
    if p.is_empty() || p.starts_with('/') { p.to_string() } else { format!("/{p}") }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn lookup(item: &Value, p: &str) -> Option<String> {
    item.pointer(&pointer(p)).and_then(text).filter(|s| !s.is_empty())
}

fn translate(v: String, map: &Option<BTreeMap<String, String>>) -> String {
    match map { Some(m) => m.get(&v).cloned().unwrap_or(v), None => v }
}

fn render(template: &str, item: &Value) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(len) => {
                let key = &rest[start + 1..start + len];
                out.push_str(&lookup(item, key).unwrap_or_default());
                rest = &rest[start + len + 1..];
            }
            None => { out.push_str(&rest[start..]); rest = ""; }
        }
    }
    out.push_str(rest);
    out
}

fn resolve(spec: &FieldSpec, item: &Value) -> Option<String> {
    let v = match spec {
        FieldSpec::Path { path, map } => lookup(item, path).map(|v| translate(v, map)),
        FieldSpec::FirstOf { first_of, map } => first_of.iter().find_map(|p| lookup(item, p)).map(|v| translate(v, map)),
        FieldSpec::Const { value } => Some(value.clone()),
        FieldSpec::Template { template } => Some(render(template, item)),
    };
    v.filter(|s| !s.is_empty())
}

pub fn default_identity(source: &str, summary: &str) -> String {
    let stripped: String = summary.chars().filter(|c| !c.is_ascii_digit()).collect();
    let digest = Sha256::digest(format!("{source}\0{stripped}").as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn one(name: &str, f: &Fields, item: &Value) -> Result<Alert, MapError> {
    let raw_status = resolve(&f.status, item).ok_or(MapError::Missing("status"))?;
    let status = Status::parse(&raw_status).ok_or(MapError::BadStatus(raw_status))?;
    let summary = resolve(&f.summary, item).ok_or(MapError::Missing("summary"))?;
    let opt = |s: &Option<FieldSpec>| s.as_ref().and_then(|s| resolve(s, item));
    let identity = opt(&f.identity).unwrap_or_else(|| default_identity(name, &summary));
    Ok(Alert {
        source: name.to_string(),
        status,
        identity,
        summary,
        details: opt(&f.details).unwrap_or_default(),
        env: opt(&f.env).unwrap_or_else(|| "unknown".into()),
        severity: opt(&f.severity).unwrap_or_else(|| "unknown".into()),
    })
}

pub fn extract(name: &str, spec: &SourceSpec, body: &[u8]) -> Result<Vec<Alert>, MapError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| MapError::NotJson)?;
    if spec.alerts.is_empty() {
        return Ok(vec![one(name, &spec.fields, &root)?]);
    }
    let items = root.pointer(&pointer(&spec.alerts)).and_then(Value::as_array)
        .ok_or_else(|| MapError::NoAlerts(spec.alerts.clone()))?;
    items.iter().map(|item| one(name, &spec.fields, item)).collect()
}
