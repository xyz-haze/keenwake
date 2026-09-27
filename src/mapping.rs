//! Turns a raw webhook body into alerts, reading only the fields the spec names.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Firing,
    Resolved,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Firing => "firing",
            Status::Resolved => "resolved",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "firing" => Some(Status::Firing),
            "resolved" => Some(Status::Resolved),
            _ => None,
        }
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
    Path {
        path: String,
        #[serde(default)]
        map: Option<BTreeMap<String, String>>,
    },
    FirstOf {
        first_of: Vec<String>,
        #[serde(default)]
        map: Option<BTreeMap<String, String>>,
    },
    Const {
        #[serde(rename = "const")]
        value: String,
    },
    Template {
        template: String,
    },
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

impl Fields {
    fn named(&self) -> [(&'static str, Option<&FieldSpec>); 6] {
        [
            ("status", Some(&self.status)),
            ("identity", self.identity.as_ref()),
            ("summary", Some(&self.summary)),
            ("details", self.details.as_ref()),
            ("env", self.env.as_ref()),
            ("severity", self.severity.as_ref()),
        ]
    }
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

/// The env or severity of an alert whose mapping names no such field, or whose field is empty.
pub const UNKNOWN: &str = "unknown";

fn pointer(p: &str) -> String {
    if p.is_empty() || p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
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

fn translate(v: String, map: Option<&BTreeMap<String, String>>) -> String {
    match map {
        Some(m) => m.get(&v).cloned().unwrap_or(v),
        None => v,
    }
}

/// Expands each `{key}` of `template` with `value(key)`; an unclosed `{` is kept as text.
fn expand(template: &str, mut value: impl FnMut(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(len) => {
                out.push_str(&value(&rest[start + 1..start + len]));
                rest = &rest[start + len + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn render(template: &str, item: &Value) -> String {
    expand(template, |key| lookup(item, key).unwrap_or_default())
}

fn resolve(spec: &FieldSpec, item: &Value) -> Option<String> {
    let v = match spec {
        FieldSpec::Path { path, map } => lookup(item, path).map(|v| translate(v, map.as_ref())),
        FieldSpec::FirstOf { first_of, map } => {
            first_of.iter().find_map(|p| lookup(item, p)).map(|v| translate(v, map.as_ref()))
        }
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
    let opt = |s: Option<&FieldSpec>| s.and_then(|s| resolve(s, item));
    let identity = opt(f.identity.as_ref()).unwrap_or_else(|| default_identity(name, &summary));
    Ok(Alert {
        source: name.to_string(),
        status,
        identity,
        summary,
        details: opt(f.details.as_ref()).unwrap_or_default(),
        env: opt(f.env.as_ref()).unwrap_or_else(|| UNKNOWN.into()),
        severity: opt(f.severity.as_ref()).unwrap_or_else(|| UNKNOWN.into()),
    })
}

/// One alert of a body that could not be mapped, with the payload item it came from.
#[derive(Debug, PartialEq)]
pub struct BadItem {
    pub error: MapError,
    pub item: Value,
}

/// The alert items of a parsed body: the root itself when the spec names no alerts pointer.
fn items<'a>(spec: &SourceSpec, root: &'a Value) -> Result<Vec<&'a Value>, MapError> {
    if spec.alerts.is_empty() {
        return Ok(vec![root]);
    }
    let items = root
        .pointer(&pointer(&spec.alerts))
        .and_then(Value::as_array)
        .ok_or_else(|| MapError::NoAlerts(spec.alerts.clone()))?;
    Ok(items.iter().collect())
}

/// Maps each alert of the body on its own, so one malformed alert does not hide its
/// neighbours. `Err` only when the body as a whole is unreadable.
pub fn extract_each(name: &str, spec: &SourceSpec, body: &[u8]) -> Result<Vec<Result<Alert, BadItem>>, MapError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| MapError::NotJson)?;
    let map = |item: &Value| one(name, &spec.fields, item).map_err(|error| BadItem { error, item: item.clone() });
    Ok(items(spec, &root)?.into_iter().map(map).collect())
}

/// Like `extract_each`, but the first malformed alert fails the whole body.
pub fn extract(name: &str, spec: &SourceSpec, body: &[u8]) -> Result<Vec<Alert>, MapError> {
    extract_each(name, spec, body)?.into_iter().map(|r| r.map_err(|b| b.error)).collect()
}

/// A field reference of a source spec that found nothing in one alert of a payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Unresolved {
    /// Index of the alert in the payload.
    pub alert: usize,
    pub field: &'static str,
    /// The pointer that missed; for `first_of`, every candidate, comma-separated.
    pub pointer: String,
}

impl fmt::Display for Unresolved {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "alert {}, field {}: {} resolved to nothing", self.alert, self.field, self.pointer)
    }
}

/// The pointers of `spec` that find nothing in `item`. A `first_of` misses only when every
/// candidate does: falling through to a later one is what it is for.
fn misses(spec: &FieldSpec, item: &Value) -> Vec<String> {
    match spec {
        FieldSpec::Path { path, .. } => lookup(item, path).is_none().then(|| pointer(path)).into_iter().collect(),
        FieldSpec::FirstOf { first_of, .. } => {
            if first_of.iter().any(|p| lookup(item, p).is_some()) {
                vec![]
            } else {
                vec![first_of.iter().map(|p| pointer(p)).collect::<Vec<_>>().join(", ")]
            }
        }
        FieldSpec::Const { .. } => vec![],
        FieldSpec::Template { template } => {
            let mut missed = Vec::new();
            expand(template, |key| {
                if lookup(item, key).is_none() {
                    missed.push(pointer(key));
                }
                String::new()
            });
            missed
        }
    }
}

/// Every field reference that resolves to nothing in `body`, for `check-source`. `serve` does not
/// call this: there a missing optional field is normal and falls back to its default.
pub fn unresolved(spec: &SourceSpec, body: &[u8]) -> Result<Vec<Unresolved>, MapError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| MapError::NotJson)?;
    let mut out = Vec::new();
    for (alert, item) in items(spec, &root)?.into_iter().enumerate() {
        for (field, fs) in spec.fields.named() {
            for pointer in fs.map(|fs| misses(fs, item)).unwrap_or_default() {
                out.push(Unresolved { alert, field, pointer });
            }
        }
    }
    Ok(out)
}
