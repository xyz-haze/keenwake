//! The single webhook worker: bodies are decided one at a time, in arrival order, so a firing is
//! fully decided before its resolved is looked at. A panic while processing one body is contained
//! to that body.

use crate::server::{handle_body, send_untriaged_raw, App};
use axum::body::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const QUEUE_CAPACITY: usize = 10_000;
/// Bodies are up to 1 MiB each: the queue is also bounded in bytes, or it could hold 10 GB.
pub const QUEUE_MAX_BYTES: usize = 64 * 1024 * 1024;

type Job = (String, Bytes);

/// The sending side of the webhook queue. The worker ends once every `Queue` clone is dropped
/// and the queued bodies are processed.
#[derive(Clone)]
pub struct Queue {
    tx: mpsc::Sender<Job>,
    bytes: Arc<AtomicUsize>,
    max_bytes: usize,
}

impl Queue {
    /// False if the queue is full, in bodies or in bytes (or the worker is gone): the caller must
    /// not reply 200.
    pub fn try_push(&self, source: String, body: Bytes) -> bool {
        let n = body.len();
        // Reserve before sending, so concurrent handlers cannot overshoot the budget together.
        if self.bytes.fetch_add(n, Ordering::SeqCst) + n > self.max_bytes || self.tx.try_send((source, body)).is_err() {
            self.bytes.fetch_sub(n, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// The receiving side: `run` gives each body's bytes back to the budget once it is processed.
pub struct Jobs {
    rx: mpsc::Receiver<Job>,
    bytes: Arc<AtomicUsize>,
}

/// A queue bounded to `capacity` bodies and `max_bytes` bytes, without a worker; `run` drains it.
pub fn queue(capacity: usize, max_bytes: usize) -> (Queue, Jobs) {
    let (tx, rx) = mpsc::channel(capacity);
    let bytes = Arc::new(AtomicUsize::new(0));
    (Queue { tx, bytes: bytes.clone(), max_bytes }, Jobs { rx, bytes })
}

pub fn start(app: Arc<App>, capacity: usize, max_bytes: usize) -> (Queue, JoinHandle<()>) {
    let (q, rx) = queue(capacity, max_bytes);
    (q, tokio::spawn(run(app, rx)))
}

pub async fn run(app: Arc<App>, mut jobs: Jobs) {
    while let Some((source, body)) = jobs.rx.recv().await {
        let n = body.len();
        process(&app, source, body).await;
        jobs.bytes.fetch_sub(n, Ordering::SeqCst);
    }
}

/// Processes one body in its own task, so a panic there is caught here instead of killing the
/// worker. In gate, the alert then still reaches a human as an untriaged ping.
pub async fn process(app: &Arc<App>, source: String, body: Bytes) {
    let task = tokio::spawn({
        let (app, source, body) = (app.clone(), source.clone(), body.clone());
        async move {
            handle_body(&app, &source, &body).await;
        }
    });
    if let Err(e) = task.await {
        if !e.is_panic() {
            return;
        }
        eprintln!("keenwake: internal error while processing an alert from source {source}");
        app.metrics.inc("keenwake_internal_errors_total", &[]);
        let text = format!("[untriaged] internal error while processing an alert from {source}");
        send_untriaged_raw(app, &source, &body, text).await;
    }
}
