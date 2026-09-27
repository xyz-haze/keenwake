//! Repeat suppression is bounded in time: a lost `resolved` must not silence an identity forever.
mod common;
use alertsift::config::Config;
use alertsift::server::{handle_body, App};
use common::{system_one_from_state, FakeHttp};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

const T0: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

// One clock per test: tests in this file run in parallel.
static NOW_LOST: AtomicI64 = AtomicI64::new(T0);
fn clock_lost() -> i64 { NOW_LOST.load(Ordering::SeqCst) }
static NOW_WINDOW: AtomicI64 = AtomicI64::new(T0);
fn clock_window() -> i64 { NOW_WINDOW.load(Ordering::SeqCst) }

fn grafana(summary: &str, fp: &str, status: &str) -> Vec<u8> {
    serde_json::json!({"alerts": [{"status": status, "fingerprint": fp,
        "labels": {"alertname": "A", "env": "prod", "severity": "critical"},
        "annotations": {"summary": summary}}]}).to_string().into_bytes()
}

async fn gate_app(be: &FakeHttp, out: &FakeHttp, dir: &tempfile::TempDir, clock: fn() -> i64) -> App {
    let toml = format!("[backend]\nurl='{}'\nmodel='m-1'\n[decision]\nmode='gate'\n[outputs]\nping='{}/ping'\nescalate='{}/e'\ndigest='{}/d'\n[store]\nundelivered='{}'\n",
        be.url, out.url, out.url, out.url, dir.path().join("u").display());
    App::new(Config::from_toml(&toml).unwrap(), alertsift::store::Store::memory(), clock).unwrap()
}

fn sent_kinds(out: &FakeHttp) -> Vec<String> {
    out.bodies().iter().map(|b| b["alertsift"]["decision"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn lost_resolved_does_not_silence_a_new_incident_30_days_later() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(Arc::new(|_| (200, "ok".into(), 0))).await;
    let dir = tempfile::tempdir().unwrap();
    let a = gate_app(&be, &out, &dir, clock_lost).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    // The resolved is never received (restart, send_resolved off, ...). 30 days later, a new incident.
    NOW_LOST.store(T0 + 30 * 24 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.99 disk full", "f1", "firing")).await;
    assert_eq!(sent_kinds(&out), vec!["ping", "ping"], "the second incident must be sent");
}

#[tokio::test]
async fn repeat_within_the_window_is_suppressed_and_after_it_pings_again() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(Arc::new(|_| (200, "ok".into(), 0))).await;
    let dir = tempfile::tempdir().unwrap();
    let a = gate_app(&be, &out, &dir, clock_window).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    NOW_WINDOW.store(T0 + 23 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    assert_eq!(sent_kinds(&out), vec!["ping"], "a repeat 23 h later stays suppressed (default window 24 h)");
    // 25 h after the only delivered ping: outside the window, so it pings again.
    NOW_WINDOW.store(T0 + 25 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    assert_eq!(sent_kinds(&out), vec!["ping", "ping"]);
    let stored: Vec<String> = a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind).collect();
    assert_eq!(stored, vec!["ping", "repeat", "ping"]);
}
