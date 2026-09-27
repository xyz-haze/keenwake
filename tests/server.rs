mod common;
use common::{app, config, grafana, sink, system_one_from_state, FakeHttp, T0};
use keenwake::config::Mode;
use keenwake::digest::{day_key, due};
use keenwake::report::replay;
use keenwake::server::{handle_body, router, App};
use keenwake::store::Store;
use keenwake::worker;
use std::sync::Arc;
use tempfile::TempDir;

/// An app on the fake `backend` with a 300 ms timeout, so the slow fake (5 s) times out fast.
fn short_timeout_app(mode: Mode, backend: &FakeHttp, out: &FakeHttp) -> (App, TempDir) {
    let mut cfg = config(mode, &backend.url, &out.url);
    cfg.backend.timeout_ms = 300;
    app(cfg, Store::memory(), || T0)
}

#[tokio::test]
async fn gate_pings_high_and_escalates_middle() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    assert_eq!(handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await, 200);
    assert_eq!(handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f2", "firing")).await, 200);
    let sent = out.bodies();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["keenwake"]["decision"], "ping");
    assert_eq!(sent[1]["keenwake"]["decision"], "escalate");
}

#[tokio::test]
async fn gate_backend_down_fails_open() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("backend"));
}

#[tokio::test]
async fn observe_sends_only_verdicts() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Observe, &be, &out);
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f2", "firing")).await;
    assert!(out.bodies().iter().all(|b| b["keenwake"]["channel"] == "verdict"));
}

#[tokio::test]
async fn group_with_one_failure_still_decides_the_rest() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    let alerts: Vec<_> = (0..50).map(|i| serde_json::json!({"status": "firing", "fingerprint": format!("f{i}"),
        "labels": {"env": "prod"}, "annotations": {"summary": if i == 7 { "fail500".to_string() } else { format!("p=0.90 n{i}") }}})).collect();
    let body = serde_json::json!({"alerts": alerts}).to_string();
    assert_eq!(handle_body(&a, "grafana", body.as_bytes()).await, 200);
    assert_eq!(out.bodies().len(), 50);
    assert_eq!(a.store.decisions_since(0).len(), 50);
}

#[tokio::test]
async fn repeat_notification_does_not_ping_twice_and_resolved_follows_ping() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "resolved")).await;
    assert_eq!(out.kinds(), vec!["ping", "resolved"]);
    assert_eq!(be.bodies().len(), 2, "the repeat still asks the backend, the resolved never does");
}

#[tokio::test]
async fn repeat_untriaged_does_not_ping_twice_and_resolved_follows() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "resolved")).await;
    assert_eq!(out.kinds(), vec!["untriaged", "resolved"]);
}

#[tokio::test]
async fn unknown_source_and_garbage() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    assert_eq!(handle_body(&a, "nope", b"{}").await, 404);
    assert_eq!(handle_body(&a, "grafana", b"not json ops@example.com").await, 200);
    let sent = out.bodies();
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    assert!(!sent[0].to_string().contains("ops@example.com"));
    assert!(a.metrics.render().contains("keenwake_mapping_errors_total{source=\"grafana\"} 1"));
}

#[tokio::test]
async fn http_router_limits_body_size() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Observe, &be, &out);
    let a = Arc::new(a);
    let (q, _worker) = worker::start(a.clone(), worker::QUEUE_CAPACITY, worker::QUEUE_MAX_BYTES);
    let r = router(a, q);
    let big = vec![b'a'; 2 * 1024 * 1024];
    let resp = r.clone().oneshot(Request::post("/hook/grafana").body(Body::from(big)).unwrap()).await.unwrap();
    assert_eq!(resp.status().as_u16(), 413);
    let resp = r.oneshot(Request::get("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

#[tokio::test]
async fn hook_replies_before_a_slow_backend_finishes() {
    use axum::body::Body;
    use axum::http::Request;
    use std::time::{Duration, Instant};
    use tower::ServiceExt;
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    // A generously long backend timeout: if the handler awaited the backend inline, this
    // request would take the full 5s the fake backend sleeps for.
    let mut cfg = config(Mode::Observe, &be.url, &out.url);
    cfg.backend.timeout_ms = 10_000;
    let (a, _d) = app(cfg, Store::memory(), || T0);
    let a = Arc::new(a);
    let (q, _worker) = worker::start(a.clone(), worker::QUEUE_CAPACITY, worker::QUEUE_MAX_BYTES);
    let r = router(a, q);
    let body = grafana("slow", "f1", "firing");
    let t = Instant::now();
    let resp = r.oneshot(Request::post("/hook/grafana").body(Body::from(body)).unwrap()).await.unwrap();
    let elapsed = t.elapsed();
    assert_eq!(resp.status().as_u16(), 200);
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn digest_is_due_once_per_day_after_its_time() {
    let day = 1_790_000_000 - (1_790_000_000 % 86_400); // 00:00 UTC of some day
    let at = "08:00".parse().unwrap();
    assert!(!due(at, None, day + 7 * 3600));
    assert!(due(at, None, day + 8 * 3600));
    let today = day_key(day + 8 * 3600);
    assert!(!due(at, Some(&today), day + 9 * 3600));
    assert!(due(at, Some(&today), day + 86_400 + 8 * 3600));
}

#[tokio::test]
async fn gate_backend_timeout_pings_untriaged() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("slow", "f1", "firing")).await; // the fake answers after 5 s
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("backend timed out"), "{}", sent[0]["text"]);
}

