mod common;
use common::{alert, sink, FakeHttp};
use keenwake::decide::Kind;
use keenwake::mapping::{Alert, Status};
use keenwake::output::{message, Sender};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn message_has_text_and_structured_part() {
    let a = Alert { identity: "id1".into(), summary: "Disk full".into(), ..alert(Status::Firing) };
    let m = message(Kind::Ping, &a, Some(0.82), "", "fired 3 times in 7 days");
    assert!(m["text"].as_str().unwrap().contains("Disk full"));
    assert!(m["text"].as_str().unwrap().contains("0.82"));
    assert_eq!(m["keenwake"]["decision"], "ping");
    assert_eq!(m["keenwake"]["identity"], "id1");
    assert_eq!(m["keenwake"]["probability"], 0.82);
}

#[tokio::test]
async fn delivers_on_success() {
    let fake = FakeHttp::start(sink()).await;
    let dir = tempfile::tempdir().unwrap();
    let s = Sender::new(dir.path().join("u.jsonl").to_str().unwrap().into(), 1).unwrap();
    assert!(s.post(&fake.url, &serde_json::json!({"a": 1})).await);
    assert_eq!(fake.bodies(), vec![serde_json::json!({"a": 1})]);
}

#[tokio::test]
async fn retries_then_writes_undelivered() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let fake = FakeHttp::start(Arc::new(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        (503, "".into(), 0)
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.jsonl");
    let s = Sender::new(path.to_str().unwrap().into(), 1).unwrap();
    assert!(!s.post(&fake.url, &serde_json::json!({"a": 1})).await);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let line: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(line["body"], serde_json::json!({"a": 1}));
    assert_eq!(line["url"], fake.url.as_str());
}

#[tokio::test]
async fn second_attempt_success_counts_as_delivered() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let fake = FakeHttp::start(Arc::new(move |_| {
        if c.fetch_add(1, Ordering::SeqCst) == 0 {
            (500, "".into(), 0)
        } else {
            (200, "".into(), 0)
        }
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let s = Sender::new(dir.path().join("u.jsonl").to_str().unwrap().into(), 1).unwrap();
    assert!(s.post(&fake.url, &serde_json::json!({})).await);
    assert!(!dir.path().join("u.jsonl").exists());
}

/// Undelivered lines hold webhook URLs, which often embed a secret: owner-only file.
#[cfg(unix)]
#[test]
fn undelivered_file_is_created_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.jsonl");
    let s = Sender::new(path.to_str().unwrap().into(), 1).unwrap();
    s.write_undelivered("https://hooks.slack.com/services/T0/B0/secret", &serde_json::json!({}));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "mode {mode:o}");
}
