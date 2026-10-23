use buglogin_network::proxy::parse_lines;

#[test]
fn parses_uri_and_legacy_rows_without_network_io() {
    let report = parse_lines(
        "http://user:secret@127.0.0.1:8080\nsocks5://[::1]:1080\n127.0.0.2:3128:user:pass",
    );
    assert_eq!(report.rows.len(), 3);
    assert_eq!(report.rows[0].host, "127.0.0.1");
    assert_eq!(report.rows[1].port, 1080);
    assert_eq!(report.errors.len(), 0);
}

#[test]
fn dedupes_and_redacts_failures() {
    let report = parse_lines(
        "http://user:secret@127.0.0.1:8080\nhttp://user:secret@127.0.0.1:8080\nftp://user:secret@127.0.0.1:21",
    );
    assert_eq!(report.rows.len(), 1);
    assert_eq!(report.duplicates, 1);
    assert_eq!(report.errors[0].code, "unsupported_protocol");
    assert!(!format!("{:?}", report.rows[0]).contains("secret"));
}

#[test]
fn parses_credentials_first_and_socks_alias_formats() {
    let report =
        parse_lines("user:secret:proxy.example:8080\nsocks://user:secret@proxy.example:1080");
    assert!(report.errors.is_empty());
    assert_eq!(report.rows.len(), 2);
    assert_eq!(report.rows[0].host, "proxy.example");
    assert_eq!(report.rows[0].port, 8080);
    assert_eq!(report.rows[0].username.as_deref(), Some("user"));
    assert_eq!(report.rows[1].port, 1080);
}
