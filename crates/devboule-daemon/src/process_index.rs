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

use crate::process_argv_redact::{redact_argv, redact_exe};
use crate::process_plan::{
    outside_the_tree, provider_tree, CleanupPlan, PlanTarget, SkippedPlan, MEMBER_UNPROVEN,
};
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
    pub(crate) started_at_ticks: u64,
    pub(crate) exe: Option<String>,
    /// Already redacted and capped at build time; safe to render.
    pub(crate) argv: Vec<String>,
    pub(crate) ports: Vec<u16>,
    /// `job_member` or `process_group` — how this entry is proven.
    pub(crate) proof: &'static str,
    /// The parent the OS reported at the last refresh: how the agent's own
    /// chain is walked when a cleanup plan decides what to exclude.
    pub(crate) ppid: u32,
    /// The daemon's direct child: a provider root. Cleanup never signals it
    /// or anything in its tree.
    pub(crate) is_agent: bool,
    /// The parent's creation time, when the parent is alive outside this
    /// membership and the platform keeps parent links across an exit. This is
    /// the only lineage that proves a member is outside the provider tree.
    pub(crate) outside_parent_started_at_ticks: Option<u64>,
}

/// What the platform said about one pid. `None` means the OS would not
/// vouch for it — a vanished process, or a query the platform refused.
#[derive(Clone, Debug)]
pub(crate) struct ProcessIdentity {
    pub(crate) started_at_ticks: u64,
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
    /// The members the job holds now. An error means the list cannot be read
    /// whole: the session's plan is then refused, never drawn from a partial list.
    fn members(&self, job: &JobObject) -> Result<Vec<u32>, String>;
    fn identity(&self, pid: u32) -> Option<ProcessIdentity>;
    fn listening_ports(&self) -> Vec<(u16, u32)>;
    fn proof_kind(&self) -> &'static str;
    /// Whether a member keeps its parent link after the parent exits. Windows
    /// does; Unix re-parents the orphan to init, so a link to a live parent
    /// there proves nothing about the provider tree.
    fn parent_links_survive_exit(&self) -> bool {
        false
    }
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

    fn members(&self, job: &JobObject) -> Result<Vec<u32>, String> {
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

    fn parent_links_survive_exit(&self) -> bool {
        platform::PARENT_LINKS_SURVIVE_EXIT
    }
}

#[cfg(windows)]
#[path = "process_probe_windows.rs"]
mod platform;

#[cfg(windows)]
pub(crate) use platform::{process_parents, read_creation_time};

#[cfg(target_os = "macos")]
#[path = "process_probe_macos.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::{CreationStatus, Membership, ProcessIdentity};
    use crate::process_tree::JobObject;

    pub(crate) const PROOF_KIND: &str = "process_group";
    pub(crate) const PARENT_LINKS_SURVIVE_EXIT: bool = false;

    pub(crate) struct Probe;

