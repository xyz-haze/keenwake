//! The single webhook worker: bodies are decided one at a time, in arrival order, so a firing is
//! fully decided before its resolved is looked at. A panic while processing one body is contained
//! to that body.

use crate::server::{handle_body, send_untriaged_raw, App};
use axum::body::Bytes;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const QUEUE_CAPACITY: usize = 10_000;

type Job = (String, Bytes);

/// The sending side of the webhook queue. The worker ends once every `Queue` clone is dropped
/// and the queued bodies are processed.
#[derive(Clone)]
pub struct Queue(mpsc::Sender<Job>);

impl Queue {
    /// False if the queue is full (or the worker is gone): the caller must not reply 200.
    pub fn try_push(&self, source: String, body: Bytes) -> bool {
        self.0.try_send((source, body)).is_ok()
    }
}

/// A bounded queue without a worker; `run` drains the receiver.
pub fn queue(capacity: usize) -> (Queue, mpsc::Receiver<Job>) {
    let (tx, rx) = mpsc::channel(capacity);
    (Queue(tx), rx)
}

/// A bounded queue and the worker task draining it.
pub fn start(app: Arc<App>, capacity: usize) -> (Queue, JoinHandle<()>) {
    let (q, rx) = queue(capacity);
    (q, tokio::spawn(run(app, rx)))
}

pub async fn run(app: Arc<App>, mut rx: mpsc::Receiver<Job>) {
    while let Some((source, body)) = rx.recv().await {
        process(&app, source, body).await;
    }
}

/// Processes one body in its own task, so a panic there is caught here instead of killing the
/// worker. In gate, the alert then still reaches a human as an untriaged ping.
pub async fn process(app: &Arc<App>, source: String, body: Bytes) {
    let task = tokio::spawn({
        let (app, source, body) = (app.clone(), source.clone(), body.clone());
        async move { handle_body(&app, &source, &body).await; }
    });
    if let Err(e) = task.await {
        if !e.is_panic() { return; }
        eprintln!("keenwake: internal error while processing an alert from source {source}");
        app.metrics.inc("keenwake_internal_errors_total", &[]);
        let text = format!("[untriaged] internal error while processing an alert from {source}");
        send_untriaged_raw(app, &source, &body, text).await;
    }
}
