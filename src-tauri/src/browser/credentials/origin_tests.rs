//! The canonicalising table: what one site address is accepted as, and what is
//! refused. Each refusal is a case a person can type, so the table doubles as
//! the list of shapes the Settings form has to reject.
//!
//! Every accepted row is spelled the way a browser serialises the same
//! address, because that is the string this app compares a frame's origin
//! against.

use super::{canonical, of_page};

fn ok(asked: &str) -> String {
    canonical(asked)
        .unwrap_or_else(|why| panic!("{asked:?} must be accepted: {why}"))
        .to_string()
}

fn refused(asked: &str) {
    assert!(
        canonical(asked).is_err(),
        "{asked:?} must be refused: it is not an exact origin"
    );
}

#[test]
fn a_site_is_its_scheme_and_its_host() {
    assert_eq!(ok("https://example.com"), "https://example.com");
    assert_eq!(ok("http://example.com"), "http://example.com");
    // A host is case-insensitive and the scheme is too, and the runtime
    // reports both lowercased, so two spellings of one site must store as one.
    assert_eq!(ok("HTTPS://Example.COM"), "https://example.com");
    assert_eq!(ok("  https://example.com  "), "https://example.com");
    // A name written with accented letters is stored as the punycode a
    // browser reports, which is what the runtime's own origin will be.
    assert_eq!(ok("https://éxample.com"), "https://xn--xample-9ua.com");
}

#[test]
fn a_default_port_is_the_same_origin_as_no_port() {
    assert_eq!(ok("https://example.com:443"), "https://example.com");
    assert_eq!(ok("http://example.com:80"), "http://example.com");
    // And a port that is not the scheme's default is part of the origin.
    assert_eq!(ok("https://example.com:8443"), "https://example.com:8443");
    assert_eq!(ok("http://example.com:443"), "http://example.com:443");
}

#[test]
fn a_trailing_slash_is_what_an_address_bar_shows_and_not_a_path() {
    assert_eq!(ok("https://example.com/"), "https://example.com");
    refused("https://example.com/login");
    refused("https://example.com/?next=1");
    refused("https://example.com/#top");
    refused("https://example.com//");
}

#[test]
fn the_spellings_a_browser_normalises_all_reach_one_origin() {
    // A browser parses these four as the same address and reports its origin
    // in dotted-quad form. A canonicalisation of our own would have stored a
    // second site for one, and filled a password into whichever it picked.
    for asked in [
        "http://127.0.0.1:5173",
        "http://0x7f.0.0.1:5173",
        "http://2130706433:5173",
        "http://127.1:5173",
    ] {
        assert_eq!(ok(asked), "http://127.0.0.1:5173", "{asked} is one address");
    }
    // Spellings that are the same site written oddly, which a parser resolves
    // rather than refuses: one slash, a dot for the root, an empty port. Port
    // zero is a port number to a parser and an origin nothing is served on,
    // which is harmless here because the comparison is against whatever the
    // runtime reports for a frame at that address.
    for asked in [
        "https:/example.com",
        "https://example.com/.",
        "https://example.com:",
    ] {
        assert_eq!(ok(asked), "https://example.com", "{asked} is that site");
    }
    assert_eq!(ok("https://example.com:0"), "https://example.com:0");
    // A percent-encoded label is decoded before it is stored, so the entry names
    // the host a parser resolves it to. It is never the site it looks like,
    // and a runtime that spells the same address differently would simply
    // never match — which is the direction a password must fail in.
    assert_eq!(
        ok("https://example.com%2e.evil.test"),
        "https://example.com..evil.test"
    );
    assert_ne!(
        ok("https://example.com%2e.evil.test"),
        ok("https://example.com")
    );
    // An IPv6 literal is compressed and lowercased, brackets and all.
    assert_eq!(
        ok("http://[fd7a:115c:a1e0:0:0:0:0:1]:8080"),
        "http://[fd7a:115c:a1e0::1]:8080"
    );
    assert_eq!(
        ok("http://[FD7A:115C:A1E0::1]:80"),
        "http://[fd7a:115c:a1e0::1]"
    );
    // A name's trailing dot is a root label the resolver accepts and the
    // browser keeps, so it is kept here too — and it is a different origin
    // from the same name without it.
    assert_eq!(ok("https://example.com./"), "https://example.com.");
    assert_ne!(ok("https://example.com."), ok("https://example.com"));
    // Punycode is already what the parser stores, so it is the identity here.
    assert_eq!(ok("https://xn--xample-9ua.com"), ok("https://éxample.com"));
}

