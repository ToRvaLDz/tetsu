use tetsu_bus::secrets::SecretKind;

use super::*;

fn long(n: usize) -> String {
    "a".repeat(n)
}

#[test]
fn secret_ids() {
    let ok = ["a", "anthropic", "0day", "my-key_2", &long(64)];
    for text in ok {
        assert_eq!(SecretId::parse(text).unwrap().as_str(), text, "{text}");
    }
    let bad = [
        "",
        "-a",
        "_a",
        "A",
        "a b",
        "a.b",
        "a/b",
        "é",
        &long(65),
        "a\n",
    ];
    for text in bad {
        let e = SecretId::parse(text).unwrap_err();
        assert_eq!(e.field, "id", "{text:?}");
    }
}

#[test]
fn hosts() {
    let ok = [
        "api.anthropic.com",
        "localhost",
        "example.com:8443",
        "10.0.0.1",
        "10.0.0.1:80",
        "[::1]:8080",
        "[2001:db8::1]",
        "a-b.example.com",
    ];
    for text in ok {
        assert_eq!(Host::parse(text).unwrap().as_str(), text, "{text}");
    }
    let bad = [
        "",
        "https://example.com",
        "example.com/path",
        "user@example.com",
        "Example.com",
        "example.com:",
        "example.com:0",
        "example.com:70000",
        "example.com:80a",
        "-a.example.com",
        "a-.example.com",
        "a..example.com",
        "exa mple.com",
        "[::1",
        "[zz]:80",
        "a?b",
        "ex_ample.com",
        &format!("{}.com", long(64)),
    ];
    for text in bad {
        let e = Host::parse(text).unwrap_err();
        assert_eq!(e.field, "host", "{text:?}");
    }
}

#[test]
fn tool_ids() {
    for text in ["web-fetch", "tools.git.v2", "a_b", "0"] {
        assert_eq!(ToolId::parse(text).unwrap().as_str(), text);
    }
    for text in ["", ".a", "-a", "A", "a b", "a/b", "a,b"] {
        assert_eq!(
            ToolId::parse(text).unwrap_err().field,
            "allowed_tools",
            "{text:?}"
        );
    }
}

#[test]
fn header_names_are_lowercase_tokens() {
    assert_eq!(
        HeaderName::parse("X-Api-Key").unwrap().as_str(),
        "x-api-key"
    );
    assert_eq!(
        HeaderName::parse("x-goog-api-key").unwrap().as_str(),
        "x-goog-api-key"
    );
    for text in ["", "a b", "a:b", "a\r\nb", "é", "a/b", "(a)"] {
        assert_eq!(
            HeaderName::parse(text).unwrap_err().field,
            "header_name",
            "{text:?}"
        );
    }
}

#[test]
fn kind_specs_round_trip_through_the_cli_form() {
    assert_eq!(KindSpec::parse("bearer").unwrap(), KindSpec::Bearer);
    assert_eq!(KindSpec::parse("basic").unwrap(), KindSpec::Basic);
    let header = KindSpec::parse("header:X-Api-Key").unwrap();
    assert_eq!(
        header,
        KindSpec::Header(HeaderName::parse("x-api-key").unwrap())
    );
    for text in [
        "",
        "header",
        "header:",
        "token",
        "bearer:x",
        "Bearer",
        "header:a b",
    ] {
        assert_eq!(KindSpec::parse(text).unwrap_err().field, "kind", "{text:?}");
    }
}

#[test]
fn kind_specs_map_to_and_from_the_wire() {
    assert_eq!(KindSpec::Bearer.to_wire(), (SecretKind::Bearer, None));
    assert_eq!(KindSpec::Basic.to_wire(), (SecretKind::Basic, None));
    let header = KindSpec::parse("header:x-api-key").unwrap();
    assert_eq!(
        header.to_wire(),
        (SecretKind::Header, Some("x-api-key".into()))
    );
    assert_eq!(
        KindSpec::from_wire(SecretKind::Bearer, None).unwrap(),
        KindSpec::Bearer
    );
    assert_eq!(
        KindSpec::from_wire(SecretKind::Header, Some("X-Key")).unwrap(),
        KindSpec::parse("header:x-key").unwrap()
    );
    let missing = KindSpec::from_wire(SecretKind::Header, None).unwrap_err();
    assert_eq!(missing.field, "header_name");
    let extra = KindSpec::from_wire(SecretKind::Bearer, Some("x")).unwrap_err();
    assert_eq!(extra.field, "header_name");
    assert_eq!(
        KindSpec::from_wire(SecretKind::Unknown, None)
            .unwrap_err()
            .field,
        "kind"
    );
}

#[test]
fn values_are_not_empty_and_have_no_nul() {
    assert!(check_value("S3NT1NEL-SENTINEL").is_ok());
    assert_eq!(check_value("").unwrap_err().field, "value");
    assert_eq!(check_value("a\0b").unwrap_err().field, "value");
}

#[test]
fn errors_never_echo_the_input() {
    let e = SecretId::parse("S3NT1NEL-SENTINEL UPPER").unwrap_err();
    assert!(!e.to_string().contains("S3NT1NEL"));
    let e = Host::parse("https://S3NT1NEL-SENTINEL").unwrap_err();
    assert!(!e.to_string().contains("S3NT1NEL"));
}
