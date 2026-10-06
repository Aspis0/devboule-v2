//! Where an agent-driven page may go: one policy, asked by every entry a
//! navigation has — the agent's own command, and the webview hook every
//! redirect, link, script and form post passes through.
//!
//! Blocked by default for agent browsing; the person's own browsing never
//! reaches the verdict. This is about the destination, not the scheme:
//! `url::gate` still owns `file:`, `javascript:` and the rest.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::Url;

#[path = "destination_ranges.rs"]
mod ranges;

use ranges::{address_blocked, is_localhost, literal_address, looks_numeric, normalise_host};

/// How long one host's verdict is reused before it is resolved again: long
/// enough to keep a busy page from resolving on every frame, short enough that
/// a host moved onto a private address is seen.
const VERDICT_TTL: Duration = Duration::from_secs(30);

/// How long a hook's lookup may hold the thread it runs on. The navigation
/// waits for the answer, so a lookup that cannot answer inside this is refused
/// rather than allowed: a wrong block is visible and named, a wrong allow is
/// the hole this module closes.
const LOOKUP_BUDGET: Duration = Duration::from_millis(100);

/// Who is driving the navigation. The person's own browsing is out of scope by
/// the owner's decision, so only `Agent` is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Agent,
    Person,
}

/// A URL the policy refuses, with the sentence the caller shows or reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    message: String,
}

impl Blocked {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// One `host:port` the person allowed. Exact on both halves: an entry for
/// `localhost:1420` says nothing about `127.0.0.1:1420` or port 1421.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    host: String,
    port: u16,
}

/// The person's exceptions, read from the app's settings file. Empty until a
/// person writes one, and nothing an agent can reach edits this.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    entries: Vec<Entry>,
}

impl Allowlist {
    fn from_settings(value: &Value) -> Self {
        let entries = value
            .get("allow")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(parse_entry)
                    .collect()
            })
            .unwrap_or_default();
        Self { entries }
    }

    fn allows(&self, host: &str, port: u16) -> bool {
        let host = normalise_host(host);
        self.entries
            .iter()
            .any(|entry| entry.host == host && entry.port == port)
    }

    #[cfg(test)]
    fn from_entries(entries: &[&str]) -> Self {
        Self {
            entries: entries.iter().filter_map(|raw| parse_entry(raw)).collect(),
        }
    }
}

fn parse_entry(raw: &str) -> Option<Entry> {
    let raw = raw.trim();
    let (host, port) = raw.rsplit_once(':')?;
    let host = host.trim_matches(['[', ']']);
    let port: u16 = port.parse().ok()?;
    (!host.is_empty()).then(|| Entry {
        host: normalise_host(host),
        port,
    })
}

/// What the policy asks the network for. The real one is the OS resolver; a
/// test hands in its own answers.
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

/// The OS resolver, as `ToSocketAddrs` reaches it.
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|error| error.to_string())
    }
}

/// The settings file the allowlist is read from, beside the app's other
/// config. No writer exists in this slice: only a person edits the file.
pub const SETTINGS_FILE: &str = "browser-destinations.json";

/// One host:port's cached verdict, with when it was reached.
type VerdictCache = Mutex<HashMap<(String, u16), (Instant, Result<(), Blocked>)>>;

/// The policy: the allowlist, the resolver, and the verdicts already reached.
pub struct DestinationPolicy {
    allowlist: Allowlist,
    resolver: Arc<dyn Resolver>,
    verdicts: VerdictCache,
}

impl DestinationPolicy {
    fn new(allowlist: Allowlist, resolver: Arc<dyn Resolver>) -> Self {
        Self {
            allowlist,
            resolver,
            verdicts: Mutex::new(HashMap::new()),
        }
    }

