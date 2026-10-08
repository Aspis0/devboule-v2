//! The daemon's one HTTP client: every request it makes on its own behalf goes
//! through [`send`], under the call site's declared [`Rule`].
//!
//! One phrase: send one request, following no hop the policy has not admitted.
//! Redirects are followed here, not by the HTTP library, so each hop is judged
//! like the first — scheme, declared host, every resolved address — and each
//! request connects to the addresses that were checked. The whole exchange has
//! one deadline and one body cap.
//!
//! This is the only module that names `reqwest`; `no_ad_hoc_http_client` holds
//! the rest of the daemon to that.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Url;

use crate::egress_policy::{admit, Pin, Resolver, Rule, SystemResolver};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
}

pub(crate) struct Request {
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

pub(crate) struct Limits {
    /// For the whole exchange, resolution and every hop included.
    pub(crate) timeout: Duration,
    pub(crate) max_body: usize,
    /// Hops that may follow the first; none for a call that never redirects.
    pub(crate) max_redirects: usize,
}

#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    /// The `Retry-After` header as seconds, when the server named a number of
    /// seconds. A date is not read: it is no delay this client can trust.
    pub(crate) retry_after_secs: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OutboundError {
    /// The policy refused before anything was sent, or a redirect it will not
    /// follow came back.
    Refused(String),
    /// The deadline passed, before the answer or while its body arrived.
    Timeout,
    /// The head arrived and the body was cut off.
    Cut,
    TooLarge,
    /// The request could not be made or answered at all.
    Transport,
}

/// One request as a transport sends it.
pub(crate) struct Hop<'a> {
    pub(crate) method: Method,
    pub(crate) url: &'a Url,
    pub(crate) headers: &'a [(&'static str, String)],
    pub(crate) body: &'a [u8],
    /// Connect to these addresses and no others; `None` is the declared
    /// loopback endpoint, which also never goes through a proxy.
    pub(crate) pin: Option<&'a Pin>,
    pub(crate) timeout: Duration,
    /// One more than the cap, so a body past it is seen to be past it.
    pub(crate) read_limit: usize,
}

pub(crate) struct Raw {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
    pub(crate) retry_after: Option<String>,
    pub(crate) body: Vec<u8>,
}

pub(crate) trait Transport {
    fn execute(&self, hop: &Hop<'_>) -> Result<Raw, OutboundError>;
}

pub(crate) fn send(
    rule: &Rule,
    request: &Request,
    limits: &Limits,
) -> Result<Answer, OutboundError> {
    let resolver: Arc<dyn Resolver> = Arc::new(SystemResolver);
    send_with(&ReqwestTransport, &resolver, rule, request, limits)
}

pub(crate) fn send_with(
    transport: &dyn Transport,
    resolver: &Arc<dyn Resolver>,
    rule: &Rule,
    request: &Request,
    limits: &Limits,
) -> Result<Answer, OutboundError> {
    let deadline = Instant::now() + limits.timeout;
    let mut url = Url::parse(&request.url)
        .map_err(|_| OutboundError::Refused("the URL could not be read".to_string()))?;
    for hop in 0..=limits.max_redirects {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(OutboundError::Timeout);
        }
        let admitted = admit(rule, &url, resolver, left)
            .map_err(|refusal| OutboundError::Refused(refusal.0))?;
        let left = deadline.saturating_duration_since(Instant::now());
        let raw = transport.execute(&Hop {
            method: request.method,
            url: &url,
            headers: &request.headers,
            body: &request.body,
            pin: admitted.pin.as_ref(),
            timeout: left,
            read_limit: limits.max_body.saturating_add(1),
        })?;
        if !is_redirect(raw.status) {
            if raw.body.len() > limits.max_body {
                return Err(OutboundError::TooLarge);
            }
            return Ok(Answer {
                status: raw.status,
                retry_after_secs: raw.retry_after.as_deref().and_then(delta_seconds),
                body: raw.body,
            });
        }
        if hop == limits.max_redirects || admitted.pin.is_none() || request.method != Method::Get {
            return Err(OutboundError::Refused(
                "a redirect is not followed".to_string(),
            ));
        }
        url = raw
            .location
            .and_then(|location| url.join(&location).ok())
            .ok_or_else(|| OutboundError::Refused("a redirect named no destination".to_string()))?;
    }
    Err(OutboundError::Refused("too many redirects".to_string()))
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// A `Retry-After` value read as delta-seconds: digits only, so a sign or a
/// date is no delay.
fn delta_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

/// The production transport. Redirects are off: [`send_with`] follows them.
/// A pinned host connects to the checked addresses; the loopback endpoint
/// bypasses any proxy. Through a configured proxy the proxy resolves the
/// name itself, so pinning cannot hold there.
struct ReqwestTransport;

impl Transport for ReqwestTransport {
    fn execute(&self, hop: &Hop<'_>) -> Result<Raw, OutboundError> {
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(hop.timeout)
            .redirect(reqwest::redirect::Policy::none());
        builder = match hop.pin {
            Some(pin) => builder.resolve_to_addrs(&pin.host, &pin.addresses),
            None => builder.no_proxy(),
        };
        let client = builder.build().map_err(|_| OutboundError::Transport)?;
        let mut request = match hop.method {
            Method::Get => client.get(hop.url.clone()),
            Method::Post => client.post(hop.url.clone()).body(hop.body.to_vec()),
        };
        for (name, value) in hop.headers {
            request = request.header(*name, value);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                OutboundError::Timeout
            } else {
                OutboundError::Transport
            }
        })?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut body = Vec::new();
        response
            .take(hop.read_limit as u64)
            .read_to_end(&mut body)
            .map_err(|error| {
                if body_timed_out(&error) {
                    OutboundError::Timeout
                } else {
                    OutboundError::Cut
                }
            })?;
        Ok(Raw {
            status,
            location,
            retry_after,
            body,
        })
    }
}

/// reqwest wraps its own error inside the `io::Error` a body read fails with,
/// so a body that hangs past the deadline still names itself a timeout.
fn body_timed_out(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
        .is_some_and(reqwest::Error::is_timeout)
}

#[cfg(test)]
#[path = "egress_client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "egress_architecture_tests.rs"]
mod architecture_tests;
