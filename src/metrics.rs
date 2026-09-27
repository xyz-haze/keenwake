//! Minimal Prometheus text exposition: counters and a latency sum/count. No dependency needed.

use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Metrics {
    counters: Mutex<BTreeMap<String, u64>>,
}

fn key(name: &str, labels: &[(&str, &str)]) -> String {
    if labels.is_empty() {
        return name.to_string();
    }
    let l: Vec<String> =
        labels.iter().map(|(k, v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))).collect();
    format!("{name}{{{}}}", l.join(","))
}

impl Metrics {
    pub fn inc(&self, name: &str, labels: &[(&str, &str)]) {
        self.add(name, labels, 1);
    }

    pub fn add(&self, name: &str, labels: &[(&str, &str)], n: u64) {
        *self.counters.lock().expect("metrics").entry(key(name, labels)).or_insert(0) += n;
    }

    pub fn observe_ms(&self, name: &str, ms: i64) {
        self.add(&format!("{name}_ms_sum"), &[], ms.max(0) as u64);
        self.inc(&format!("{name}_ms_count"), &[]);
    }

    pub fn render(&self) -> String {
        self.counters.lock().expect("metrics").iter().map(|(k, v)| format!("{k} {v}\n")).collect()
    }
}
