mod common;
use keenwake::backend::{request_body, Backend, BackendError};
use keenwake::config::{BackendCfg, Question};
use common::{system_one_from_state, FakeHttp};

fn cfg(url: &str) -> BackendCfg {
    BackendCfg { url: url.into(), model: "jev-1.13.0".into(), api_key_env: None, timeout_ms: 300 }
}

#[test]
fn body_matches_the_system_one_format() {
    let q = Question::default();
    let b = request_body("jev-1.13.0", "hello", &q);
    assert_eq!(b["model"], "jev-1.13.0");
    assert_eq!(b["state"], "hello");
    assert_eq!(b["questions"]["page_now"]["type"], "noul");
    assert_eq!(b["questions"]["page_now"]["instructions"], q.instructions.as_str());
    assert_eq!(b["questions"]["page_now"]["criteria"]["true"], q.criteria_true.as_str());
    assert_eq!(b["questions"]["page_now"]["criteria"]["false"], q.criteria_false.as_str());
}

#[tokio::test]
async fn reads_the_probability() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let a = Backend::new(&cfg(&fake.url)).unwrap().ask("x p=0.72", &Question::default()).await.unwrap();
    assert_eq!(a.probability, 0.72);
    assert_eq!(a.input_tokens, Some(100));
    assert_eq!(fake.bodies()[0]["state"], "x p=0.72");
}

#[tokio::test]
async fn errors_are_typed() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let b = Backend::new(&cfg(&fake.url)).unwrap();
    assert!(matches!(b.ask("fail500", &Question::default()).await, Err(BackendError::Status(500))));
    assert!(matches!(b.ask("slow", &Question::default()).await, Err(BackendError::Timeout)));
    assert!(matches!(b.ask("no answer", &Question::default()).await, Err(BackendError::Invalid(_))));
}

#[tokio::test]
async fn out_of_range_probability_is_invalid() {
    let fake = FakeHttp::start(system_one_from_state()).await;
    let b = Backend::new(&cfg(&fake.url)).unwrap();
    assert!(matches!(b.ask("p=1.50", &Question::default()).await, Err(BackendError::Invalid(_))));
}

#[tokio::test]
async fn unreachable_backend_is_transport_error() {
    let b = Backend::new(&cfg("http://127.0.0.1:9")).unwrap();
    assert!(matches!(b.ask("x", &Question::default()).await, Err(BackendError::Transport(_)) | Err(BackendError::Timeout)));
}
