//! The per-session process index: which OS processes a live session proves
//! it owns.
//!
//! Every entry is rooted in the session's own Job Object (Windows) or
//! process group (Unix) and is identified by pid **and** creation time, so a
//! pid the OS has since reused can never answer as the old process. A member
//! the OS will not vouch for is reported as unproven and is never matched,
//! never listed as owned and never a cleanup target. Nothing here reaches
//! outside that proof: detached or escaped descendants are outside it by
//! construction.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use crate::process_argv_redact::redact_argv;
use crate::process_plan::{excluded_agent_chain, CleanupPlan, PlanTarget};
use crate::process_tree::JobObject;

/// One live session's proof root: the job or group every membership claim
/// for that session must come through, plus the two labels a match reports.
pub(crate) struct SessionProof {
    pub(crate) id: String,
    pub(crate) workspace_id: Option<String>,
    pub(crate) label: String,
    pub(crate) job: Arc<JobObject>,
}

/// A proven member of some session's job or group.
#[derive(Clone, Debug)]
pub(crate) struct ProcessEntry {
    pub(crate) pid: u32,
    /// The process's creation time in unix milliseconds. Only ever compared
    /// against itself on this machine: it is the identity that stops a reused
    /// pid from matching an old entry.
    pub(crate) started_at_ms: u64,
    pub(crate) exe: Option<String>,
    /// Already redacted and capped at build time; safe to render.
    pub(crate) argv: Vec<String>,
    pub(crate) ports: Vec<u16>,
    /// `job_member` or `process_group` — how this entry is proven.
    pub(crate) proof: &'static str,
    /// The parent the OS reported at the last refresh: how the agent's own
    /// chain is walked when a cleanup plan decides what to exclude.
    pub(crate) ppid: u32,
    /// The session's own root: the daemon's direct child. Cleanup skips it.
    pub(crate) is_agent: bool,
}

/// What the platform said about one pid. `None` means the OS would not
/// vouch for it — a vanished process, or a query the platform refused.
#[derive(Clone, Debug)]
pub(crate) struct ProcessIdentity {
    pub(crate) started_at_ms: u64,
    pub(crate) ppid: u32,
    pub(crate) exe: Option<String>,
    pub(crate) argv: Vec<String>,
}

/// The OS queries the index answers from. The real one is the platform's
/// (`SystemProbe`); tests substitute scripted answers so identity changes —
/// which no test can make the kernel perform on demand — are observable.
///
/// `begin_refresh` takes the platform's one snapshot for the whole refresh
/// (its helper spawns, its socket-table reads), so members, identities and
/// ports all read the same picture and a refresh costs a handful of calls
/// instead of one per member.
pub(crate) trait ProcessProbe {
    fn begin_refresh(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn members(&self, job: &JobObject) -> Vec<u32>;
    fn identity(&self, pid: u32) -> Option<ProcessIdentity>;
    fn listening_ports(&self) -> Vec<(u16, u32)>;
    fn proof_kind(&self) -> &'static str;
}

/// The real, platform-backed probe.
pub(crate) struct SystemProbe {
    platform: platform::Probe,
}

impl SystemProbe {
    pub(crate) fn new() -> Self {
        Self {
            platform: platform::Probe::new(),
        }
    }
}

impl ProcessProbe for SystemProbe {
    fn begin_refresh(&mut self) -> Result<(), String> {
        self.platform.begin_refresh()
    }

    fn members(&self, job: &JobObject) -> Vec<u32> {
        self.platform.members(job)
    }

    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.platform.identity(pid)
    }

    fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.platform.listening_ports()
    }

    fn proof_kind(&self) -> &'static str {
        platform::PROOF_KIND
    }
}

#[cfg(windows)]
#[path = "process_probe_windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "process_probe_macos.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::{CreationStatus, Membership, ProcessIdentity};
    use crate::process_tree::JobObject;

    pub(crate) const PROOF_KIND: &str = "process_group";

    pub(crate) struct Probe;

    impl Probe {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn begin_refresh(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn members(&self, _job: &JobObject) -> Vec<u32> {
            Vec::new()
        }

        pub(crate) fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
            None
        }

        pub(crate) fn listening_ports(&self) -> Vec<(u16, u32)> {
            Vec::new()
        }
    }

    pub(crate) fn creation_status(_pid: u32) -> CreationStatus {
        CreationStatus::Unverified
    }

    pub(crate) fn membership(_job: &JobObject, _pid: u32) -> Membership {
        Membership::Unreadable
    }
}

