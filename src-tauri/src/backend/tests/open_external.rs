//! The one guard the system-browser command runs before it hands anything to
//! the OS: scheme, credentials, and the bytes a URL parser would normalize
//! away. Pure input, no runtime — the sentences are pinned by value so a
//! rewording is a reviewable change, not a silent one.

use devboule_protocol::ErrorCode;

use crate::backend::open_external::{openable_url, MAX_URL_LENGTH};

const NOT_A_WEB_URL: &str = "only http and https links can be opened";
const WITH_CREDENTIALS: &str = "a link that carries credentials is never opened";
const TOO_LONG: &str = "a link this long is never opened";
const NOT_EXACT: &str = "a link with whitespace or control characters is never opened";

fn refusal(input: &str) -> (ErrorCode, String) {
    let error = openable_url(input).expect_err("this input must be refused");
    (error.code, error.message)
}

#[test]
fn an_http_and_an_https_url_are_the_urls_the_os_may_open() {
    let http = openable_url("http://example.com/a").expect("http");
    let https = openable_url("https://example.com/a?q=1#f").expect("https");

    assert_eq!(http.as_str(), "http://example.com/a");
    assert_eq!(https.as_str(), "https://example.com/a?q=1#f");
}

#[test]
fn every_other_scheme_is_refused() {
    for input in [
        "ftp://example.com/a",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "mailto:someone@example.com",
    ] {
        assert_eq!(
            refusal(input),
            (ErrorCode::InvalidRequest, NOT_A_WEB_URL.to_string())
        );
    }
}

#[test]
fn a_user_or_a_password_in_the_authority_is_refused() {
    for input in [
        "https://user@example.com/a",
        "https://user:pass@example.com/a",
        "http://:pass@example.com/a",
    ] {
        assert_eq!(
            refusal(input),
            (ErrorCode::InvalidRequest, WITH_CREDENTIALS.to_string())
        );
    }
}

#[test]
fn an_at_sign_outside_the_authority_is_part_of_the_url() {
    let path = openable_url("https://example.com/a@b").expect("an at sign in a path");
    let query =
        openable_url("https://example.com/?mail=user@example.com").expect("an at sign in a query");

    assert_eq!(path.as_str(), "https://example.com/a@b");
    assert_eq!(query.as_str(), "https://example.com/?mail=user@example.com");
}

#[test]
fn whitespace_the_parser_would_strip_is_refused() {
    for input in [
        " https://example.com/a",
        "https://example.com/a ",
        "https://example.com/a\tb",
        "https://example.com/a\nb",
        "https://example.com/a\u{0}b",
    ] {
        assert_eq!(
            refusal(input),
            (ErrorCode::InvalidRequest, NOT_EXACT.to_string())
        );
    }
}

#[test]
fn input_longer_than_the_ceiling_is_refused_before_it_is_parsed() {
    let prefix = "https://example.com/";
    let at_ceiling = format!("{prefix}{}", "a".repeat(MAX_URL_LENGTH - prefix.len()));
    let over_ceiling = format!("{at_ceiling}a");

    assert!(openable_url(&at_ceiling).is_ok());
    assert_eq!(
        refusal(&over_ceiling),
        (ErrorCode::InvalidRequest, TOO_LONG.to_string())
    );
}
