use crate::decide::Kind;
use crate::mapping::Alert;
use serde_json::{json, Value};
use std::io::Write;
use std::time::Duration;

pub const ATTEMPTS: u32 = 3;

pub fn message(kind: Kind, a: &Alert, probability: Option<f64>, reason: &str, facts_line: &str) -> Value {
    let p = probability.map(|p| format!("{p:.2}")).unwrap_or_else(|| "n/a".into());
    let kind = kind.as_str();
    let mut text = format!("[{kind}] {} ({}, {}, p={p})", a.summary, a.env, a.severity);
    for extra in [reason, facts_line].into_iter().filter(|e| !e.is_empty()) {
        text.push_str(" - ");
        text.push_str(extra);
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
    pub fn new(undelivered: String, base_delay_ms: u64) -> Result<Sender, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Sender { client, undelivered, base_delay_ms })
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
        // One write_all per line: concurrent failures append from different tasks, and a single
        // O_APPEND write does not interleave, whereas writeln!'s piecemeal writes could.
        let mut s = line.to_string();
        s.push('\n');
        let mut open = std::fs::OpenOptions::new();
        open.create(true).append(true);
        // Lines hold webhook URLs, which often embed a secret: owner-only when created.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
        if let Ok(mut f) = open.open(&self.undelivered) {
            let _ = f.write_all(s.as_bytes());
        }
    }
}
