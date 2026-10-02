//! The one guard the system-browser command runs before it hands anything to
//! the OS: the canonical http(s) prefix, credentials, and the control and
//! whitespace bytes a URL parser would strip. Pure input, no runtime — the
//! sentences are pinned by value so a rewording is a reviewable change, not a
//! silent one.

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
        "https://user@[::1]/",
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

    let fragment =
        openable_url("https://example.com/#user@example.com").expect("an at sign in a fragment");

    assert_eq!(path.as_str(), "https://example.com/a@b");
    assert_eq!(query.as_str(), "https://example.com/?mail=user@example.com");
    assert_eq!(fragment.as_str(), "https://example.com/#user@example.com");
}

#[test]
fn a_double_slash_inside_the_path_does_not_start_an_authority() {
    let url = openable_url("https://example.com/path//@x").expect("an at sign after a path //");

    assert_eq!(url.as_str(), "https://example.com/path//@x");
}

#[test]
fn only_the_canonical_http_and_https_prefix_is_a_web_url() {
    for input in [
        "https:/@example.com/",
        "https:/example.com/path//@evil",
        "http:/host",
        "https:\\\\host",
        "https:host",
    ] {
        assert_eq!(
            refusal(input),
            (ErrorCode::InvalidRequest, NOT_A_WEB_URL.to_string())
        );
    }
}

#[test]
fn the_scheme_is_matched_without_regard_to_case() {
    assert!(openable_url("HTTPS://example.com/a").is_ok());
    assert_eq!(
        refusal("HTTPS://user@example.com/a"),
        (ErrorCode::InvalidRequest, WITH_CREDENTIALS.to_string())
    );
}

#[test]
fn an_empty_userinfo_marker_is_refused_though_the_parser_drops_it() {
    for input in ["https://@example.com/", "https://:@example.com/"] {
        assert_eq!(
            refusal(input),
            (ErrorCode::InvalidRequest, WITH_CREDENTIALS.to_string())
        );
    }
}

#[test]
fn input_the_parser_rejects_is_refused_as_not_a_web_url() {
    assert_eq!(
        refusal("https://[::1"),
        (ErrorCode::InvalidRequest, NOT_A_WEB_URL.to_string())
    );
}

#[test]
fn whitespace_the_parser_would_strip_is_refused() {
    for input in [
        " https://example.com/a",
        "https://example.com/a ",
        "https://example.com/a\tb",
        "https://example.com/a\nb",
        "https://example.com/a\u{0}b",
        "https://example.com/a\u{a0}b",
        "https://example.com/a\u{85}b",
        "https://example.com/a\u{3000}b",
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

#[test]
fn the_ceiling_counts_bytes_not_characters() {
    let prefix = "https://example.com/";
    let two_byte_characters = (MAX_URL_LENGTH - prefix.len()) / 2;
    let at_ceiling = format!("{prefix}{}", "é".repeat(two_byte_characters));
    let over_ceiling = format!("{at_ceiling}a");

    assert_eq!(at_ceiling.len(), MAX_URL_LENGTH);
    assert!(openable_url(&at_ceiling).is_ok());
    assert_eq!(
        refusal(&over_ceiling),
        (ErrorCode::InvalidRequest, TOO_LONG.to_string())
    );
}
