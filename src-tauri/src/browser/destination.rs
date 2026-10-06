//! Where an agent-driven page may go: one policy, asked by every entry a
//! navigation has — the agent's own command, and the webview hook every
//! redirect, link, script and form post passes through.
//!
//! Blocked by default for agent browsing; the person's own browsing never
//! reaches the verdict. This is about the destination, not the scheme:
//! `url::gate` still owns `file:`, `javascript:` and the rest.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::Url;

#[path = "destination_ranges.rs"]
mod ranges;
#[path = "destination_resolver.rs"]
mod resolver;

use ranges::{address_blocked, is_localhost, literal_address, looks_numeric, normalise_host};
use resolver::{LookupPool, Resolver, SystemResolver};

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

/// The settings file the allowlist is read from, beside the app's other
/// config. No writer exists in this slice: only a person edits the file.
pub const SETTINGS_FILE: &str = "browser-destinations.json";

/// One host:port's blocked verdict, with the instant it stops deciding.
type VerdictCache = Mutex<HashMap<(String, u16), (Instant, Blocked)>>;

/// How long a blocked verdict decides. An allow verdict is never reused: a
/// host that flips to a private address is resolved again on the next look.
const BLOCK_TTL: Duration = Duration::from_secs(30);

/// How long a failed or timed-out lookup decides. Long enough to stop a page
/// looping against a blackholed resolver, short enough to recover from a blip.
const FAILURE_TTL: Duration = Duration::from_secs(5);

/// The most blocked verdicts remembered at once.
const MAX_VERDICTS: usize = 256;

/// The policy: the allowlist, the one resolver, and the blocked verdicts.
pub struct DestinationPolicy {
    allowlist: Allowlist,
    resolver: LookupPool,
    verdicts: VerdictCache,
}

impl DestinationPolicy {
    fn new(allowlist: Allowlist, resolver: Arc<dyn Resolver>) -> Self {
        Self {
            allowlist,
            resolver: LookupPool::start(resolver),
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
            if let Some(blocked) = blocked_for(address, host, port) {
                return Err(blocked);
            }
            return Ok(());
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
        if let Some((expires, blocked)) = self
            .verdicts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key)
        {
            if *expires > Instant::now() {
                return Err(blocked.clone());
            }
        }
        let addresses = match self.resolver.lookup(host, port) {
            Ok(addresses) => addresses,
            Err(reason) => {
                let blocked = Blocked::new(format!(
                    "{host}:{port} could not be checked for agent browsing: {reason}."
                ));
                self.remember(key, blocked.clone(), FAILURE_TTL);
                return Err(blocked);
            }
        };
        if addresses.is_empty() {
            // No address is not a public address: an answer the policy cannot
            // read is refused.
            let blocked = Blocked::new(format!(
                "{host}:{port} resolved to no address, so agent browsing refuses it."
            ));
            self.remember(key, blocked.clone(), FAILURE_TTL);
            return Err(blocked);
        }
        if let Some(blocked) = addresses
            .iter()
            .find_map(|address| blocked_for(*address, host, port))
        {
            self.remember(key, blocked.clone(), BLOCK_TTL);
            return Err(blocked);
        }
        Ok(())
    }

    /// Remember one blocked verdict, dropping expired entries and the oldest
    /// when the table is full, so the table has a bound.
    fn remember(&self, key: (String, u16), blocked: Blocked, ttl: Duration) {
        let now = Instant::now();
        let mut verdicts = self
            .verdicts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        verdicts.retain(|_, (expires, _)| *expires > now);
        if verdicts.len() >= MAX_VERDICTS {
            if let Some(oldest) = verdicts
                .iter()
                .min_by_key(|(_, (expires, _))| *expires)
                .map(|(key, _)| key.clone())
            {
                verdicts.remove(&oldest);
            }
        }
        verdicts.insert(key, (now + ttl, blocked));
    }
}

/// The refusal one address earns, or `None` when it is public. The subject is
/// the address the verdict was about; when the host was a name, the name
/// follows it so the person can find the entry to allow.
fn blocked_for(address: IpAddr, host: &str, port: u16) -> Option<Blocked> {
    let category = address_blocked(address)?;
    let as_address = display_address(address, port);
    let subject = if host.trim_matches(['[', ']']) == address.to_string() {
        as_address
    } else {
        format!("{as_address} ({host}:{port})")
    };
    Some(Blocked::new(format!(
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
