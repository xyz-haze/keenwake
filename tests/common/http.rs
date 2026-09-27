//! Fake HTTP servers on 127.0.0.1 for tests: we mock at the network boundary only.

use axum::{body::Bytes, extract::State, http::StatusCode, routing::post, Router};
use std::sync::{Arc, Mutex};

pub type Responder = Arc<dyn Fn(&serde_json::Value) -> (u16, String, u64) + Send + Sync>;

#[derive(Clone)]
pub struct FakeHttp {
    pub url: String,
    pub received: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl FakeHttp {
    /// `responder(body) -> (status, body, delay_ms)`, keyed by request content, never by call order.
    pub async fn start(responder: Responder) -> FakeHttp {
        let received = Arc::new(Mutex::new(Vec::new()));
        let st = (received.clone(), responder);
        let app = Router::new().fallback(post(handle)).with_state(st);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakeHttp { url, received }
    }

    pub fn bodies(&self) -> Vec<serde_json::Value> {
        self.received.lock().unwrap().clone()
    }

    /// The `keenwake.decision` of every message received, in order.
    pub fn kinds(&self) -> Vec<String> {
        self.bodies().iter().map(|b| b["keenwake"]["decision"].as_str().unwrap().to_string()).collect()
    }
}

async fn handle(
    State((rec, resp)): State<(Arc<Mutex<Vec<serde_json::Value>>>, Responder)>,
    body: Bytes,
) -> (StatusCode, String) {
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    rec.lock().unwrap().push(v.clone());
    let (code, text, delay) = resp(&v);
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    }
    (StatusCode::from_u16(code).unwrap(), text)
}

/// A System One backend whose probability is read from the state: "p=0.72" anywhere in it.
pub fn system_one_from_state() -> Responder {
    Arc::new(|body| {
        let state = body["state"].as_str().unwrap_or("");
        match state.split("p=").nth(1).and_then(|s| s.get(..4)).and_then(|s| s.parse::<f64>().ok()) {
            Some(p) => (
                200,
                serde_json::json!({"model": "fake", "answers": {"page_now": {"type": "noul", "noul": p}},
                                               "usage": {"input_tokens": 100}})
                .to_string(),
                0,
            ),
            None if state.contains("fail500") => (500, "boom".into(), 0),
            None if state.contains("slow") => (200, "{}".into(), 5_000),
            None => (200, r#"{"answers":{}}"#.into(), 0),
        }
    })
}

pub fn sink() -> Responder {
    Arc::new(|_| (200, "ok".into(), 0))
}
