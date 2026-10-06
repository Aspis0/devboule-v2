//! What the daemon's own outbound requests may reach, decided before a byte is
//! sent.
//!
//! One phrase: a destination is admitted only when its call site declared it.
//! The scheme is `https`, the host is one the caller named exactly (no wildcard,
//! no address literal), and every address the host resolves to is public by the
//! one range table the desktop app's browser uses too. The single exception is
//! a call site that declares a loopback endpoint of its own — the Oracle query
//! route inside the app — and then only `http://127.0.0.1:<port><that path>`.
//!
//! The answer carries the addresses that were checked so the client can connect
//! to exactly those: a name that resolves differently a moment later is not
//! looked up again.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use devboule_protocol::{
    address_blocked, is_localhost, literal_address, looks_numeric, normalise_host,
};
use reqwest::Url;

/// What one call site declares it may reach.
pub(crate) struct Rule {
    /// Exact host names, lower case. Nothing is matched by suffix or pattern.
    pub(crate) hosts: &'static [&'static str],
    /// The one path on `127.0.0.1` the site may call over plain `http`, when it
    /// has a loopback endpoint of its own.
    pub(crate) loopback_path: Option<&'static str>,
}

/// Why a destination was refused: a sentence, never the caller's own text
/// echoed past the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal(pub(crate) String);

fn refuse(sentence: impl Into<String>) -> Refusal {
    Refusal(sentence.into())
}

/// A host pinned to the addresses that passed the check.
pub(crate) struct Pin {
    pub(crate) host: String,
    pub(crate) addresses: Vec<SocketAddr>,
}

pub(crate) struct Admitted {
    /// `None` only for the declared loopback endpoint.
    pub(crate) pin: Option<Pin>,
}

/// What the policy asks the network for. The real one is the OS resolver; a
/// test hands in its own answers.
pub(crate) trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

pub(crate) struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|error| error.to_string())
    }
}

const HTTPS_PORT: u16 = 443;

/// Decide one URL for one call site, resolving its host under `budget`.
pub(crate) fn admit(
    rule: &Rule,
    url: &Url,
    resolver: &Arc<dyn Resolver>,
    budget: Duration,
) -> Result<Admitted, Refusal> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refuse("a URL that carries credentials is not requested"));
    }
    let Some(host) = url.host_str() else {
        return Err(refuse("a URL without a host is not requested"));
    };
    if is_declared_loopback(rule, url, host) {
        return Ok(Admitted { pin: None });
    }
    if url.scheme() != "https" {
        return Err(refuse(format!(
            "only https is requested, not {}",
            url.scheme()
        )));
    }
    if url.port().is_some() {
        return Err(refuse("only the default https port is requested"));
    }
    let host = normalise_host(host);
    if literal_address(&host).is_some() || looks_numeric(&host) || is_localhost(&host) {
        return Err(refuse(
            "an address or a local name is not requested by name",
        ));
    }
    if !rule.hosts.iter().any(|allowed| *allowed == host) {
        return Err(refuse(format!(
            "{host} is not a host this request may reach"
        )));
    }
    let addresses = resolve_within(resolver, &host, budget)?;
    if let Some(category) = addresses
        .iter()
        .find_map(|address| address_blocked(*address))
    {
        return Err(refuse(format!(
            "{host} resolves to a {category}, which is not requested"
        )));
    }
    Ok(Admitted {
        pin: Some(Pin {
            host,
            addresses: addresses
                .into_iter()
                .map(|address| SocketAddr::new(address, HTTPS_PORT))
                .collect(),
        }),
    })
}

/// The one plain-http destination: loopback, the declared path, a port.
fn is_declared_loopback(rule: &Rule, url: &Url, host: &str) -> bool {
    rule.loopback_path == Some(url.path())
        && url.scheme() == "http"
        && host == "127.0.0.1"
        && url.port().is_some()
}

/// The host's addresses, never waiting past `budget`. A lookup that fails,
/// times out or answers nothing is a refusal: no address is not a public one.
fn resolve_within(
    resolver: &Arc<dyn Resolver>,
    host: &str,
    budget: Duration,
) -> Result<Vec<IpAddr>, Refusal> {
    let (sender, receiver) = mpsc::channel();
    let lookup = Arc::clone(resolver);
    let name = host.to_string();
    std::thread::Builder::new()
        .name("outbound-resolve".to_string())
        .spawn(move || {
            let _ = sender.send(lookup.resolve(&name, HTTPS_PORT));
        })
        .map_err(|_| refuse(format!("{host} could not be looked up")))?;
    match receiver.recv_timeout(budget) {
        Ok(Ok(addresses)) if !addresses.is_empty() => Ok(addresses),
        Ok(Ok(_)) => Err(refuse(format!("{host} resolved to no address"))),
        Ok(Err(_)) => Err(refuse(format!("{host} could not be resolved"))),
        Err(_) => Err(refuse(format!("{host} did not resolve in time"))),
    }
}

/// Which GitHub host a `gh` call is about. `gh` makes its own connections, so
/// their addresses cannot be checked here; what can be decided is the name.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GhHost {
    /// `github.com`: always the one the person's `gh` knows by default.
    Dotcom,
    /// Any other well-formed name: only as good as the person's own `gh`
    /// login for exactly this host, which the caller must confirm.
    Enterprise(String),
}

/// Read a host a `gh` call would name. No pattern admits anything: a host is
/// `github.com` or a plain DNS name the caller then proves a login for.
pub(crate) fn classify_gh_host(host: &str) -> Result<GhHost, Refusal> {
    let host = normalise_host(host);
    let plain = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    if !plain || literal_address(&host).is_some() || looks_numeric(&host) || is_localhost(&host) {
        return Err(refuse("that is not a host name `gh` may be asked about"));
    }
    Ok(if host == "github.com" {
        GhHost::Dotcom
    } else {
        GhHost::Enterprise(host)
    })
}

#[cfg(test)]
#[path = "egress_policy_tests.rs"]
pub(crate) mod tests;
