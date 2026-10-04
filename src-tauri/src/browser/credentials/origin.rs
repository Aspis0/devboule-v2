//! One site a saved login may be used on, in the only form it is stored or
//! compared in: scheme, host and port, and nothing else.
//!
//! The comparison this form exists for is against the origin of the frame a
//! field lives in, so the spelling has to be the one the runtime itself writes.
//! That is what [`url`] gives: the same WHATWG parser the browser used, and
//! its own `origin().ascii_serialization()`. A hand-rolled canonicalisation is
//! a comparison against a guess — it would fold `http://0x7f.1` and
//! `http://127.0.0.1` as two sites when a browser calls them one, and would
//! have to refuse an IDN name the runtime reports in punycode.

use std::fmt::Display;

use url::Url;

/// The address a page's frame has to be at before a password may be typed
/// into it. Always the canonical spelling, so two spellings of one site
/// compare equal and nothing else does.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Origin(String);

impl Origin {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Origin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The canonical spelling of `raw`, or the sentence the Settings form shows
/// when it is not one.
pub fn canonical(raw: &str) -> Result<Origin, String> {
    let asked = raw.trim();
    let parsed = Url::parse(asked).map_err(|error| refusal(asked, &error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(refusal(asked, "only http and https sites can be saved"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        // Userinfo is a credential of the address's own, and an origin never
        // has one; keeping the part before the `@` would drop it silently.
        return Err(refusal(
            asked,
            "an address with a login in it is not a site address",
        ));
    }
    // A parser gives a special scheme an empty path as "/", which is the root
    // and not a page: what is refused is a path that names one.
    if parsed.path() != "/" {
        return Err(refusal(asked, "an origin has no path after the host"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(refusal(asked, "an origin carries no query or fragment"));
    }
    let origin = parsed.origin();
    if !origin.is_tuple() {
        // The scheme is one of the two above, so this is the parser refusing
        // to give an address a host at all. Refused rather than stored as the
        // string "null", which would compare equal to every other null.
        return Err(refusal(asked, "it has no origin this app can compare"));
    }
    Ok(Origin(origin.ascii_serialization()))
}

fn refusal(asked: &str, why: &str) -> String {
    format!("{asked:?} is not a site address this app can save: {why}.")
}

#[cfg(test)]
#[path = "origin_tests.rs"]
mod tests;