#[tokio::test]
async fn gate_backend_quota_429_pings_untriaged() {
    let be = FakeHttp::start(Arc::new(|_| (429, "quota exceeded".into(), 0))).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("p=0.10 disk", "f1", "firing")).await;
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("HTTP 429"), "{}", sent[0]["text"]);
}

/// Invariant 6 on the shipped `replay`: a gate history recorded by `serve`, replayed with the same
/// config and the same deterministic backend, changes no decision.
#[tokio::test]
async fn replay_of_recorded_gate_history_changes_nothing() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    for (summary, fp, status) in [
        ("p=0.90 disk", "f1", "firing"),
        ("p=0.90 disk", "f1", "firing"),
        ("p=0.90 disk", "f1", "resolved"),
        ("p=0.90 disk", "f1", "firing"),
        ("p=0.40 cpu", "f2", "firing"),
        ("p=0.10 queue", "f3", "firing"),
    ] {
        handle_body(&a, "grafana", &grafana(summary, fp, status)).await;
    }
    let stored: Vec<&str> = a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind.as_str()).collect();
    assert_eq!(stored, vec!["ping", "repeat", "resolved", "ping", "escalate", "digest"]);
    assert_eq!(replay(&a, 0).await, vec![]);
}

/// Invariant 6 when a ping failed delivery: `serve` did not count it as sent, so the next firing
/// pinged again; `replay` must follow the recorded `delivered` flag, not the kind alone.
#[tokio::test]
async fn replay_follows_a_failed_delivery() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(Arc::new(|b| {
        let lost = b["keenwake"]["summary"].as_str().unwrap_or("").contains("lost");
        (if lost { 500 } else { 200 }, String::new(), 0)
    }))
    .await;
    let mut cfg = config(Mode::Gate, &be.url, &out.url);
    cfg.backend.timeout_ms = 300;
    let (mut a, _d) = app(cfg, Store::memory(), || T0);
    a.sender = keenwake::output::Sender::new(a.cfg.store.undelivered.clone(), 1).unwrap();
    handle_body(&a, "grafana", &grafana("p=0.90 disk lost", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk seen", "f1", "firing")).await;
    let stored: Vec<(&str, bool)> =
        a.store.decisions_since(0).into_iter().map(|(_, d)| (d.kind.as_str(), d.delivered)).collect();
    assert_eq!(stored, vec![("ping", false), ("ping", true)]);
    assert_eq!(replay(&a, 0).await, vec![]);
}

/// With on_error = "drop", an untriaged decision is sent nowhere, so it never makes a later
/// untriaged a repeat, in `serve` or in `replay`.
#[tokio::test]
async fn replay_of_dropped_untriaged_changes_nothing() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let mut cfg = config(Mode::Gate, &be.url, &out.url);
    cfg.backend.timeout_ms = 300;
    cfg.decision.on_error = keenwake::config::OnError::Drop;
    let (a, _d) = app(cfg, Store::memory(), || T0);
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    assert!(out.bodies().is_empty());
    assert_eq!(replay(&a, 0).await, vec![]);
}

static NOW_SINCE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(T0);
fn clock_since() -> i64 {
    NOW_SINCE.load(std::sync::atomic::Ordering::SeqCst)
}