    impl Probe {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn begin_refresh(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub(crate) fn members(&self, _job: &JobObject) -> Result<Vec<u32>, String> {
            Ok(Vec::new())
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
    /// The last refresh could not read the job's member list whole: no plan is
    /// drawn from this session until a refresh reads it.
    membership_unreadable: bool,
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
        let links_survive = probe.parent_links_survive_exit();
        let mut sessions = self.sessions();
        sessions.retain(|id, _| roots.iter().any(|root| &root.id == id));
        for root in roots {
            let proof = probe.proof_kind();
            let (members, unreadable): (HashSet<u32>, bool) = match probe.members(&root.job) {
                Ok(members) => (members.into_iter().collect(), false),
                Err(_) => (HashSet::new(), true),
            };
            let state = sessions
                .entry(root.id.clone())
                .or_insert_with(|| SessionProcesses {
                    workspace_id: None,
                    agent: String::new(),
                    entries: HashMap::new(),
                    unproven: HashSet::new(),
                    membership_unreadable: false,
                    agent_root: None,
                });
            state.workspace_id = root.workspace_id.clone();
            state.agent = root.label.clone();
            state.membership_unreadable = unreadable;
            state.entries.retain(|pid, _| members.contains(pid));
            state.unproven.retain(|pid| members.contains(pid));
            for &pid in &members {
                let ports = per_pid_ports.get(&pid).cloned().unwrap_or_default();
                let Some(identity) = probe.identity(pid) else {
                    state.entries.remove(&pid);
                    state.unproven.insert(pid);
                    continue;
                };
                state.unproven.remove(&pid);
                let outside_parent_started_at_ticks =
                    if links_survive && !members.contains(&identity.ppid) {
                        probe
                            .identity(identity.ppid)
                            .map(|parent| parent.started_at_ticks)
                    } else {
                        None
                    };
                state.entries.insert(
                    pid,
                    ProcessEntry {
                        pid,
                        started_at_ticks: identity.started_at_ticks,
                        exe: identity.exe.as_deref().map(redact_exe),
                        argv: redact_argv(&identity.argv),
                        ports,
                        proof,
                        ppid: identity.ppid,
                        is_agent: identity.ppid == daemon_pid,
                        outside_parent_started_at_ticks,
                    },
                );
            }
            // Keep the recorded pair while the same process still holds its
            // pid and creation time; a reused pid must not inherit it.
            let recorded = state.agent_root.filter(|(pid, started_at)| {
                state
                    .entries
                    .get(pid)
                    .is_some_and(|entry| entry.started_at_ticks == *started_at)
            });
            state.agent_root = recorded.or_else(|| {
                state
                    .entries
                    .values()
                    .filter(|entry| entry.is_agent)
                    .min_by_key(|entry| entry.pid)
                    .map(|entry| (entry.pid, entry.started_at_ticks))
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

    /// The cleanup plan for one session: every proven member except the
    /// daemon, the session's provider tree, and any other live session's
    /// provider tree — each exclusion named — plus the members the OS refused
    /// to vouch for. `None` for an unknown session; `Err` when this session's
    /// provider tree cannot be proved, so the caller refuses. The plan carries
    /// every target's creation time so the signal can prove identity again.
    pub(crate) fn cleanup_plan(
        &self,
        session_id: &str,
    ) -> Option<Result<CleanupPlan, &'static str>> {
        let sessions = self.sessions();
        let state = sessions.get(session_id)?;
        if state.membership_unreadable || !state.unproven.is_empty() {
            return Some(Err(MEMBER_UNPROVEN));
        }
        let tree = match provider_tree(&state.entries, state.agent_root) {
            Ok(tree) => tree,
            Err(reason) => return Some(Err(reason)),
        };
        let daemon = std::process::id();
        let mut named: HashMap<u32, &'static str> = tree
            .into_iter()
            .map(|skip| (skip.pid, skip.reason))
            .collect();
        if state.entries.contains_key(&daemon) {
            named.insert(daemon, "daemon");
        }
        let mut elsewhere: HashSet<u32> = HashSet::new();
        for (_, other) in sessions.iter().filter(|(id, _)| id.as_str() != session_id) {
            match provider_tree(&other.entries, other.agent_root) {
                Ok(tree) => elsewhere.extend(tree.iter().map(|skip| skip.pid)),
                Err(_) => elsewhere.extend(other.entries.keys().copied()),
            }
        }
        for pid in state.entries.keys().filter(|pid| elsewhere.contains(pid)) {
            named.entry(*pid).or_insert("other_session_provider");
        }
        for entry in state.entries.values() {
            if !named.contains_key(&entry.pid) && !outside_the_tree(entry) {
                named.insert(entry.pid, "lineage_unproven");
            }
        }
        let mut excluded: Vec<SkippedPlan> = named
            .into_iter()
            .map(|(pid, reason)| SkippedPlan { pid, reason })
            .collect();
        excluded.sort_by_key(|skip| skip.pid);
        let excluded_pids: HashSet<u32> = excluded.iter().map(|skip| skip.pid).collect();
        let targets = state
            .entries
            .values()
            .filter(|entry| !excluded_pids.contains(&entry.pid))
            .map(|entry| PlanTarget {
                pid: entry.pid,
                started_at_ticks: entry.started_at_ticks,
                exe: entry.exe.clone(),
            })
            .collect();
        let mut unproven: Vec<u32> = state.unproven.iter().copied().collect();
        unproven.sort_unstable();
        Some(Ok(CleanupPlan {
            targets,
            excluded,
            unproven,
        }))
    }

    /// The human label a card or a list shows for one live session.
    pub(crate) fn session_label(&self, session_id: &str) -> Option<String> {
        self.sessions()
            .get(session_id)
            .map(|state| state.agent.clone())
    }
}

/// Creation times are unix time in 100 ns ticks, the unit of a FILETIME. The
/// shift from FILETIME's 1601 epoch is applied in one place, so the index and
/// the terminate-time check compare the same numbers.
#[cfg(windows)]
pub(crate) const FILETIME_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

/// A FILETIME read as unix ticks, keeping every 100 ns of it.
#[cfg(windows)]
pub(crate) fn unix_ticks_from_filetime(filetime: u64) -> u64 {
    filetime.saturating_sub(FILETIME_EPOCH_TICKS)
}

/// Whether a creation time read by the platform is full resolution. Windows
/// reports 100 ns ticks; a coarser platform cannot tell two processes with
/// the same time apart, so an equal time proves nothing there.
pub(crate) const CREATION_TICKS_PRECISE: bool = cfg!(windows);

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

/// Whether a signalled pid is gone for good: unlisted, or a zombie awaiting its
/// parent's reap (macOS, where signal 0 still answers for a zombie).
#[cfg(target_os = "macos")]
pub(crate) fn has_exited(pid: u32) -> bool {
    platform::has_exited(pid)
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
#[path = "process_index_fakes.rs"]
mod fakes;

#[cfg(test)]
#[path = "process_index_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "process_chain_tests.rs"]
mod chain_tests;
