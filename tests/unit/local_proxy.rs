use super::*;

#[test]
fn telegram_mtproto_link_and_bare_endpoint_parse_the_same_secret() {
    let secret = "11".repeat(16);
    let link = parse_proxy(&format!(
        "tg://proxy?server=EXAMPLE.com&port=443&secret={secret}"
    ))
    .unwrap();
    let bare = parse_proxy(&format!("example.com:443:{secret}")).unwrap();
    assert_eq!(link.fingerprint().unwrap(), bare.fingerprint().unwrap());
}

#[test]
fn standard_dd_and_faketls_secrets_have_explicit_validated_shapes() {
    for secret in [
        "11".repeat(16),
        format!("dd{}", "11".repeat(16)),
        format!("ee{}6578616d706c652e636f6d", "11".repeat(16)),
    ] {
        assert!(parse_proxy(&format!("host.example:443:{secret}")).is_ok());
    }
    for secret in [
        "11".repeat(15),
        format!("aa{}", "11".repeat(16)),
        format!("ee{}", "11".repeat(16)),
    ] {
        assert!(parse_proxy(&format!("host.example:443:{secret}")).is_err());
    }
}

#[test]
fn proxy_description_never_exposes_secret_username_or_password() {
    let proxy = parse_proxy("socks5://private-user:private-password@proxy.example:1080").unwrap();
    let description = proxy.describe().unwrap().to_string();
    assert!(!description.contains("private-user"));
    assert!(!description.contains("private-password"));
    assert!(!description.contains("secret"));
    assert!(!description.contains("password"));
}

#[test]
fn socks_credentials_are_percent_decoded_without_turning_plus_into_space() {
    let proxy = parse_proxy("socks5://my%40name:p%3Ass+word@proxy.example:1080").unwrap();
    assert_eq!(proxy.username.as_deref(), Some("my@name"));
    assert_eq!(proxy.password.as_deref(), Some("p:ss+word"));
}

#[test]
fn control_characters_in_credentials_are_rejected_before_storage() {
    assert!(parse_proxy("socks5://name%00injected:password@proxy.example:1080").is_err());
}

#[test]
fn ipv6_host_is_stored_without_url_brackets() {
    let proxy = parse_proxy("socks5://[::1]:1080").unwrap();
    assert_eq!(proxy.host, "::1");
    assert_eq!(proxy.port, 1080);
}

#[test]
fn impossible_ports_and_unknown_protocols_are_refused() {
    for link in [
        "socks5://host.example:0",
        "socks5://host.example:70000",
        "ftp://host.example:21",
        "tg://proxy?server=host&port=443&secret=bad",
    ] {
        assert!(parse_proxy(link).is_err(), "{link}");
    }
}
#[test]
fn telegram_source_links_normalize_only_peer_identity_without_joining_or_opening_urls() {
    for source in [
        "https://t.me/Proxy_Source",
        "t.me/proxy_source/42",
        "https://telegram.me/proxy_source/42/43?single",
        "https://t.me/s/proxy_source",
        "tg://resolve?domain=proxy_source&post=42",
        "tg:resolve?domain=proxy_source",
        "telegram.dog/proxy_source",
        "https://proxy_source.t.me/42",
        "proxy_source.t.me",
        "https://t.me/proxy_source#ignored-fragment",
    ] {
        assert_eq!(
            source_selector(&json!(source)).unwrap(),
            json!("@proxy_source")
        );
    }
    for source in [
        "https://t.me/c/12345/42",
        "https://t.me/c/12345/41/42",
        "tg://privatepost?channel=12345&post=42",
    ] {
        assert_eq!(
            source_selector(&json!(source)).unwrap(),
            json!("-1000000012345")
        );
    }
    assert_eq!(
        source_selector(&json!("-1000000012345")).unwrap(),
        json!("-1000000012345")
    );
    assert_eq!(
        source_selector(&json!("saved alias")).unwrap(),
        json!("saved alias")
    );
}
#[test]
fn source_link_validation_rejects_foreign_hosts_invites_actions_and_ambiguous_ids() {
    for source in [
        "https://t.me.evil.test/source",
        "https://example.test/source",
        "https://owner@t.me/source",
        "https://t.me:444/source",
        "https://share.t.me/path",
        "https://a.t.me/",
        "https://t.me/+invite",
        "https://t.me/joinchat/invite",
        "https://t.me/proxy?server=host&port=443",
        "https://t.me/source/webapp",
        "https://t.me/source/0",
        "https://t.me/c/0/42",
        "https://t.me/c/9223372036854775807/42",
        "https://t.me/source%2Fother",
        "tg://resolve?domain=source&domain=other",
        "tg://join?invite=hash",
        "tg://resolve?phone=00000000",
        "0",
        "9223372036854775808",
        "\n",
    ] {
        assert!(
            source_selector(&json!(source)).is_err(),
            "accepted invalid source {source:?}"
        );
    }
}
