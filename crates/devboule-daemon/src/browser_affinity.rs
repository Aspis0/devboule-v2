//! Which browser host owns which tab.
//!
//! A tab belongs to the host that opened it, and a call for that tab goes back
//! to it. The map is keyed by the workspace of the caller that opened the tab
//! and the tab's id, so two workspaces may use the same id independently, and
//! it learns an owner only from a tab-creating command's successful result.
//! The first claim for a key wins while its host is live; once the host left,
//! the next claim replaces it, so a restored tab can be learned again.
//!
//! The map is bounded. At the cap the oldest entry is evicted to make room,
//! entries whose host left (`Gone`) first: they exist only to answer
//! `browser_owner_unavailable` instead of letting a call land on a host that
//! never had the tab.

use std::collections::HashMap;

use serde_json::Value;

/// The commands whose successful result names a tab they opened: a top-level
/// string `browserId` in `result`. The only commands the daemon learns an
/// owner from.
const TAB_CREATING_COMMANDS: &[&str] = &["new_tab"];
/// The command that closes the tab named by the call's `browser_id`. Its
/// success makes the daemon forget that tab's owner.
const TAB_CLOSING_COMMAND: &str = "close_tab";

const MAX_TAB_OWNERS: usize = 1024;
const MAX_BROWSER_ID_BYTES: usize = 128;

#[derive(Clone, PartialEq, Eq, Hash)]
struct TabKey {
    workspace_id: Option<String>,
    browser_id: String,
}

enum Owner {
    Host(String),
    Gone,
}

struct Entry {
    owner: Owner,
    /// Order of insertion, so eviction can find the oldest.
    born: u64,
}

/// Where a tab's calls go.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TabRoute<'a> {
    Host(&'a str),
    Gone,
    /// Nobody has claimed it; the caller picks a host.
    Unknown,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    Learned,
    /// The same host claimed it again.
    Known,
    /// Another live host already holds the key. Nothing moved.
    Conflict,
    /// Not a usable id.
    Refused,
}

#[derive(Default)]
pub(crate) struct TabAffinity {
    entries: HashMap<TabKey, Entry>,
    next_born: u64,
}

pub(crate) fn creates_tab(command: &str) -> bool {
    TAB_CREATING_COMMANDS.contains(&command)
}

pub(crate) fn closes_tab(command: &str) -> bool {
    command == TAB_CLOSING_COMMAND
}

/// The tab a tab-creating command's result names.
pub(crate) fn opened_tab(result: &Value) -> Option<&str> {
    result.get("browserId").and_then(Value::as_str)
}

impl TabAffinity {
    pub(crate) fn route(&self, workspace_id: Option<&str>, browser_id: &str) -> TabRoute<'_> {
        match self.entries.get(&key(workspace_id, browser_id)) {
            Some(Entry {
                owner: Owner::Host(host_id),
                ..
            }) => TabRoute::Host(host_id),
            Some(Entry {
                owner: Owner::Gone, ..
            }) => TabRoute::Gone,
            None => TabRoute::Unknown,
        }
    }

    pub(crate) fn claim(
        &mut self,
        workspace_id: Option<&str>,
        browser_id: &str,
        host_id: &str,
    ) -> Claim {
        if browser_id.is_empty() || browser_id.len() > MAX_BROWSER_ID_BYTES {
            return Claim::Refused;
        }
        let key = key(workspace_id, browser_id);
        match self.entries.get(&key) {
            Some(Entry {
                owner: Owner::Host(owner),
                ..
            }) if owner == host_id => return Claim::Known,
            Some(Entry {
                owner: Owner::Host(_),
                ..
            }) => return Claim::Conflict,
            Some(Entry {
                owner: Owner::Gone, ..
            }) => {}
            None if self.entries.len() >= MAX_TAB_OWNERS => self.evict_one(),
            None => {}
        }
        self.next_born += 1;
        self.entries.insert(
            key,
            Entry {
                owner: Owner::Host(host_id.to_string()),
                born: self.next_born,
            },
        );
        Claim::Learned
    }

    pub(crate) fn forget(&mut self, workspace_id: Option<&str>, browser_id: &str) {
        self.entries.remove(&key(workspace_id, browser_id));
    }

    /// The host left: its tabs stay, as `Gone`, to answer for it.
    pub(crate) fn strand_host(&mut self, host_id: &str) {
        for entry in self.entries.values_mut() {
            if matches!(&entry.owner, Owner::Host(owner) if owner == host_id) {
                entry.owner = Owner::Gone;
            }
        }
    }

    fn evict_one(&mut self) {
        let oldest = |gone_only: bool| {
            self.entries
                .iter()
                .filter(|(_, entry)| !gone_only || matches!(entry.owner, Owner::Gone))
                .min_by_key(|(_, entry)| entry.born)
                .map(|(key, _)| key.clone())
        };
        if let Some(victim) = oldest(true).or_else(|| oldest(false)) {
            self.entries.remove(&victim);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn key(workspace_id: Option<&str>, browser_id: &str) -> TabKey {
    TabKey {
        workspace_id: workspace_id.map(str::to_string),
        browser_id: browser_id.to_string(),
    }
}

#[cfg(test)]
#[path = "browser_affinity_tests.rs"]
mod tests;
