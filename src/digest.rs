//! Daily digest: once a day after digest_at (UTC), one POST with everything classified digest.

use crate::server::App;
use serde_json::json;
use std::sync::Arc;

pub fn day_key(now: i64) -> String { format!("{}", now.div_euclid(86_400)) }

pub fn due(digest_at: &str, last_sent_day: Option<&str>, now: i64) -> bool {
    let (h, m) = digest_at.split_once(':').map(|(h, m)| (h.parse::<i64>().unwrap_or(8), m.parse::<i64>().unwrap_or(0))).unwrap_or((8, 0));
    let secs_today = now.rem_euclid(86_400);
    secs_today >= h * 3600 + m * 60 && last_sent_day != Some(day_key(now).as_str())
}

pub async fn tick(app: &App, now: i64) -> bool {
    let last = app.store.meta_get("digest_last_day");
    if !due(&app.cfg.decision.digest_at, last.as_deref(), now) { return false; }
    app.store.meta_set("digest_last_day", &day_key(now));
    let events = app.store.take_digest();
    let Some(url) = app.cfg.outputs.digest.as_deref() else { return false };
    if events.is_empty() { return false; }
    let lines: Vec<String> = events.iter().map(|e| format!("- {} ({})", e.alert.summary, e.alert.env)).collect();
    let body = json!({
        "text": format!("[digest] {} alerts that did not need a ping:\n{}", events.len(), lines.join("\n")),
        "alertsift": {"decision": "digest", "alerts": events.iter().map(|e| json!({
            "identity": e.alert.identity, "summary": e.alert.summary, "env": e.alert.env, "severity": e.alert.severity,
            "received_at": e.received_at})).collect::<Vec<_>>()},
    });
    let ok = app.sender.post(url, &body).await;
    if !ok { app.metrics.inc("alertsift_undelivered_total", &[]); }
    ok
}

/// `tick` in its own task: a panic there (e.g. a store error) is logged and counted, and the
/// digest loop keeps running.
pub async fn guarded_tick(app: &Arc<App>, now: i64) -> bool {
    let a = app.clone();
    match tokio::spawn(async move { tick(&a, now).await }).await {
        Ok(sent) => sent,
        Err(_) => {
            eprintln!("alertsift: internal error while sending the digest");
            app.metrics.inc("alertsift_internal_errors_total", &[]);
            false
        }
    }
}
