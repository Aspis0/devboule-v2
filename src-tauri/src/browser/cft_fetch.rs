//! Where a Chrome for Testing archive comes from, and over what.
//!
//! One phrase: fetch the pinned URL without letting a plain-HTTP hop deliver
//! bytes. HTTPS is the only origin any production download uses; HTTP is
//! allowed for loopback literals alone, because the loopback fixture — and
//! nothing else — serves a test archive over HTTP. Redirects are held to
//! HTTPS with no exception: the pinned URL is HTTPS, so a hop off it is a
//! downgrade, and a stopped hop is reported instead of being written.

#![cfg_attr(not(test), allow(dead_code))]

use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Where the archive comes from. Tests hand it a loopback URL or a file that
/// is already on disk, because a unit test that proves a digest is checked
/// before an unzip must not need the internet to get its archive.
pub trait ArchiveSource {
    /// Put the bytes at `url` into `into`, replacing whatever was there.
    fn fetch(&self, url: &str, into: &Path) -> Result<(), String>;
}

/// Fetches over HTTPS with reqwest's blocking client, the same version the
/// daemon already resolves. Blocking builds a runtime of its own, and building
/// one inside the app's runtime panics; `cft_install::install` calls this from
/// a blocking worker for exactly that reason, and tests call
/// `cft_install::install_blocking` directly from a synchronous test.
pub struct Https;

/// Whether a download URL may be opened: HTTPS everywhere, HTTP only on
/// loopback literals. Names are refused for HTTP so no resolver on this
/// machine gets to decide what "local" means.
fn origin_allowed(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|error| format!("{url}: {error}"))?;
    if parsed.scheme() == "https" {
        return Ok(());
    }
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if parsed.scheme() == "http" && loopback {
        return Ok(());
    }
    Err(format!("refusing non-https download: {url}"))
}

/// Whether a redirect may be followed: HTTPS only, with no loopback
/// exception. The pinned URL is HTTPS, so every hop is a possible downgrade,
/// and a stopped hop is reported instead of being written as an archive.
fn redirect_is_https(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https")
}

impl ArchiveSource for Https {
    fn fetch(&self, url: &str, into: &Path) -> Result<(), String> {
        origin_allowed(url)?;
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if redirect_is_https(attempt.url().as_str()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|error| format!("{url}: {error}"))?;
        let answer = client
            .get(url)
            .send()
            .map_err(|error| format!("{url}: {error}"))?;
        // The policy above follows only HTTPS hops, so a redirect that
        // arrives here is the one it refused; the empty body of a stopped
        // 3xx is not an archive and must say why.
        if answer.status().is_redirection() {
            return Err(format!("{url}: a redirect left https"));
        }
        let mut answer = answer
            .error_for_status()
            .map_err(|error| format!("{url}: {error}"))?;
        let mut file =
            File::create(into).map_err(|error| format!("{}: {error}", into.display()))?;
        let written = std::io::copy(&mut answer, &mut file)
            .map_err(|error| format!("{}: {error}", into.display()))?;
        file.flush()
            .map_err(|error| format!("{}: {error}", into.display()))?;
        // The hash is read from this file immediately after; an unflushed
        // download that the OS has not written would fail it on a crash.
        file.sync_all()
            .map_err(|error| format!("{}: {error}", into.display()))?;
        if written == 0 {
            return Err(format!("{url}: the download was empty"));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "cft_fetch_tests.rs"]
mod tests;
