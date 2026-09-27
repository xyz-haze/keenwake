//! Outgoing webhooks: generic JSON with a `text` field, 3 attempts, then undelivered.jsonl.

use crate::mapping::Alert;
use serde_json::{json, Value};
use std::io::Write;
use std::time::Duration;

pub const ATTEMPTS: u32 = 3;

pub fn message(kind: &str, a: &Alert, probability: Option<f64>, reason: &str, facts_line: &str) -> Value {
    let p = probability.map(|p| format!("{p:.2}")).unwrap_or_else(|| "n/a".into());
    let mut text = format!("[{kind}] {} ({}, {}, p={p})", a.summary, a.env, a.severity);
    if !reason.is_empty() {
        text.push_str(&format!(" - {reason}"));
    }
    if !facts_line.is_empty() {
        text.push_str(&format!(" - {facts_line}"));
    }
    json!({
        "text": text,
        "keenwake": {
            "decision": kind, "probability": probability, "reason": reason,
            "source": a.source, "identity": a.identity, "status": a.status.as_str(),
            "summary": a.summary, "details": a.details, "env": a.env, "severity": a.severity,
        }
    })
}

pub struct Sender {
    client: reqwest::Client,
    undelivered: String,
    base_delay_ms: u64,
}

impl Sender {
    pub fn new(undelivered: String, base_delay_ms: u64) -> Sender {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("http client");
        Sender { client, undelivered, base_delay_ms }
    }

    pub async fn post(&self, url: &str, body: &Value) -> bool {
        for attempt in 0..ATTEMPTS {
            if let Ok(r) = self.client.post(url).json(body).send().await {
                if r.status().is_success() {
                    return true;
                }
            }
            if attempt + 1 < ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(self.base_delay_ms << attempt)).await;
            }
        }
        self.write_undelivered(url, body);
        false
    }

    pub fn write_undelivered(&self, url: &str, body: &Value) {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let line = json!({"url": url, "body": body, "at": at});
        // Build the whole line before writing: several concurrent failures append to this
        // file from different tasks, and a single write_all() on an O_APPEND file descriptor
        // is atomic, whereas writeln!'s piecemeal writes could interleave and corrupt lines.
        let mut s = line.to_string();
        s.push('\n');
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.undelivered) {
            let _ = f.write_all(s.as_bytes());
        }
    }
}