/// One session's cached view: its label, its proven entries, and the members
/// the OS refused to vouch for.
struct SessionProcesses {
    workspace_id: Option<String>,
    agent: String,
    entries: HashMap<u32, ProcessEntry>,
    unproven: HashSet<u32>,
    /// The root the first sight recorded: (pid, creation time) of the
    /// daemon's direct child. The exclusion matches this pair, so a stray
    /// merely re-parented to the daemon never inherits the agent's safety.
    agent_root: Option<(u32, u64)>,
}

/// The live, per-session process index. One mutex: refresh and every answer
/// read the same snapshot, so a tool call never sees half a refresh.
pub(crate) struct ProcessIndex {
    sessions: Mutex<HashMap<String, SessionProcesses>>,
}

impl ProcessIndex {
    pub(crate) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionProcesses>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Re-read every rooted session from its proof: members come from the
    /// job or group, identities and ports from the platform's one snapshot.
    /// Every entry is rebuilt from that current read — ports and the exe line
    /// are never frozen at first sight — and a creation time that differs
    /// from the cached entry simply becomes the fresh identity of whatever
    /// holds the pid now. A member with no identity at all is dropped and
    /// remembered as unproven. Sessions no longer rooted here are forgotten.
    pub(crate) fn refresh(&self, roots: Vec<SessionProof>) -> Result<(), String> {
        let mut probe = SystemProbe::new();
        self.refresh_with(roots, &mut probe)
    }

    pub(crate) fn refresh_with(
        &self,
        roots: Vec<SessionProof>,
        probe: &mut dyn ProcessProbe,
    ) -> Result<(), String> {
        probe.begin_refresh()?;
        let mut per_pid_ports: HashMap<u32, Vec<u16>> = HashMap::new();
        for (port, pid) in probe.listening_ports() {
            per_pid_ports.entry(pid).or_default().push(port);
        }
        let daemon_pid = std::process::id();
        let mut sessions = self.sessions();
        sessions.retain(|id, _| roots.iter().any(|root| &root.id == id));
        for root in roots {
            let proof = probe.proof_kind();
            let members: HashSet<u32> = probe.members(&root.job).into_iter().collect();
            let state = sessions
                .entry(root.id.clone())
                .or_insert_with(|| SessionProcesses {
                    workspace_id: None,
                    agent: String::new(),
                    entries: HashMap::new(),
                    unproven: HashSet::new(),
                    agent_root: None,
                });
            state.workspace_id = root.workspace_id.clone();
            state.agent = root.label.clone();
            state.entries.retain(|pid, _| members.contains(pid));
            state.unproven.retain(|pid| members.contains(pid));
            for pid in members {
                let ports = per_pid_ports.get(&pid).cloned().unwrap_or_default();
                let Some(identity) = probe.identity(pid) else {
                    state.entries.remove(&pid);
                    state.unproven.insert(pid);
                    continue;
                };
                state.unproven.remove(&pid);
                state.entries.insert(
                    pid,
                    ProcessEntry {
                        pid,
                        started_at_ms: identity.started_at_ms,
                        exe: identity.exe.clone(),
                        argv: redact_argv(&identity.argv),
                        ports,
                        proof,
                        ppid: identity.ppid,
                        is_agent: identity.ppid == daemon_pid,
                    },
                );
            }
            // Record the root once — the lowest-pid daemon child, which is
            // the session's own spawn before any stray re-parents here — and
            // keep that pair for the session's life.
            let recorded = state
                .agent_root
                .filter(|(pid, _)| state.entries.contains_key(pid));
            state.agent_root = recorded.or_else(|| {
                state
                    .entries
                    .values()
                    .filter(|entry| entry.is_agent)
                    .min_by_key(|entry| entry.pid)
                    .map(|entry| (entry.pid, entry.started_at_ms))
            });
        }
        Ok(())
    }

