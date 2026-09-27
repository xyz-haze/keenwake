use keenwake::mapping::{default_identity, extract, FieldSpec, Fields, SourceSpec, Status};
use std::collections::BTreeMap;

fn p(path: &str) -> FieldSpec {
    FieldSpec::Path { path: path.into(), map: None }
}

fn grafana_like() -> SourceSpec {
    SourceSpec {
        alerts: "/alerts".into(),
        fields: Fields {
            status: p("/status"),
            identity: Some(p("/fingerprint")),
            summary: FieldSpec::FirstOf {
                first_of: vec!["/annotations/summary".into(), "/labels/alertname".into()],
                map: None,
            },
            details: Some(p("/annotations/description")),
            env: Some(p("/labels/env")),
            severity: Some(p("/labels/severity")),
        },
    }
}

#[test]
fn grafana_fixture_yields_one_alert_per_entry() {
    let body = std::fs::read("tests/fixtures/grafana.json").unwrap();
    let alerts = extract("grafana", &grafana_like(), &body).unwrap();
    assert_eq!(alerts.len(), 2);
    assert_eq!(alerts[0].status, Status::Firing);
    assert_eq!(alerts[0].identity, "77de1aae9f2d6621");
    assert_eq!(alerts[0].summary, "CPU above 90% on etl-2");
    assert_eq!(alerts[0].details, "CPU at 92% for 2m");
    assert_eq!(alerts[0].env, "prod");
    assert_eq!(alerts[0].severity, "critical");
    assert_eq!(alerts[1].status, Status::Resolved);
    assert_eq!(alerts[1].details, "", "missing optional field is empty, not an error");
}

#[test]
fn unmapped_fields_never_reach_the_alert() {
    let body = std::fs::read("tests/fixtures/grafana.json").unwrap();
    let alerts = extract("grafana", &grafana_like(), &body).unwrap();
    let dump = format!("{:?}", alerts);
    assert!(!dump.contains("secret-token"));
    assert!(!dump.contains("valueString"));
}

#[test]
fn map_translates_status_and_const_fills_env() {
    let spec = SourceSpec {
        alerts: "".into(),
        fields: Fields {
            status: FieldSpec::Path {
                path: "/state".into(),
                map: Some(BTreeMap::from([("KO".into(), "firing".into()), ("OK".into(), "resolved".into())])),
            },
            identity: Some(p("/check")),
            summary: FieldSpec::Template { template: "{check} failed: {line}".into() },
            details: None,
            env: Some(FieldSpec::Const { value: "prod".into() }),
            severity: None,
        },
    };
    let body = br#"{"state":"KO","check":"backup","line":"archive older than 26h"}"#;
    let a = &extract("homelab", &spec, body).unwrap()[0];
    assert_eq!(a.status, Status::Firing);
    assert_eq!(a.summary, "backup failed: archive older than 26h");
    assert_eq!(a.env, "prod");
    assert_eq!(a.severity, "unknown");
}

#[test]
fn template_accepts_nested_pointers() {
    let spec = SourceSpec {
        alerts: "".into(),
        fields: Fields {
            status: FieldSpec::Const { value: "firing".into() },
            identity: None,
            summary: FieldSpec::Template { template: "{labels/alertname} on {/labels/instance}".into() },
            details: None,
            env: None,
            severity: None,
        },
    };
    let a = &extract("x", &spec, br#"{"labels":{"alertname":"A","instance":"i-1"}}"#).unwrap()[0];
    assert_eq!(a.summary, "A on i-1");
}

#[test]
fn non_string_values_are_stringified() {
    let spec = SourceSpec {
        alerts: "".into(),
        fields: Fields {
            status: FieldSpec::Const { value: "firing".into() },
            identity: None,
            summary: p("/n"),
            details: Some(p("/b")),
            env: Some(p("/z")),
            severity: Some(p("/o")),
        },
    };
    let a = &extract("x", &spec, br#"{"n":42,"b":true,"z":null,"o":{"k":1}}"#).unwrap()[0];
    assert_eq!(a.summary, "42");
    assert_eq!(a.details, "true");
    assert_eq!(a.env, "unknown", "null counts as missing");
    assert_eq!(a.severity, r#"{"k":1}"#);
}

#[test]
fn bad_status_is_an_error_not_a_guess() {
    let spec = SourceSpec {
        alerts: "".into(),
        fields: Fields {
            status: p("/status"),
            identity: None,
            summary: FieldSpec::Const { value: "s".into() },
            details: None,
            env: None,
            severity: None,
        },
    };
    assert!(extract("x", &spec, br#"{"status":"weird"}"#).is_err());
    assert!(extract("x", &spec, br#"{}"#).is_err());
    assert!(extract("x", &spec, b"not json").is_err());
}

#[test]
fn default_identity_ignores_digits() {
    assert_eq!(default_identity("s", "CPU at 92%"), default_identity("s", "CPU at 95%"));
    assert_ne!(default_identity("s", "CPU at 92%"), default_identity("t", "CPU at 92%"));
    assert_eq!(default_identity("s", "x").len(), 16);
}

#[test]
fn missing_identity_falls_back_to_default() {
    let spec = SourceSpec {
        alerts: "".into(),
        fields: Fields {
            status: FieldSpec::Const { value: "firing".into() },
            identity: None,
            summary: FieldSpec::Const { value: "CPU at 92%".into() },
            details: None,
            env: None,
            severity: None,
        },
    };
    let a = &extract("s", &spec, b"{}").unwrap()[0];
    assert_eq!(a.identity, default_identity("s", "CPU at 92%"));
}

use proptest::prelude::*;

fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        ".{0,40}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::from),
            prop::collection::btree_map("[a-z/~]{0,8}", inner, 0..6)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn extract_never_panics(v in arb_json(), raw in prop::collection::vec(any::<u8>(), 0..200)) {
        let spec = grafana_like();
        let _ = extract("grafana", &spec, v.to_string().as_bytes());
        let _ = extract("grafana", &spec, &raw);
    }
}

// `#[serde(untagged, deny_unknown_fields)]` on FieldSpec is a known ambiguity (see task-2
// brief): some serde versions ignore deny_unknown_fields on untagged enums. This crate's
// pinned serde does honor it, but guard the behavior so a serde bump can't silently regress it.
#[test]
fn fieldspec_rejects_unknown_fields() {
    let bad = r#"{"path":"/x","bogus":1}"#;
    let res: Result<FieldSpec, _> = serde_json::from_str(bad);
    assert!(res.is_err(), "expected deny_unknown_fields to reject bogus field, got {res:?}");
}

#[test]
fn extract_each_maps_alerts_one_by_one() {
    use keenwake::mapping::{extract_each, MapError};
    let body = br#"{"alerts":[{"status":"firing","fingerprint":"a","annotations":{"summary":"x"}},
        {"status":"maybe","fingerprint":"b","annotations":{"summary":"y"}}]}"#;
    let each = extract_each("grafana", &grafana_like(), body).unwrap();
    assert_eq!(each[0].as_ref().unwrap().identity, "a");
    let bad = each[1].as_ref().unwrap_err();
    assert_eq!(bad.error, MapError::BadStatus("maybe".into()));
    assert_eq!(bad.item["fingerprint"], "b");
    assert_eq!(extract("grafana", &grafana_like(), body), Err(MapError::BadStatus("maybe".into())));
    assert_eq!(extract_each("grafana", &grafana_like(), b"nope"), Err(MapError::NotJson));
}
