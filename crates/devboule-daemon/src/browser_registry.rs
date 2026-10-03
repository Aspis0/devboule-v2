//! The broker's books: the registered hosts, the calls waiting on them, and
//! the rules that pick a host and admit a call.
//!
//! Plain state behind the broker's one mutex; it never waits and never touches
//! a connection's socket, so everything here is safe to do while holding that
//! lock.

use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use devboule_protocol::{BrowserError, BrowserErrorCode};
use serde_json::Value;

use crate::browser_affinity::{closes_tab, creates_tab, opened_tab, Claim, TabAffinity, TabRoute};
use crate::outbound::ConnOut;

/// Calls one host may have in flight.
pub(crate) const MAX_PENDING_PER_HOST: usize = 16;
/// Calls the daemon may have in flight across every host.
pub(crate) const MAX_PENDING_TOTAL: usize = 64;

pub(crate) type Answer = Result<Value, BrowserError>;

struct Host {
    host_id: String,
    conn_id: u64,
    outbound: Arc<ConnOut>,
    commands: Vec<String>,
}

/// One call waiting for its host, with what is needed to settle the tab
/// bookkeeping when the answer arrives.
pub(crate) struct Pending {
    host_id: String,
    command: String,
    workspace_id: Option<String>,
    browser_id: Option<String>,
    pub(crate) answer: SyncSender<Answer>,
}

pub(crate) struct Admitted {
    pub(crate) request_id: String,
    pub(crate) host_id: String,
    pub(crate) outbound: Arc<ConnOut>,
}

/// What a caller asks for, as the registry needs to route it.
pub(crate) struct Call<'a> {
    pub(crate) command: &'a str,
    pub(crate) workspace_id: Option<&'a str>,
    pub(crate) browser_id: Option<&'a str>,
}

#[derive(Default)]
pub(crate) struct Registry {
    next_host: u64,
    next_request: u64,
    /// Registration order: the last entry is the most recently registered.
    hosts: Vec<Host>,
    pending: HashMap<String, Pending>,
    tabs: TabAffinity,
}

impl Registry {
    /// Make `conn_id` a host. The id is the connection and a per-daemon
    /// counter, so a connection that registers again gets a new id and the old
    /// one never matches anything; the connection's earlier registration is
    /// dropped first.
    pub(crate) fn add_host(
        &mut self,
        conn_id: u64,
        outbound: Arc<ConnOut>,
        commands: Vec<String>,
    ) -> String {
        self.drop_connection(conn_id);
        self.next_host += 1;
        let host_id = format!("{conn_id}.{}", self.next_host);
        self.hosts.push(Host {
            host_id: host_id.clone(),
            conn_id,
            outbound,
            commands,
        });
        host_id
    }

    /// Whether `conn_id` held `host_id` and gave it up.
    pub(crate) fn unregister(&mut self, conn_id: u64, host_id: &str) -> bool {
        let Some(index) = self.host_of_connection(conn_id) else {
            return false;
        };
        if self.hosts[index].host_id != host_id {
            return false;
        }
        self.drop_host(index);
        true
    }

    /// The connection ended: whatever host it held goes with it.
    pub(crate) fn drop_connection(&mut self, conn_id: u64) {
        if let Some(index) = self.host_of_connection(conn_id) {
            self.drop_host(index);
        }
    }

    /// Remove one host: its calls fail as retryable `browser_no_host`, and the
    /// tabs it owned stay as `Gone`.
    fn drop_host(&mut self, index: usize) {
        let host = self.hosts.remove(index);
        let failed = BrowserError::daemon(
            BrowserErrorCode::NoHost,
            "The browser host disconnected before it answered.",
        );
        self.pending.retain(|_, pending| {
            if pending.host_id != host.host_id {
                return true;
            }
            let _ = pending.answer.try_send(Err(failed.clone()));
            false
        });
        self.tabs.strand_host(&host.host_id);
    }

    fn host_of_connection(&self, conn_id: u64) -> Option<usize> {
        self.hosts.iter().position(|host| host.conn_id == conn_id)
    }

