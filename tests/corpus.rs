//! The synthetic corpus, scored two ways. Fast: a no-model rule, the baseline a model has to
//! beat. Slow: a real backend. The slow test is ignored by default; when run with --ignored it
//! needs KEENWAKE_CORPUS_URL and KEENWAKE_CORPUS_MODEL and fails loudly without them, so it
//! never reports a green result it did not compute.

use keenwake::backend::Backend;
use keenwake::config::{BackendCfg, Question};
use keenwake::history::Facts;
use keenwake::mapping::{Alert, Status};
use keenwake::state::sentence;

fn auc(ps: &[(f64, bool)]) -> f64 {
    let pos: Vec<f64> = ps.iter().filter(|x| x.1).map(|x| x.0).collect();
    let neg: Vec<f64> = ps.iter().filter(|x| !x.1).map(|x| x.0).collect();
    let mut wins = 0.0;
    for p in &pos {
        for n in &neg {
            wins += if p > n {
                1.0
            } else if p == n {
                0.5
            } else {
                0.0
            };
        }
    }
    wins / (pos.len() * neg.len()) as f64
}

#[test]
fn auc_helper_is_right() {
    assert_eq!(auc(&[(0.9, true), (0.1, false)]), 1.0);
    assert_eq!(auc(&[(0.1, true), (0.9, false)]), 0.0);
    assert_eq!(auc(&[(0.5, true), (0.5, false)]), 0.5);
}

/// Firing corpus alerts as production would see them, with the expected "page" label.
fn corpus() -> Vec<(Alert, Facts, bool)> {
    let mut out = Vec::new();
    for line in std::fs::read_to_string("tests/fixtures/corpus.jsonl").unwrap().lines() {
        let r: serde_json::Value = serde_json::from_str(line).unwrap();
        if r["status"] != "firing" {
            continue;
        }
        // Only Facts `history::facts` can produce: no median without a past episode. The corpus
        // `ratio` field is ignored, since the sentence no longer states a resolved fraction.
        let episodes = r["episodes"].as_u64().unwrap() as u32;
        let f = Facts {
            env: r["env"].as_str().unwrap().into(),
            firing: true,
            minutes: r["minutes"].as_i64().unwrap(),
            episodes_7d: episodes,
            median_minutes: r["median"].as_i64().filter(|_| episodes > 0),
        };
        let a = Alert {
            source: "corpus".into(),
            status: Status::Firing,
            identity: r["id"].as_str().unwrap().into(),
            summary: r["summary"].as_str().unwrap().into(),
            details: r["details"].as_str().unwrap().into(),
            env: f.env.clone(),
            severity: r["severity"].as_str().unwrap().into(),
        };
        out.push((a, f, r["page"].as_i64().unwrap() == 1));
    }
    out
}

/// The rule restates what the sentence already tells the model: running over 3x its median (or
/// no history at all), plus a point for prod. It was written after looking at the corpus, so it
/// flatters the baseline. The value is pinned so a corpus or rule change shows up here.
#[test]
fn rule_baseline_auc() {
    let scored: Vec<(f64, bool)> = corpus()
        .into_iter()
        .map(|(_, f, page)| {
            let overrun = f.median_minutes.is_none_or(|m| f.minutes > 3 * m);
            (f64::from(u8::from(overrun)) + f64::from(u8::from(f.env == "prod")), page)
        })
        .collect();
    let value = auc(&scored);
    eprintln!("rule baseline AUC = {value:.3} over {} firing alerts", scored.len());
    assert!((value - 0.931).abs() < 0.0005, "rule baseline AUC moved: {value:.3}");
}

#[tokio::test]
#[ignore = "slow: needs a real backend, run with --ignored"]
async fn corpus_auc_stays_above_095() {
    let (Ok(url), Ok(model)) = (std::env::var("KEENWAKE_CORPUS_URL"), std::env::var("KEENWAKE_CORPUS_MODEL")) else {
        eprintln!("SKIPPED: set KEENWAKE_CORPUS_URL and KEENWAKE_CORPUS_MODEL");
        panic!("slow corpus test was asked for but not configured");
    };
    let key_env = std::env::var("KEENWAKE_CORPUS_KEY_ENV").ok();
    let b = Backend::new(&BackendCfg { url, model, api_key_env: key_env, timeout_ms: 30_000 }).unwrap();
    let mut scored = Vec::new();
    for (a, f, page) in corpus() {
        let p = b.ask(&sentence(&a, &f), &Question::default()).await.expect("backend answer").probability;
        scored.push((p, page));
    }
    let value = auc(&scored);
    eprintln!("corpus AUC = {value:.3} over {} firing alerts", scored.len());
    assert!(value >= 0.95, "AUC {value:.3} < 0.95");
}
