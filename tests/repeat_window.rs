//! Repeat suppression is bounded in time: a lost `resolved` must not silence an identity forever.
mod common;
use common::{app, config, grafana, sink, system_one_from_state, FakeHttp, T0};
use keenwake::config::Mode;
use keenwake::server::handle_body;
use keenwake::store::Store;
use std::sync::atomic::{AtomicI64, Ordering};

const HOUR: i64 = 3600;

// One clock per test: tests in this file run in parallel.
static NOW_LOST: AtomicI64 = AtomicI64::new(T0);
fn clock_lost() -> i64 {
    NOW_LOST.load(Ordering::SeqCst)
}
static NOW_WINDOW: AtomicI64 = AtomicI64::new(T0);
fn clock_window() -> i64 {
    NOW_WINDOW.load(Ordering::SeqCst)
}

#[tokio::test]
async fn lost_resolved_does_not_silence_a_new_incident_30_days_later() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), clock_lost);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    // The resolved is never received (restart, send_resolved off, ...). 30 days later, a new incident.
    NOW_LOST.store(T0 + 30 * 24 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.99 disk full", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["ping", "ping"], "the second incident must be sent");
}

#[tokio::test]
async fn repeat_within_the_window_is_suppressed_and_after_it_pings_again() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), clock_window);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    NOW_WINDOW.store(T0 + 23 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["ping"], "a repeat 23 h later stays suppressed (default window 24 h)");
    // 25 h after the only delivered ping: outside the window, so it pings again.
    NOW_WINDOW.store(T0 + 25 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.90 disk full", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["ping", "ping"]);
    let stored: Vec<&str> = a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind.as_str()).collect();
    assert_eq!(stored, vec!["ping", "repeat", "ping"]);
}
