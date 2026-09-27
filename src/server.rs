//! HTTP entry: /hook/{source}, /metrics, /healthz.

use crate::backend::Backend;
use crate::config::{Config, Mode};
use crate::decide::{Kind, Target};
use crate::mapping::extract;
use crate::metrics::Metrics;
use crate::output::{message, Sender};
use crate::pipeline::{facts_line, finish, prepare};
use crate::redact::Redactor;
use crate::store::{DecisionRow, Store};
use crate::worker::Queue;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use std::sync::Arc;

pub const MAX_BODY: usize = 1024 * 1024;

pub fn now_utc() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub struct App {
    pub cfg: Config,
    pub store: Store,
    pub backend: Backend,
    pub sender: Sender,
    pub redactor: Redactor,
    pub metrics: Metrics,
    pub clock: fn() -> i64,
}

impl App {
    pub fn new(cfg: Config, store: Store, clock: fn() -> i64) -> anyhow::Result<App> {
        Ok(App {
            backend: Backend::new(&cfg.backend)?,
            sender: Sender::new(cfg.store.undelivered.clone(), 200),
            redactor: Redactor::new(&cfg.redact.patterns),
            metrics: Metrics::default(),
            store,
            cfg,
            clock,
        })
    }

    fn url(&self, t: Target) -> Option<&str> {
        let o = &self.cfg.outputs;
        match t {
            Target::Ping => o.ping.as_deref(),
            Target::Escalate => o.escalate.as_deref(),
            Target::Verdict => o.verdict.as_deref(),
            Target::DigestQueue | Target::Nothing => None,
        }
    }

    pub(crate) async fn send(&self, t: Target, mut body: serde_json::Value) -> bool {
        let Some(url) = self.url(t) else { return false };
        body["keenwake"]["channel"] = serde_json::json!(match t {
            Target::Verdict => "verdict",
            _ => "team",
        });
        let ok = self.sender.post(url, &body).await;
        if !ok {
            self.metrics.inc("keenwake_undelivered_total", &[]);
        }
        ok
    }
}

/// Gate only: sends the redacted raw body (first 4000 characters) to `outputs.ping` as
/// `untriaged`, for an alert keenwake could not decide on its own.
pub(crate) async fn send_untriaged_raw(app: &App, source: &str, body: &[u8], text: String) {
    if app.cfg.decision.mode != Mode::Gate {
        return;
    }
    let raw: String = app.redactor.clean(&String::from_utf8_lossy(body)).chars().take(4000).collect();
    let msg = serde_json::json!({"text": text, "keenwake": {"decision": "untriaged", "source": source, "raw": raw}});
    app.send(Target::Ping, msg).await;
}

pub async fn handle_body(app: &App, source: &str, body: &[u8]) -> u16 {
    let Some(spec) = app.cfg.sources.get(source) else {
        app.metrics.inc("keenwake_unknown_source_total", &[]);
        return 404;
    };
    let alerts = match extract(source, spec, body) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("keenwake: mapping error from source {source}: {e}");
            app.metrics.inc("keenwake_mapping_errors_total", &[("source", source)]);
            send_untriaged_raw(app, source, body, format!("[untriaged] unreadable alert from {source}: {e}")).await;
            return 200;
        }
    };
    for alert in alerts {
        app.metrics.inc("keenwake_alerts_total", &[("source", source)]);
        let now = (app.clock)();
        let p = prepare(&app.store, &app.redactor, alert, now, app.cfg.decision.repeat_window_secs());
        let mode = app.cfg.decision.mode;
        if !p.needs_model {
            let delivered = if app.cfg.decision.mode == Mode::Gate && p.already_pinged {
                app.send(Target::Ping, message(Kind::Resolved, &p.alert, None, "", "")).await
            } else {
                false
            };
            app.store.insert_decision(&DecisionRow {
                event_seq: p.event_seq,
                decided_at: now,
                mode,
                kind: Kind::Resolved,
                probability: None,
                reason: String::new(),
                delivered,
                backend_ms: None,
                input_tokens: None,
            });
            continue;
        }
        let (outcome, ms, tokens, reason) = match app.backend.ask(&p.state, &app.cfg.question).await {
            Ok(a) => {
                app.metrics.observe_ms("keenwake_backend", a.ms);
                (Ok(a.probability), Some(a.ms), a.input_tokens, String::new())
            }
            Err(e) => {
                app.metrics.inc("keenwake_backend_errors_total", &[]);
                (Err(e.to_string()), None, None, format!("not triaged: backend unavailable ({e})"))
            }
        };
        let prob = outcome.as_ref().ok().copied();
        let r = finish(&app.cfg, &p, outcome);
        app.metrics.inc("keenwake_decisions_total", &[("kind", r.kind.as_str())]);
        let delivered = match r.target {
            Target::DigestQueue => {
                app.store.queue_digest(p.event_seq);
                false
            }
            Target::Nothing => false,
            t => app.send(t, message(r.kind, &p.alert, prob, &reason, &facts_line(&p.facts))).await,
        };
        let delivered = delivered && r.kind != Kind::Repeat;
        app.store.insert_decision(&DecisionRow {
            event_seq: p.event_seq,
            decided_at: now,
            mode,
            kind: r.kind,
            probability: prob,
            reason,
            delivered: delivered && r.target != Target::Verdict,
            backend_ms: ms,
            input_tokens: tokens,
        });
    }
    200
}

#[derive(Clone)]
struct Http {
    app: Arc<App>,
    queue: Queue,
}

/// Replies before the alert is processed: the body is queued for the single worker, so a slow
/// backend never makes the source wait, and alerts are decided in arrival order. Unknown sources
/// are checked synchronously (404); a full queue replies 503 so the source retries later.
async fn hook(State(h): State<Http>, Path(source): Path<String>, body: Bytes) -> StatusCode {
    if !h.app.cfg.sources.contains_key(&source) {
        h.app.metrics.inc("keenwake_unknown_source_total", &[]);
        return StatusCode::NOT_FOUND;
    }
    if h.queue.try_push(source, body) {
        StatusCode::OK
    } else {
        h.app.metrics.inc("keenwake_queue_full_total", &[]);
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// The HTTP routes. Webhook bodies go to `queue`, drained by `worker::run`.
pub fn router(app: Arc<App>, queue: Queue) -> Router {
    Router::new()
        .route("/hook/{source}", post(hook))
        .route("/metrics", get(|State(h): State<Http>| async move { h.app.metrics.render() }))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(Http { app, queue })
}
