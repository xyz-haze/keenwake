mod common;
use alertsift::config::Config;
use alertsift::server::{handle_body, App};
use common::{system_one_from_state, FakeHttp};
use std::sync::Arc;

async fn app(mode: &str, on_error: &str, backend: &FakeHttp, out: &FakeHttp, extra: &str) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(r#"
[backend]
url = "{}"
model = "m-1"
timeout_ms = 300
[decision]
mode = "{mode}"
on_error = "{on_error}"
[outputs]
ping = "{}/ping"
escalate = "{}/escalate"
digest = "{}/digest"
verdict = "{}/verdict"
[store]
undelivered = "{}"
{extra}
"#, backend.url, out.url, out.url, out.url, out.url, dir.path().join("u.jsonl").display());
    let cfg = Config::from_toml(&toml).unwrap();
    (App::new(cfg, alertsift::store::Store::memory(), || 1_800_000_000).unwrap(), dir)
}

fn grafana(summary: &str, fp: &str, status: &str) -> Vec<u8> {
    serde_json::json!({"alerts": [{"status": status, "fingerprint": fp,
        "labels": {"alertname": "A", "env": "prod", "severity": "critical"},
        "annotations": {"summary": summary}}]}).to_string().into_bytes()
}

fn sink() -> common::Responder { Arc::new(|_| (200, "ok".into(), 0)) }

#[tokio::test]
async fn gate_pings_high_and_escalates_middle() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    assert_eq!(handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await, 200);
    assert_eq!(handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f2", "firing")).await, 200);
    let sent = out.bodies();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["alertsift"]["decision"], "ping");
    assert_eq!(sent[1]["alertsift"]["decision"], "escalate");
}

#[tokio::test]
async fn gate_backend_down_fails_open() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["alertsift"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("backend"));
}

#[tokio::test]
async fn observe_sends_only_verdicts() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("observe", "ping", &be, &out, "").await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f2", "firing")).await;
    assert!(out.bodies().iter().all(|b| b["alertsift"]["channel"] == "verdict"));
}

#[tokio::test]
async fn group_with_one_failure_still_decides_the_rest() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
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
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.90 disk", "f1", "resolved")).await;
    let kinds: Vec<_> = out.bodies().iter().map(|b| b["alertsift"]["decision"].as_str().unwrap().to_string()).collect();
    assert_eq!(kinds, vec!["ping", "resolved"]);
    assert_eq!(be.bodies().len(), 2, "the repeat still asks the backend, the resolved never does");
}

#[tokio::test]
async fn repeat_untriaged_does_not_ping_twice_and_resolved_follows() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500", "f1", "resolved")).await;
    let kinds: Vec<_> = out.bodies().iter().map(|b| b["alertsift"]["decision"].as_str().unwrap().to_string()).collect();
    assert_eq!(kinds, vec!["untriaged", "resolved"]);
}

#[tokio::test]
async fn unknown_source_and_garbage() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    assert_eq!(handle_body(&a, "nope", b"{}").await, 404);
    assert_eq!(handle_body(&a, "grafana", b"not json ops@example.com").await, 200);
    let sent = out.bodies();
    assert_eq!(sent[0]["alertsift"]["decision"], "untriaged");
    assert!(!sent[0].to_string().contains("ops@example.com"));
    assert!(a.metrics.render().contains("alertsift_mapping_errors_total{source=\"grafana\"} 1"));
}

#[tokio::test]
async fn http_router_limits_body_size() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("observe", "ping", &be, &out, "").await;
    let a = Arc::new(a);
    let (q, _worker) = alertsift::worker::start(a.clone(), alertsift::worker::QUEUE_CAPACITY);
    let r = alertsift::server::router(a, q);
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
    let dir = tempfile::tempdir().unwrap();
    // A generously long backend timeout: if the handler awaited the backend inline, this
    // request would take the full 5s the fake backend sleeps for.
    let toml = format!(r#"
[backend]
url = "{}"
model = "m-1"
timeout_ms = 10000
[decision]
mode = "observe"
[outputs]
verdict = "{}/verdict"
[store]
undelivered = "{}"
"#, be.url, out.url, dir.path().join("u.jsonl").display());
    let cfg = Config::from_toml(&toml).unwrap();
    let a = App::new(cfg, alertsift::store::Store::memory(), || 1_800_000_000).unwrap();
    let a = Arc::new(a);
    let (q, _worker) = alertsift::worker::start(a.clone(), alertsift::worker::QUEUE_CAPACITY);
    let r = alertsift::server::router(a, q);
    let body = grafana("slow", "f1", "firing");
    let t = Instant::now();
    let resp = r.oneshot(Request::post("/hook/grafana").body(Body::from(body)).unwrap()).await.unwrap();
    let elapsed = t.elapsed();
    assert_eq!(resp.status().as_u16(), 200);
    assert!(elapsed < Duration::from_secs(1), "took {elapsed:?}");
}

#[test]
fn digest_is_due_once_per_day_after_its_time() {
    use alertsift::digest::due;
    let day = 1_790_000_000 - (1_790_000_000 % 86_400); // 00:00 UTC of some day
    assert!(!due("08:00", None, day + 7 * 3600));
    assert!(due("08:00", None, day + 8 * 3600));
    let today = alertsift::digest::day_key(day + 8 * 3600);
    assert!(!due("08:00", Some(&today), day + 9 * 3600));
    assert!(due("08:00", Some(&today), day + 86_400 + 8 * 3600));
}

#[tokio::test]
async fn gate_backend_timeout_pings_untriaged() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await; // backend timeout_ms = 300
    handle_body(&a, "grafana", &grafana("slow", "f1", "firing")).await; // the fake answers after 5 s
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["alertsift"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("backend timed out"), "{}", sent[0]["text"]);
}

#[tokio::test]
async fn gate_backend_quota_429_pings_untriaged() {
    let be = FakeHttp::start(Arc::new(|_| (429, "quota exceeded".into(), 0))).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    handle_body(&a, "grafana", &grafana("p=0.10 disk", "f1", "firing")).await;
    let sent = out.bodies();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["alertsift"]["decision"], "untriaged");
    assert!(sent[0]["text"].as_str().unwrap().contains("HTTP 429"), "{}", sent[0]["text"]);
}

/// Invariant 6 on the shipped `replay`: a gate history recorded by `serve`, replayed with the same
/// config and the same deterministic backend, changes no decision.
#[tokio::test]
async fn replay_of_recorded_gate_history_changes_nothing() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app("gate", "ping", &be, &out, "").await;
    for (summary, fp, status) in [("p=0.90 disk", "f1", "firing"), ("p=0.90 disk", "f1", "firing"), ("p=0.90 disk", "f1", "resolved"),
                                  ("p=0.90 disk", "f1", "firing"), ("p=0.40 cpu", "f2", "firing"), ("p=0.10 queue", "f3", "firing")] {
        handle_body(&a, "grafana", &grafana(summary, fp, status)).await;
    }
    let stored: Vec<String> = a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind).collect();
    assert_eq!(stored, vec!["ping", "repeat", "resolved", "ping", "escalate", "digest"]);
    assert_eq!(alertsift::report::replay(&a, 0).await, vec![]);
}
