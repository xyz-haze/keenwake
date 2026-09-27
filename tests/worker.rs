//! The webhook queue: one ordered worker, bounded, panic-isolated, drained on shutdown.
mod common;
use axum::body::Body;
use axum::http::Request;
use common::{alert, app, config, grafana, sink, system_one_from_state, FakeHttp, T0};
use keenwake::config::Mode;
use keenwake::mapping::{Alert, Status};
use keenwake::server::{router, App};
use keenwake::store::Store;
use keenwake::worker::{queue, start, QUEUE_CAPACITY};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

/// An app in `mode` on `store`, shared with the worker.
fn shared_app(mode: Mode, be: &FakeHttp, out: &FakeHttp, store: Store) -> (Arc<App>, TempDir) {
    let (a, dir) = app(config(mode, &be.url, &out.url), store, || T0);
    (Arc::new(a), dir)
}

async fn post(r: &axum::Router, source: &str, body: Vec<u8>) -> u16 {
    r.clone()
        .oneshot(Request::post(format!("/hook/{source}")).body(Body::from(body)).unwrap())
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Polls until `out` has received `n` bodies, or 5 s have passed.
async fn wait_for(out: &FakeHttp, n: usize) {
    for _ in 0..250 {
        if out.bodies().len() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A file store whose inserts of an event with `boom` in its summary abort, so processing that
/// alert panics inside the store (the lock is held at that moment and gets poisoned).
fn store_with_trigger(dir: &tempfile::TempDir, trigger: &str) -> Store {
    let path = dir.path().join("a.db");
    let s = Store::open(path.to_str().unwrap()).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(trigger).unwrap();
    s
}

const BOOM_ON_EVENT: &str =
    "CREATE TRIGGER boom BEFORE INSERT ON events WHEN NEW.summary LIKE '%boom%' BEGIN SELECT RAISE(ABORT, 'forced'); END;";

/// A store call that panics (here: a SQL trigger aborting the insert) poisons the store's lock;
/// the store must still serve the next call.
#[test]
fn store_recovers_after_a_panic_in_a_previous_call() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.db");
    let s = Store::open(path.to_str().unwrap()).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(
        "CREATE TRIGGER boom BEFORE INSERT ON events WHEN NEW.summary = 'boom' BEGIN SELECT RAISE(ABORT, 'forced'); END;").unwrap();
    let named = |summary: &str| Alert { summary: summary.into(), ..alert(Status::Firing) };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.insert_event(&named("boom"), 1)));
    assert!(r.is_err(), "the trigger must make insert_event panic");
    s.insert_event(&named("fine"), 2);
    assert_eq!(s.events_since(0).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolved_during_backend_call_follows_its_ping() {
    let be = FakeHttp::start(Arc::new(|_| {
        (200, serde_json::json!({"answers": {"page_now": {"noul": 0.9}}}).to_string(), 500)
    }))
    .await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = shared_app(Mode::Gate, &be, &out, Store::memory());
    let (q, _worker) = start(a.clone(), QUEUE_CAPACITY);
    let r = router(a.clone(), q);
    r.clone()
        .oneshot(Request::post("/hook/grafana").body(Body::from(grafana("disk", "f9", "firing"))).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    r.oneshot(Request::post("/hook/grafana").body(Body::from(grafana("disk", "f9", "resolved"))).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert_eq!(out.kinds(), vec!["ping", "resolved"]);
}

#[tokio::test]
async fn full_queue_replies_503_and_unknown_source_stays_404() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = shared_app(Mode::Gate, &be, &out, Store::memory());
    let (q, _rx) = queue(1); // no worker: nothing drains it
    let r = router(a.clone(), q);
    assert_eq!(post(&r, "grafana", grafana("p=0.90 a", "f1", "firing")).await, 200);
    assert_eq!(post(&r, "grafana", grafana("p=0.90 b", "f2", "firing")).await, 503);
    assert_eq!(post(&r, "nope", b"{}".to_vec()).await, 404);
    let m = a.metrics.render();
    assert!(m.contains("keenwake_queue_full_total 1"), "{m}");
    assert!(m.contains("keenwake_unknown_source_total 1"), "{m}");
}

#[tokio::test]
async fn panic_while_processing_sends_untriaged_in_gate_and_the_next_webhook_still_works() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let dir = tempfile::tempdir().unwrap();
    let (a, _d) = shared_app(Mode::Gate, &be, &out, store_with_trigger(&dir, BOOM_ON_EVENT));
    let (q, _worker) = start(a.clone(), QUEUE_CAPACITY);
    let r = router(a.clone(), q);
    assert_eq!(post(&r, "grafana", grafana("boom p=0.10 mail ops@example.com", "f1", "firing")).await, 200);
    assert_eq!(post(&r, "grafana", grafana("p=0.90 disk", "f2", "firing")).await, 200);
    wait_for(&out, 2).await;
    let sent = out.bodies();
    assert_eq!(out.kinds(), vec!["untriaged", "ping"]);
    assert_eq!(sent[0]["text"], "[untriaged] internal error while processing an alert from grafana");
    let raw = sent[0]["keenwake"]["raw"].as_str().unwrap();
    assert!(raw.contains("boom p=0.10"), "{raw}");
    assert!(!raw.contains("ops@example.com"), "raw body must be redacted: {raw}");
    assert_eq!(sent[1]["keenwake"]["identity"], "f2");
    assert!(a.metrics.render().contains("keenwake_internal_errors_total 1"));
}

#[tokio::test]
async fn panic_while_processing_in_observe_is_only_counted() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let dir = tempfile::tempdir().unwrap();
    let (a, _d) = shared_app(Mode::Observe, &be, &out, store_with_trigger(&dir, BOOM_ON_EVENT));
    let (q, _worker) = start(a.clone(), QUEUE_CAPACITY);
    let r = router(a.clone(), q);
    post(&r, "grafana", grafana("boom", "f1", "firing")).await;
    post(&r, "grafana", grafana("p=0.90 disk", "f2", "firing")).await;
    wait_for(&out, 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let sent = out.bodies();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["keenwake"]["channel"], "verdict");
    assert_eq!(sent[0]["keenwake"]["identity"], "f2");
    assert!(a.metrics.render().contains("keenwake_internal_errors_total 1"));
}

#[tokio::test]
async fn worker_drains_queued_webhooks_once_the_router_is_dropped() {
    let be = FakeHttp::start(Arc::new(|_| {
        (200, serde_json::json!({"answers": {"page_now": {"noul": 0.1}}}).to_string(), 100)
    }))
    .await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = shared_app(Mode::Gate, &be, &out, Store::memory());
    let (q, worker) = start(a.clone(), QUEUE_CAPACITY);
    let r = router(a.clone(), q);
    for i in 0..3 {
        assert_eq!(post(&r, "grafana", grafana("disk", &format!("f{i}"), "firing")).await, 200);
    }
    drop(r);
    tokio::time::timeout(std::time::Duration::from_secs(5), worker).await.expect("worker ends").unwrap();
    assert_eq!(a.store.decisions_since(0).len(), 3, "every queued webhook was decided before the worker ended");
}

#[tokio::test]
async fn digest_tick_panic_is_contained() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let dir = tempfile::tempdir().unwrap();
    let s = store_with_trigger(
        &dir,
        "CREATE TRIGGER boom BEFORE DELETE ON digest_queue BEGIN SELECT RAISE(ABORT, 'forced'); END;",
    );
    s.queue_digest(s.insert_event(&alert(Status::Firing), T0));
    let (a, _d) = shared_app(Mode::Gate, &be, &out, s);
    let day = 1_800_000_000 - 1_800_000_000 % 86_400;
    assert!(!keenwake::digest::guarded_tick(&a, day + 9 * 3600).await);
    assert!(a.metrics.render().contains("keenwake_internal_errors_total 1"));
    assert!(a.store.meta_get("digest_last_day").is_some(), "the store still answers after the panic");
}
