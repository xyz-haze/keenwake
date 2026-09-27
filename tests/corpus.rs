//! Slow: sends the synthetic corpus to a real backend. Ignored by default; when run with
//! --ignored it needs ALERTSIFT_CORPUS_URL and ALERTSIFT_CORPUS_MODEL and fails loudly without
//! them, so it never reports a green result it did not compute.

use alertsift::backend::Backend;
use alertsift::config::{BackendCfg, Question};
use alertsift::history::Facts;
use alertsift::mapping::{Alert, Status};
use alertsift::state::sentence;

fn auc(ps: &[(f64, bool)]) -> f64 {
    let pos: Vec<f64> = ps.iter().filter(|x| x.1).map(|x| x.0).collect();
    let neg: Vec<f64> = ps.iter().filter(|x| !x.1).map(|x| x.0).collect();
    let mut wins = 0.0;
    for p in &pos { for n in &neg { wins += if p > n { 1.0 } else if p == n { 0.5 } else { 0.0 }; } }
    wins / (pos.len() * neg.len()) as f64
}

#[test]
fn auc_helper_is_right() {
    assert_eq!(auc(&[(0.9, true), (0.1, false)]), 1.0);
    assert_eq!(auc(&[(0.1, true), (0.9, false)]), 0.0);
    assert_eq!(auc(&[(0.5, true), (0.5, false)]), 0.5);
}

#[tokio::test]
#[ignore = "slow: needs a real backend, run with --ignored"]
async fn corpus_auc_stays_above_095() {
    let (Ok(url), Ok(model)) = (std::env::var("ALERTSIFT_CORPUS_URL"), std::env::var("ALERTSIFT_CORPUS_MODEL")) else {
        eprintln!("SKIPPED: set ALERTSIFT_CORPUS_URL and ALERTSIFT_CORPUS_MODEL");
        panic!("slow corpus test was asked for but not configured");
    };
    let key_env = std::env::var("ALERTSIFT_CORPUS_KEY_ENV").ok();
    let b = Backend::new(&BackendCfg { url, model, api_key_env: key_env, timeout_ms: 30_000 }).unwrap();
    let mut scored = Vec::new();
    for line in std::fs::read_to_string("tests/fixtures/corpus.jsonl").unwrap().lines() {
        let r: serde_json::Value = serde_json::from_str(line).unwrap();
        if r["status"] != "firing" { continue; }
        let episodes = r["episodes"].as_u64().unwrap() as u32;
        let f = Facts { env: r["env"].as_str().unwrap().into(), firing: true, minutes: r["minutes"].as_i64().unwrap(),
            episodes_7d: episodes, resolved_7d: (episodes as f64 * r["ratio"].as_f64().unwrap()).round() as u32,
            median_minutes: r["median"].as_i64() };
        let a = Alert { source: "corpus".into(), status: Status::Firing, identity: r["id"].as_str().unwrap().into(),
            summary: r["summary"].as_str().unwrap().into(), details: r["details"].as_str().unwrap().into(),
            env: f.env.clone(), severity: r["severity"].as_str().unwrap().into() };
        let p = b.ask(&sentence(&a, &f), &Question::default()).await.expect("backend answer").probability;
        scored.push((p, r["page"].as_i64().unwrap() == 1));
    }
    let value = auc(&scored);
    eprintln!("corpus AUC = {value:.3} over {} firing alerts", scored.len());
    assert!(value >= 0.95, "AUC {value:.3} < 0.95");
}
