use alertsift::config::{Config, Mode, OnError};
use alertsift::mapping::{extract, FieldSpec};

const MIN: &str = r#"
[backend]
url = "https://api.typesafe.ai"
model = "jev-1.13.0"
"#;

#[test]
fn defaults_are_observe_and_spec_thresholds() {
    let c = Config::from_toml(MIN).unwrap();
    assert_eq!(c.decision.mode, Mode::Observe);
    assert_eq!(c.decision.on_error, OnError::Ping);
    assert_eq!(c.decision.ping, 0.55);
    assert_eq!(c.decision.digest, 0.30);
    assert_eq!(c.decision.digest_at, "08:00");
    assert_eq!(c.backend.timeout_ms, 2000);
    assert_eq!(c.redact.patterns, vec!["email", "ip", "token"]);
    assert!(c.sources.contains_key("grafana"));
    assert!(c.sources.contains_key("alertmanager"));
}

#[test]
fn latest_model_is_refused() {
    let t = MIN.replace("jev-1.13.0", "jev-latest");
    assert!(Config::from_toml(&t).unwrap_err().to_string().contains("pin"));
}

#[test]
fn thresholds_must_be_ordered_and_in_range() {
    let bad = format!("{MIN}\n[decision]\nping = 0.2\ndigest = 0.5\n");
    assert!(Config::from_toml(&bad).is_err());
    let bad = format!("{MIN}\n[decision]\nping = 1.5\n");
    assert!(Config::from_toml(&bad).is_err());
}

#[test]
fn gate_requires_a_ping_output() {
    let bad = format!("{MIN}\n[decision]\nmode = \"gate\"\n");
    assert!(Config::from_toml(&bad).unwrap_err().to_string().contains("outputs.ping"));
}

#[test]
fn bad_digest_time_is_refused() {
    let bad = format!("{MIN}\n[decision]\ndigest_at = \"25:00\"\n");
    assert!(Config::from_toml(&bad).is_err());
}

#[test]
fn preset_override_changes_one_field_only() {
    let t = format!("{MIN}\n[source.grafana]\npreset = \"grafana\"\nfields.env = {{ path = \"/labels/stage\" }}\n");
    let c = Config::from_toml(&t).unwrap();
    let g = &c.sources["grafana"];
    assert_eq!(g.fields.env, Some(FieldSpec::Path { path: "/labels/stage".into(), map: None }));
    assert_eq!(g.fields.identity, Some(FieldSpec::Path { path: "/fingerprint".into(), map: None }));
}

#[test]
fn section_named_like_a_preset_extends_it() {
    let t = format!("{MIN}\n[source.grafana]\nfields.env = {{ const = \"prod\" }}\n");
    let c = Config::from_toml(&t).unwrap();
    assert_eq!(c.sources["grafana"].fields.env, Some(FieldSpec::Const { value: "prod".into() }));
    assert_eq!(c.sources["grafana"].alerts, "/alerts");
}

#[test]
fn custom_source_without_preset() {
    let t = format!(r#"{MIN}
[source.homelab]
alerts = ""
[source.homelab.fields]
status = {{ path = "/state", map = {{ KO = "firing", OK = "resolved" }} }}
identity = {{ path = "/check" }}
summary = {{ template = "{{check}} failed: {{line}}" }}
env = {{ const = "prod" }}
"#);
    let c = Config::from_toml(&t).unwrap();
    let a = &extract("homelab", &c.sources["homelab"], br#"{"state":"OK","check":"b","line":"l"}"#).unwrap()[0];
    assert_eq!(a.summary, "b failed: l");
}

#[test]
fn unknown_preset_and_unknown_keys_fail_at_load() {
    let t = format!("{MIN}\n[source.x]\npreset = \"nope\"\n");
    assert!(Config::from_toml(&t).is_err());
    let t = format!("{MIN}\n[decision]\npingg = 0.5\n");
    assert!(Config::from_toml(&t).is_err());
}

#[test]
fn shipped_presets_read_their_fixtures() {
    let c = Config::from_toml(MIN).unwrap();
    for (name, file) in [("grafana", "tests/fixtures/grafana.json"), ("alertmanager", "tests/fixtures/alertmanager.json")] {
        let alerts = extract(name, &c.sources[name], &std::fs::read(file).unwrap()).unwrap();
        assert!(!alerts.is_empty());
        assert!(alerts.iter().all(|a| a.env == "prod"));
    }
}
