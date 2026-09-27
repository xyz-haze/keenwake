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

static NOW_ESC: AtomicI64 = AtomicI64::new(T0);
fn clock_esc() -> i64 {
    NOW_ESC.load(Ordering::SeqCst)
}

fn stored(a: &keenwake::server::App) -> Vec<&'static str> {
    a.store.decisions_since(0).into_iter().map(|(_, d)| d.kind.as_str()).collect()
}

/// Alertmanager re-sends a firing group every repeat_interval: an escalation must not be re-sent
/// each time, but a new one goes out once the window has passed.
#[tokio::test]
async fn repeated_escalate_is_suppressed_within_the_window() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), clock_esc);
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "firing")).await;
    NOW_ESC.store(T0 + 4 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["escalate"]);
    NOW_ESC.store(T0 + 25 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["escalate", "escalate"]);
    assert_eq!(stored(&a), vec!["escalate", "repeat", "escalate"]);
}

/// Urgency order: ping (and an untriaged ping) > escalate > digest. Only a strictly more urgent
/// decision gets through a notification already sent in the episode.
#[tokio::test]
async fn only_a_more_urgent_decision_gets_through() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), || T0);
    for p in ["0.40", "0.90", "0.40", "0.10", "0.90"] {
        handle_body(&a, "grafana", &grafana(&format!("p={p} cpu"), "f1", "firing")).await;
    }
    handle_body(&a, "grafana", &grafana("fail500", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["escalate", "ping"]);
    assert_eq!(stored(&a), vec!["escalate", "ping", "repeat", "repeat", "repeat", "repeat"]);
    assert!(a.store.take_digest().is_empty(), "a digest after a ping is a repeat, not queued");
}

/// An untriaged ping (backend down) after an escalation still reaches a human: when in doubt, ping.
#[tokio::test]
async fn untriaged_after_escalate_pings() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), || T0);
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("fail500 cpu", "f1", "firing")).await;
    assert_eq!(out.kinds(), vec!["escalate", "untriaged"]);
}

/// The same identity and episode is queued for the digest once, however often it repeats.
#[tokio::test]
async fn digest_is_queued_once_per_episode() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let (a, _d) = app(config(Mode::Gate, &be.url, &out.url), Store::memory(), clock_digest);
    handle_body(&a, "grafana", &grafana("p=0.10 queue", "f1", "firing")).await;
    NOW_DIGEST.store(T0 + 30 * HOUR, Ordering::SeqCst);
    handle_body(&a, "grafana", &grafana("p=0.10 queue", "f1", "firing")).await;
    assert_eq!(a.store.take_digest().len(), 1);
    assert_eq!(stored(&a), vec!["digest", "repeat"]);
    // A new episode is queued again.
    handle_body(&a, "grafana", &grafana("p=0.10 queue", "f1", "resolved")).await;
    handle_body(&a, "grafana", &grafana("p=0.10 queue", "f1", "firing")).await;
    assert_eq!(a.store.take_digest().len(), 1);
    assert!(out.kinds().is_empty(), "a digest-only episode sends no resolved either");
}
static NOW_DIGEST: AtomicI64 = AtomicI64::new(T0);
fn clock_digest() -> i64 {
    NOW_DIGEST.load(Ordering::SeqCst)
}

/// A resolved follows a delivered escalation, to the escalation's output.
#[tokio::test]
async fn resolved_follows_an_escalation_to_its_output() {
    let be = FakeHttp::start(system_one_from_state()).await;
    let out = FakeHttp::start(sink()).await;
    let esc = FakeHttp::start(sink()).await;
    let mut cfg = config(Mode::Gate, &be.url, &out.url);
    cfg.outputs.escalate = Some(format!("{}/escalate", esc.url));
    let (a, _d) = app(cfg, Store::memory(), || T0);
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "firing")).await;
    handle_body(&a, "grafana", &grafana("p=0.40 cpu", "f1", "resolved")).await;
    assert_eq!(esc.kinds(), vec!["escalate", "resolved"]);
    assert!(out.kinds().is_empty());
}
