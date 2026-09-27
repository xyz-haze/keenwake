#![no_main]
use keenwake::config::Config;
use keenwake::mapping::extract;
use keenwake::pipeline::prepare;
use keenwake::redact::Redactor;
use keenwake::store::Store;
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

static CFG: OnceLock<Config> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let cfg = CFG.get_or_init(|| Config::from_toml("[backend]\nurl='http://x'\nmodel='m-1'\n").unwrap());
    let red = Redactor::new(&cfg.redact.patterns);
    for (name, spec) in &cfg.sources {
        if let Ok(alerts) = extract(name, spec, data) {
            let store = Store::memory();
            for a in alerts {
                let _ = prepare(&store, &red, a, 1_800_000_000, cfg.decision.repeat_window_secs());
            }
        }
    }
});