    /// The production policy, with the settings file's exceptions when it is
    /// there. A file that cannot be read leaves the list empty: the default.
    pub fn load(config_dir: Option<&Path>) -> Self {
        let allowlist = config_dir
            .and_then(|dir| std::fs::read_to_string(dir.join(SETTINGS_FILE)).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(|value| Allowlist::from_settings(&value))
            .unwrap_or_default();
        Self::new(allowlist, Arc::new(SystemResolver))
    }

    /// The one verdict. A person is never checked; an agent's URL is refused
    /// unless the person allowed its exact host and port.
    pub fn admit(&self, url: &Url, audience: Audience) -> Result<(), Blocked> {
        if audience == Audience::Person {
            return Ok(());
        }
        let Some(host) = url.host_str() else {
            return Err(Blocked::new(
                "That address names no host, so it cannot be reached.",
            ));
        };
        let port = url.port_or_known_default().unwrap_or(0);
        if self.allowlist.allows(host, port) {
            return Ok(());
        }
        // The host itself, before any lookup: every spelling a resolver would
        // accept as an address is decided here.
        if let Some(address) = literal_address(host) {
            return blocked_address(address, host, port);
        }
        if looks_numeric(host) {
            return Err(Blocked::new(format!(
                "{host}:{port} is a numeric host the policy cannot read as an address, so it is blocked for agent browsing."
            )));
        }
        if is_localhost(host) {
            return Err(refusal("loopback host", host, port));
        }
        if host.ends_with('.') {
            return Err(Blocked::new(format!(
                "{host}:{port} is a trailing-dot host, which agent browsing does not resolve."
            )));
        }
        let key = (normalise_host(host), port);
        if let Some((at, verdict)) = self
            .verdicts
            .lock()
            .expect("destination verdicts poisoned")
            .get(&key)
        {
            if at.elapsed() < VERDICT_TTL {
                return verdict.clone();
            }
        }
        let addresses = match self.resolve_bounded(host, port) {
            Ok(addresses) => addresses,
            Err(reason) => {
                let blocked = Blocked::new(format!(
                    "{host}:{port} could not be checked for agent browsing: {reason}."
                ));
                return Err(blocked);
            }
        };
        let verdict = addresses
            .iter()
            .find(|address| address_blocked(**address).is_some())
            .map(|address| blocked_address(*address, host, port))
            .unwrap_or(Ok(()));
        self.verdicts
            .lock()
            .expect("destination verdicts poisoned")
            .insert(key, (Instant::now(), verdict.clone()));
        verdict
    }

    /// The hook's verdict for a tab whose driver is known: a navigation is
    /// agent-driven while an agent command is in flight on the tab, or while
    /// the tab sits behind the pane with no person in front of it.
    pub fn admit_hook(
        &self,
        url: &Url,
        agent_in_flight: bool,
        parked: bool,
    ) -> Result<(), Blocked> {
        let audience = if agent_in_flight || parked {
            Audience::Agent
        } else {
            Audience::Person
        };
        self.admit(url, audience)
    }

    /// Resolve without holding the caller's thread past `LOOKUP_BUDGET`. The
    /// worker owns its resolver handle, so a lookup left behind by a timeout
    /// borrows nothing from the caller and cannot outlive it.
    fn resolve_bounded(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let resolver = Arc::clone(&self.resolver);
        let host_owned = host.to_string();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("devboule-browser-destination".to_string())
            .spawn(move || {
                let _ = sender.send(resolver.resolve(&host_owned, port));
            })
            .map_err(|error| error.to_string())?;
        drop(worker);
        receiver
            .recv_timeout(LOOKUP_BUDGET)
            .unwrap_or_else(|_| Err(format!("no answer within {} ms", LOOKUP_BUDGET.as_millis())))
    }
}

fn blocked_address(address: IpAddr, host: &str, port: u16) -> Result<(), Blocked> {
    let Some(category) = address_blocked(address) else {
        return Ok(());
    };
    // The subject is the address the verdict was about. When the host was a
    // name, the name follows it so the person can find the entry to allow.
    let as_address = display_address(address, port);
    let subject = if host.trim_matches(['[', ']']) == address.to_string() {
        as_address
    } else {
        format!("{as_address} ({host}:{port})")
    };
    Err(Blocked::new(format!(
        "{category} {subject} is blocked for agent browsing; the person can allow {host}:{port} in Settings."
    )))
}

fn display_address(address: IpAddr, port: u16) -> String {
    match address {
        IpAddr::V4(address) => format!("{address}:{port}"),
        IpAddr::V6(address) => format!("[{address}]:{port}"),
    }
}

fn refusal(category: &str, host: &str, port: u16) -> Blocked {
    Blocked::new(format!(
        "{category} {host}:{port} is blocked for agent browsing; the person can allow {host}:{port} in Settings."
    ))
}

#[cfg(test)]
#[path = "destination_tests.rs"]
mod tests;