#[test]
fn a_local_address_is_read_as_written() {
    assert_eq!(ok("http://localhost:8080"), "http://localhost:8080");
    assert_eq!(ok("http://[::1]:8080"), "http://[::1]:8080");
}

#[test]
fn a_scheme_that_is_not_http_is_refused() {
    for asked in [
        "ftp://example.com",
        "file:///c:/secrets",
        "javascript:alert(1)",
        "data:text/html,<p>hi</p>",
        "about:blank",
        "chrome://settings",
        "ws://example.com",
        "blob:https://example.com/uuid",
        "example.com",
        "//example.com",
        "",
        "   ",
    ] {
        refused(asked);
    }
}

#[test]
fn an_address_that_names_more_than_an_origin_is_refused() {
    for asked in [
        // Userinfo is a credential of the address's own.
        "https://person:secret@example.com",
        "https://person@example.com",
        // No host.
        "https://",
        "https://:8443",
        // Ports that are not numbers.
        "https://example.com:99999",
        "https://example.com:80a",
        // A host that is an IPv6 literal without its brackets, and one with a
        // port written without the colon that introduces it.
        "http://::1",
        "http://[::1]8080",
        // A zone id scopes an address to one interface, which is not where a
        // site is reachable from another machine.
        "http://[fe80::1%25eth0]:8080",
        // Hosts a parser refuses outright.
        "https://exa mple.com",
    ] {
        refused(asked);
    }
}

#[test]
fn two_spellings_of_one_site_are_one_origin() {
    assert_eq!(
        canonical("https://Example.com:443/").expect("canonical"),
        canonical("https://example.com").expect("canonical")
    );
}

#[test]
fn a_similar_site_is_never_the_same_origin() {
    let wanted = canonical("https://example.com").expect("canonical");
    for other in [
        "https://www.example.com",
        "https://example.com.evil.test",
        "http://example.com",
        "https://sub.example.com",
    ] {
        assert_ne!(
            canonical(other).expect("canonical"),
            wanted,
            "{other} is a different site, not a spelling of it"
        );
    }
}

/// A field lives on a page, not on a bare origin, so the page's own address is
/// read down to its origin: the path a login form sits at is not part of what
/// the saved login allows.
#[test]
fn a_page_address_is_compared_by_the_origin_under_it() {
    let site = of_page("https://shop.example.test").expect("origin");
    for page in [
        "https://shop.example.test/sign-in",
        "https://shop.example.test/sign-in?next=%2Fhome",
        "https://shop.example.test/sign-in#password",
        "https://shop.example.test:443/anything/else",
    ] {
        assert_eq!(of_page(page).expect(page), site, "{page}");
    }
    assert_eq!(
        of_page("https://shop.example.test:8443/sign-in")
            .expect("page")
            .to_string(),
        "https://shop.example.test:8443",
        "a port that is not the scheme's default is part of the site"
    );
}

/// An address with no origin to compare — an opaque one, or a scheme that is
/// not the web's — is refused rather than read as any site at all.
#[test]
fn a_page_with_no_origin_to_compare_is_refused() {
    for asked in [
        "about:blank",
        "data:text/html,<p>hi</p>",
        "javascript:alert(1)",
        "not a url",
        "",
    ] {
        assert!(
            of_page(asked).is_err(),
            "{asked:?} must be refused: it names no site"
        );
    }
}
