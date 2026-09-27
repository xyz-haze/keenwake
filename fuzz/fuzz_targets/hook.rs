#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

static CFG: OnceLock<alertsift::config::Config> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let cfg = CFG.get_or_init(|| alertsift::config::Config::from_toml("[backend]\nurl='http://x'\nmodel='m-1'\n").unwrap());
    let red = alertsift::redact::Redactor::new(&cfg.redact.patterns);
    for (name, spec) in &cfg.sources {
        if let Ok(alerts) = alertsift::mapping::extract(name, spec, data) {
            let store = alertsift::store::Store::memory();
            for a in alerts { let _ = alertsift::pipeline::prepare(&store, &red, a, 1_800_000_000, cfg.decision.repeat_window_secs()); }
        }
    }
});