    /// Every proven entry of one session, sorted by pid.
    pub(crate) fn session_entries(&self, session_id: &str) -> Vec<ProcessEntry> {
        let sessions = self.sessions();
        let Some(state) = sessions.get(session_id) else {
            return Vec::new();
        };
        let mut entries: Vec<ProcessEntry> = state.entries.values().cloned().collect();
        entries.sort_by_key(|entry| entry.pid);
        entries
    }

    /// Entries matching a port or a pid (the tool passes exactly one).
    /// Anything the OS would not vouch for is already outside these answers.
    pub(crate) fn owner_matches(
        &self,
        port: Option<u16>,
        pid: Option<u32>,
    ) -> Vec<(ProcessEntry, String, String, Option<String>)> {
        let sessions = self.sessions();
        let mut matches = Vec::new();
        for (session_id, state) in sessions.iter() {
            for entry in state.entries.values() {
                let hit = match (port, pid) {
                    (Some(port), None) => entry.ports.contains(&port),
                    (None, Some(pid)) => entry.pid == pid,
                    _ => false,
                };
                if hit {
                    matches.push((
                        entry.clone(),
                        session_id.clone(),
                        state.agent.clone(),
                        state.workspace_id.clone(),
                    ));
                }
            }
        }
        matches.sort_by_key(|left| left.0.pid);
        matches
    }

    /// The cleanup plan for one session: the proven members that are not in
    /// the agent chain — the recorded root by pid and creation time, plus
    /// any member above it, each exclusion named — plus the members the OS
    /// refused to vouch for. The plan carries every target's creation time
    /// so the signal can prove identity again at the moment it acts.
    pub(crate) fn cleanup_plan(&self, session_id: &str) -> Option<CleanupPlan> {
        let sessions = self.sessions();
        let state = sessions.get(session_id)?;
        let excluded = excluded_agent_chain(&state.entries, state.agent_root);
        let excluded_pids: HashSet<u32> = excluded.iter().map(|skip| skip.pid).collect();
        let targets = state
            .entries
            .values()
            .filter(|entry| !excluded_pids.contains(&entry.pid))
            .map(|entry| PlanTarget {
                pid: entry.pid,
                started_at_ms: entry.started_at_ms,
                exe: entry.exe.clone(),
            })
            .collect();
        let mut unproven: Vec<u32> = state.unproven.iter().copied().collect();
        unproven.sort_unstable();
        Some(CleanupPlan {
            targets,
            excluded,
            unproven,
        })
    }

    /// The human label a card or a list shows for one live session.
    pub(crate) fn session_label(&self, session_id: &str) -> Option<String> {
        self.sessions()
            .get(session_id)
            .map(|state| state.agent.clone())
    }
}

/// What the platform says about one pid's creation time at the moment of a
/// signal — the identity check immediately before anything is signalled.
/// FILETIME counts 100 ns ticks since 1601; the shift to unix milliseconds
/// every Windows-side read applies (the two reads — index and terminate —
/// share it so their identities compare equal).
#[cfg(windows)]
pub(crate) const FILETIME_EPOCH_MS: u64 = 11_644_473_600_000;

#[derive(Debug)]
pub(crate) enum CreationStatus {
    /// The pid does not, any longer, name a process.
    Gone,
    /// A process created at this time exists.
    At(u64),
    /// The creation time could not be read: never signal blind.
    Unverified,
}

/// The platform's creation-time read, for the terminate-time identity check.
pub(crate) fn creation_status(pid: u32) -> CreationStatus {
    platform::creation_status(pid)
}

/// Whether a pid is in the session's job or group at this moment.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Membership {
    Member,
    Outside,
    /// The kernel could not be asked: never signal blind.
    Unreadable,
}

/// A fresh read of the session's proof for one pid — not the cached index —
/// for the terminate-time ownership check.
pub(crate) fn membership(job: &JobObject, pid: u32) -> Membership {
    platform::membership(job, pid)
}

#[cfg(test)]
#[path = "process_index_tests.rs"]
mod tests;
