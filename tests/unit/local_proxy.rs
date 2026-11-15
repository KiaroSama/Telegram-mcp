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