/// A `--since` shorter than the repeat window: the ping that made the next firing a repeat lies
/// before the replayed range, and replay must still know about it.
#[tokio::test]
async fn replay_since_inside_the_repeat_window_knows_the_earlier_ping() {
    use std::sync::atomic::Ordering;
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let mut cfg = config(Mode::Gate, &be.url, &out.url);
    cfg.backend.timeout_ms = 300;
    let (a, _d) = app(cfg, Store::memory(), clock_since);
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    NOW_SINCE.store(T0 + 3600, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    let stored: Vec<&str> = a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind.as_str()).collect();
    assert_eq!(stored, vec!["ping", "repeat"]);
    assert_eq!(replay(&a, T0 + 1800).await, vec![]);
}

/// A Grafana body carrying `n` alerts that would each ping.
fn many_alerts(n: usize) -> Vec<u8> {
    let alerts: Vec<_> = (0..n)
        .map(|i| {
            serde_json::json!({"status": "firing", "fingerprint": format!("f{i}"),
            "labels": {"env": "prod"}, "annotations": {"summary": format!("p=0.90 n{i} ops@example.com")}})
        })
        .collect();
    serde_json::json!({"alerts": alerts}).to_string().into_bytes()
}

/// One POST with thousands of alerts must not turn into thousands of backend calls: over
/// `server.max_alerts_per_body` (default 500), the body is handled like an unreadable one.
#[tokio::test]
async fn gate_body_over_the_alert_cap_calls_no_backend_and_pings_once() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    assert_eq!(a.cfg.server.max_alerts_per_body, 500);
    assert_eq!(handle_body(&a, "grafana", &many_alerts(501)).await, 200);
    assert!(be.bodies().is_empty(), "no backend call");
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("501 alerts in one body"), "{}", sent[0]["text"]);
    assert!(!sent[0].to_string().contains("ops@example.com"), "the raw excerpt is redacted");
    assert!(a.store.decisions_since(0).is_empty());
    assert!(a.metrics.render().contains("keenwake_oversized_bodies_total{source=\"grafana\"} 1"));
}

#[tokio::test]
async fn observe_body_over_the_alert_cap_is_only_counted() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Observe, &be, &out);
    handle_body(&a, "grafana", &many_alerts(501)).await;
    assert!(be.bodies().is_empty());
    assert!(out.bodies().is_empty());
    assert!(a.metrics.render().contains("keenwake_oversized_bodies_total{source=\"grafana\"} 1"));
}

#[tokio::test]
async fn body_at_the_alert_cap_is_decided() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let mut cfg = config(Mode::Gate, &be.url, &out.url);
    cfg.server.max_alerts_per_body = 3;
    let (a, _d) = app(cfg, Store::memory(), || T0);
    handle_body(&a, "grafana", &many_alerts(3)).await;
    assert_eq!(out.kinds(), vec!["ping", "ping", "ping"]);
}

/// The status value is quoted in the mapping error, which reaches the untriaged ping's text.
#[tokio::test]
async fn bad_status_value_is_redacted_in_the_untriaged_text() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    handle_body(&a, "grafana", &grafana("disk", "f1", "ops@example.com AKIAIOSFODNN7EXAMPLE")).await;
    let sent = out.bodies();
    assert_eq!(sent[0]["keenwake"]["decision"], "untriaged");
    let text = sent[0]["text"].as_str().unwrap();
    assert!(text.contains("neither firing nor resolved"), "{text}");
    assert!(!text.contains("ops@example.com") && !text.contains("AKIAIOSFODNN7EXAMPLE"), "{text}");
}

/// One malformed alert in a group is handled as unreadable on its own; its neighbours are
/// stored and decided normally.
#[tokio::test]
async fn one_malformed_alert_does_not_fail_its_group() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = short_timeout_app(Mode::Gate, &be, &out);
    let alert = |fp: &str, status: &str| {
        serde_json::json!({"status": status, "fingerprint": fp,
        "labels": {"env": "prod"}, "annotations": {"summary": format!("p=0.90 {fp}")}})
    };
    let body = serde_json::json!({"alerts": [alert("f1", "firing"), alert("f2", "maybe"), alert("f3", "firing")]});
    assert_eq!(handle_body(&a, "grafana", body.to_string().as_bytes()).await, 200);
    assert_eq!(out.kinds(), vec!["ping", "untriaged", "ping"]);
    let raw = out.bodies()[1]["keenwake"]["raw"].as_str().unwrap().to_string();
    assert!(raw.contains("f2") && !raw.contains("f1") && !raw.contains("f3"), "only the bad alert: {raw}");
    let decided: Vec<String> = a.store.decisions_since(0).into_iter().map(|(e, _)| e.alert.identity).collect();
    assert_eq!(decided, vec!["f1", "f3"]);
    assert!(a.metrics.render().contains("keenwake_mapping_errors_total{source=\"grafana\"} 1"));
}
