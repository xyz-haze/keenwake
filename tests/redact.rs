use keenwake::redact::{Pattern, Redactor};
use proptest::prelude::*;

fn all() -> Redactor {
    Redactor::new(&[Pattern::Email, Pattern::Ip, Pattern::Token])
}

#[test]
fn scrubs_each_kind() {
    let r = all();
    assert_eq!(r.clean("mail ops@example.com now"), "mail [redacted:email] now");
    assert_eq!(r.clean("from 10.0.3.17 down"), "from [redacted:ip] down");
    assert_eq!(r.clean("peer fe80::1ff:fe23:4567:890a lost"), "peer [redacted:ip] lost");
    assert_eq!(r.clean("Authorization: Bearer eyJhbGciOi.abc-def"), "Authorization: [redacted:token]");
    assert_eq!(r.clean("key ghp_abcdefghijklmnop1234"), "key [redacted:token]");
}

/// Secrets seen in real alert text: each row is (input, expected output).
#[test]
fn scrubs_credentials_in_realistic_alert_text() {
    let r = all();
    for (input, want) in [
        ("IAM key AKIAIOSFODNN7EXAMPLE used from new region", "IAM key [redacted:token] used from new region"),
        ("session ASIAY34FZKBOKMUTVV7A expired", "session [redacted:token] expired"),
        (
            "aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY in env dump",
            "aws_secret_access_key=[redacted:token] in env dump",
        ),
        (
            "cannot connect to postgres://app:s3cr3t-P4ss@db-1.internal:5432/orders",
            "cannot connect to postgres://app:[redacted:token]@db-1.internal:5432/orders",
        ),
        ("redis://:hunter2hunter2@cache.internal:6379/0 timeout", "redis://:[redacted:token]@cache.internal:6379/0 timeout"),
        (
            "GET https://api.example.org/v1/items?api_key=abc123def456&page=2 returned 500",
            "GET https://api.example.org/v1/items?api_key=[redacted:token]&page=2 returned 500",
        ),
        ("probe failed apikey=Zx81kQ token=tok_9f8e7d secret=s3cr3t", "probe failed apikey=[redacted:token] token=[redacted:token] secret=[redacted:token]"),
        ("PASSWORD=Winter2026! passwd: letmein99", "PASSWORD=[redacted:token] passwd: [redacted:token]"),
        (r#"config {"password": "hunter2", "user": "app"}"#, r#"config {"password": "[redacted:token]", "user": "app"}"#),
        ("Authorization: Basic YWRtaW46cGFzc3dvcmQ= rejected", "Authorization: [redacted:token] rejected"),
        ("authorization=Token 9f86d081884c7d659a2f rejected", "authorization=[redacted:token] rejected"),
        (
            "posting to https://hooks.slack.com/services/T0000/B0000/XXXXXXXXXXXXXXXXXXXXXXXX failed",
            "posting to https://hooks.slack.com/[redacted:token] failed",
        ),
        (
            "webhook https://discord.com/api/webhooks/123456789012345678/AbCdEf-GhIjKl_MnOp failed",
            "webhook https://discord.com/api/webhooks/[redacted:token] failed",
        ),
        (
            "token in log: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            "token in log: [redacted:token]",
        ),
        ("jwt header eyJhbGciOiJSUzI1NiJ9 only", "jwt header [redacted:token] only"),
    ] {
        assert_eq!(r.clean(input), want, "input: {input}");
        assert_eq!(r.clean(want), want, "not idempotent: {want}");
    }
}

#[test]
fn keeps_ordinary_alert_text() {
    let r = all();
    for s in [
        "p99 latency 250ms above 200ms for 5m on checkout-api",
        "Disk usage 91% on db-1, 12.5 GiB left",
        "token bucket refill rate dropped to 0",
        "password reset emails queued: 42",
        "secret rotation job failed after 3 retries",
        "Basic health check failed on web-3",
        "open http://grafana.internal:3000/d/abc?orgId=1&from=now-1h",
        "cannot reach postgres://db.internal:5432/app",
        "hooks.slack.com unreachable for 2m",
        "uptime 99.95% over 30d, 5xx rate 0.4%",
    ] {
        assert_eq!(r.clean(s), s);
    }
}

#[test]
fn keeps_ordinary_text() {
    let r = all();
    let s = "CPU above 90% on etl-2 for 3m at 02:57:00Z, fingerprint 77de1aae9f2d6621, version 1.2.3, pod api-7d9f8c6b5-x2k4q-very-long-deployment-name";
    assert_eq!(r.clean(s), s);
}

#[test]
fn only_enabled_patterns_apply() {
    let r = Redactor::new(&[Pattern::Email]);
    assert_eq!(r.clean("a@b.io 10.0.0.1"), "[redacted:email] 10.0.0.1");
}

proptest! {
    #[test]
    fn a_secret_after_a_known_key_never_survives(
        key in prop::sample::select(vec!["api_key", "apikey", "token", "secret", "password", "passwd", "aws_secret_access_key"]),
        upper in any::<bool>(), sep in prop::sample::select(vec!["=", ": ", " = "]), value in "[A-Za-z0-9/+!$%*._~-]{6,40}",
    ) {
        let key = if upper { key.to_uppercase() } else { key.to_string() };
        let s = format!("check failed {key}{sep}{value} retrying");
        prop_assert_eq!(all().clean(&s), format!("check failed {key}{sep}[redacted:token] retrying"));
    }

    #[test]
    fn a_url_password_never_survives(
        scheme in prop::sample::select(vec!["postgres", "mysql", "redis", "amqp", "https", "mongodb+srv"]),
        user in "[a-z]{1,8}", pass in "[A-Za-z0-9!$%*~._-]{6,24}", host in "[a-z]{3,8}",
    ) {
        let s = format!("dial {scheme}://{user}:{pass}@{host}.internal:5432/db refused");
        prop_assert_eq!(all().clean(&s), format!("dial {scheme}://{user}:[redacted:token]@{host}.internal:5432/db refused"));
    }

    #[test]
    fn output_never_contains_an_email_and_is_idempotent(
        pre in "[ -~]{0,20}", user in "[a-z0-9.]{1,10}", dom in "[a-z]{1,10}", post in "[ -~]{0,20}"
    ) {
        let r = all();
        let s = format!("{pre} {user}@{dom}.com {post}");
        let once = r.clean(&s);
        let needle = format!("{}@{}.com", user, dom);
        prop_assert!(!once.contains(&needle));
        prop_assert_eq!(r.clean(&once), once.clone());
    }

    #[test]
    fn output_never_contains_an_ipv4(a in 0u8..=255, b in 0u8..=255, c in 0u8..=255, d in 0u8..=255) {
        let ip = format!("{a}.{b}.{c}.{d}");
        let once = all().clean(&format!("host {ip} down"));
        prop_assert!(!once.contains(&ip));
    }
}
