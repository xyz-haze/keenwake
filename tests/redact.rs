use keenwake::redact::Redactor;
use proptest::prelude::*;

fn all() -> Redactor {
    Redactor::new(&["email".into(), "ip".into(), "token".into()])
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

#[test]
fn keeps_ordinary_text() {
    let r = all();
    let s = "CPU above 90% on etl-2 for 3m at 02:57:00Z, fingerprint 77de1aae9f2d6621, version 1.2.3, pod api-7d9f8c6b5-x2k4q-very-long-deployment-name";
    assert_eq!(r.clean(s), s);
}

#[test]
fn only_enabled_patterns_apply() {
    let r = Redactor::new(&["email".into()]);
    assert_eq!(r.clean("a@b.io 10.0.0.1"), "[redacted:email] 10.0.0.1");
}

proptest! {
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
