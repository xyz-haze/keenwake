//! System One client. Jev (hosted) and the Laya sidecar (local) share this API.

use crate::config::{BackendCfg, Question};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub const QUESTION_ID: &str = "page_now";

pub fn request_body(model: &str, state: &str, q: &Question) -> Value {
    json!({
        "model": model,
        "state": state,
        "questions": { QUESTION_ID: {
            "type": "noul",
            "instructions": q.instructions,
            "criteria": { "true": q.criteria_true, "false": q.criteria_false },
        }},
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer { pub probability: f64, pub input_tokens: Option<i64>, pub ms: i64 }

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BackendError {
    #[error("backend timed out")]
    Timeout,
    #[error("backend returned HTTP {0}")]
    Status(u16),
    #[error("backend answer is invalid: {0}")]
    Invalid(String),
    #[error("backend unreachable: {0}")]
    Transport(String),
}

pub struct Backend { client: reqwest::Client, endpoint: String, model: String, api_key: Option<String> }

impl Backend {
    pub fn new(cfg: &BackendCfg) -> anyhow::Result<Backend> {
        let client = reqwest::Client::builder().timeout(Duration::from_millis(cfg.timeout_ms)).build()?;
        let api_key = match &cfg.api_key_env {
            Some(var) => Some(std::env::var(var).map_err(|_| anyhow::anyhow!("environment variable {var} is not set"))?),
            None => None,
        };
        Ok(Backend { client, endpoint: format!("{}/v1/systemone", cfg.url.trim_end_matches('/')), model: cfg.model.clone(), api_key })
    }

    pub fn model(&self) -> &str { &self.model }

    pub async fn ask(&self, state: &str, q: &Question) -> Result<Answer, BackendError> {
        let t = Instant::now();
        let mut req = self.client.post(&self.endpoint).json(&request_body(&self.model, state, q));
        if let Some(k) = &self.api_key { req = req.bearer_auth(k); }
        let resp = req.send().await.map_err(|e| if e.is_timeout() { BackendError::Timeout } else { BackendError::Transport(e.to_string()) })?;
        if !resp.status().is_success() { return Err(BackendError::Status(resp.status().as_u16())); }
        let body: Value = resp.json().await.map_err(|e| if e.is_timeout() { BackendError::Timeout } else { BackendError::Invalid(e.to_string()) })?;
        let p = body["answers"][QUESTION_ID]["noul"].as_f64().ok_or_else(|| BackendError::Invalid("no answers.page_now.noul".into()))?;
        if !p.is_finite() || !(0.0..=1.0).contains(&p) { return Err(BackendError::Invalid(format!("probability {p} out of range"))); }
        Ok(Answer { probability: p, input_tokens: body["usage"]["input_tokens"].as_i64(), ms: t.elapsed().as_millis() as i64 })
    }
}
