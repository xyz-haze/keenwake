#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

static CFG: OnceLock<keenwake::config::Config> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let cfg =
        CFG.get_or_init(|| keenwake::config::Config::from_toml("[backend]\nurl='http://x'\nmodel='m-1'\n").unwrap());
    let red = keenwake::redact::Redactor::new(&cfg.redact.patterns);
    for (name, spec) in &cfg.sources {
        if let Ok(alerts) = keenwake::mapping::extract(name, spec, data) {
            let store = keenwake::store::Store::memory();
            for a in alerts {
                let _ = keenwake::pipeline::prepare(&store, &red, a, 1_800_000_000, cfg.decision.repeat_window_secs());
            }
        }
    }
});