    fn route(
        &self,
        workspace_id: Option<&str>,
        browser_id: Option<&str>,
    ) -> Result<&Host, BrowserError> {
        let owner_unavailable = || {
            BrowserError::daemon(
                BrowserErrorCode::OwnerUnavailable,
                "The browser window that owns this tab is no longer connected.",
            )
        };
        match browser_id.map(|id| self.tabs.route(workspace_id, id)) {
            Some(TabRoute::Host(host_id)) => self
                .hosts
                .iter()
                .find(|host| host.host_id == host_id)
                .ok_or_else(owner_unavailable),
            Some(TabRoute::Gone) => Err(owner_unavailable()),
            Some(TabRoute::Unknown) | None => self.hosts.last().ok_or_else(|| {
                BrowserError::daemon(BrowserErrorCode::NoHost, "No browser host is connected.")
            }),
        }
    }

    /// Pick the host for one call and record the call as pending on it.
    pub(crate) fn admit(
        &mut self,
        call: &Call<'_>,
        answer: SyncSender<Answer>,
    ) -> Result<Admitted, BrowserError> {
        let host = self.route(call.workspace_id, call.browser_id)?;
        if !host
            .commands
            .iter()
            .any(|supported| supported == call.command)
        {
            return Err(BrowserError::daemon(
                BrowserErrorCode::UnsupportedCommand,
                "The browser host does not run that command.",
            ));
        }
        let host_id = host.host_id.clone();
        let outbound = Arc::clone(&host.outbound);
        let on_host = self
            .pending
            .values()
            .filter(|pending| pending.host_id == host_id)
            .count();
        if on_host >= MAX_PENDING_PER_HOST || self.pending.len() >= MAX_PENDING_TOTAL {
            return Err(BrowserError::daemon(
                BrowserErrorCode::Busy,
                "The browser host has too many commands in flight.",
            ));
        }
        self.next_request += 1;
        let request_id = format!("browser-{}", self.next_request);
        self.pending.insert(
            request_id.clone(),
            Pending {
                host_id: host_id.clone(),
                command: call.command.to_string(),
                workspace_id: call.workspace_id.map(str::to_string),
                browser_id: call.browser_id.map(str::to_string),
                answer,
            },
        );
        Ok(Admitted {
            request_id,
            host_id,
            outbound,
        })
    }

    /// What a successful answer does to the tab map: a tab-creating command
    /// names the tab it opened, a closing command ends the tab it was aimed at,
    /// and no other command's result is read.
    pub(crate) fn settle_tabs(&mut self, pending: &Pending, result: &Value, request_id: &str) {
        let workspace_id = pending.workspace_id.as_deref();
        if creates_tab(&pending.command) {
            let Some(browser_id) = opened_tab(result) else {
                return;
            };
            if self.tabs.claim(workspace_id, browser_id, &pending.host_id) == Claim::Conflict {
                eprintln!(
                    "browser tab claim ignored: host {} request {request_id} named a tab that is already owned",
                    pending.host_id
                );
            }
        } else if closes_tab(&pending.command) {
            if let Some(browser_id) = pending.browser_id.as_deref() {
                self.tabs.forget(workspace_id, browser_id);
            }
        }
    }

    /// Whether `request_id` is a call still waiting on `host_id`, and
    /// `host_id` is a host that `conn_id` owns: the only answer worth reading.
    pub(crate) fn expects(&self, conn_id: u64, request_id: &str, host_id: &str) -> bool {
        let owns_host = self
            .hosts
            .iter()
            .any(|host| host.host_id == host_id && host.conn_id == conn_id);
        owns_host
            && self
                .pending
                .get(request_id)
                .is_some_and(|pending| pending.host_id == host_id)
    }

    pub(crate) fn take_pending(&mut self, request_id: &str) -> Option<Pending> {
        self.pending.remove(request_id)
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn host_count(&self) -> usize {
        self.hosts.len()
    }
}
