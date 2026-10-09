//! The remote-host surface: the three lists a held peer link may read, the
//! body one of them comes back in, and the state a host row is in when it is
//! not online.
//!
//! The list enum is the **whole** vocabulary of the link. There is no
//! "forward this arbitrary frame to that peer" envelope, so a fourth read
//! cannot be constructed, let alone sent: a caller that wants to reach a peer
//! names a variant here, and the variant's peer request is the only frame it
//! can produce. The remote's `run_gate` stays the authority over each one.

use serde::{Deserialize, Serialize};

use crate::{ClientMessage, DaemonMessage, Project, Session, Workspace};

/// One of the three allowlisted reads a host link may carry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RemoteHostList {
    Projects,
    Workspaces { project_id: String },
    Sessions,
}

impl RemoteHostList {
    /// The request this read puts on the wire toward the paired daemon.
    ///
    /// These are the far daemon's own frames, not a wrapper carrying a target:
    /// this daemon is the peer of record there, so there is nothing for the
    /// remote to route and no header it could be tricked with.
    pub fn peer_request(&self, id: u64) -> ClientMessage {
        match self {
            Self::Projects => ClientMessage::ProjectsList { id },
            Self::Workspaces { project_id } => ClientMessage::WorkspacesList {
                id,
                project_id: project_id.clone(),
            },
            Self::Sessions => ClientMessage::SessionsList { id },
        }
    }

    /// The remote's answer as this list's body, or `None` when `reply` is not
    /// the answer to this read.
    ///
    /// `None` must not be papered over with an empty body: a reply of another
    /// shape would then reach the app dressed as the remote's own answer.
    pub fn body_from_reply(&self, reply: &DaemonMessage) -> Option<RemoteHostListBody> {
        match (self, reply) {
            (Self::Projects, DaemonMessage::Projects { projects, .. }) => {
                Some(RemoteHostListBody::Projects {
                    rows: projects.clone(),
                })
            }
            (Self::Workspaces { .. }, DaemonMessage::Workspaces { workspaces, .. }) => {
                Some(RemoteHostListBody::Workspaces {
                    rows: workspaces.clone(),
                })
            }
            (Self::Sessions, DaemonMessage::Sessions { sessions, .. }) => {
                Some(RemoteHostListBody::Sessions {
                    rows: sessions.clone(),
                })
            }
            _ => None,
        }
    }
}

/// The remote daemon's answer to one list, carried through unchanged.
///
/// Each arm holds exactly the rows the far daemon sent, in its own types. No
/// arm is filled in locally, and a refusal is not one of them: a refusal comes
/// back as the remote's own typed error, with its reason intact.
/// `rows` is the far daemon's own list, one arm per read. It is always
/// present, empty or not: the app reads `body.rows` without asking whether the
/// machine had anything, and an empty list is an answer rather than a gap.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "list", rename_all = "snake_case")]
pub enum RemoteHostListBody {
    Projects {
        #[serde(default)]
        rows: Vec<Project>,
    },
    Workspaces {
        #[serde(default)]
        rows: Vec<Workspace>,
    },
    Sessions {
        #[serde(default)]
        rows: Vec<Session>,
    },
}

/// One host row's state, as the app's sidebar reads it.
///
/// These are seven different facts with seven different repairs, so they stay
/// seven states: a revoked peer, a missing local key, a version skew and a
/// saturated budget must never collapse into "offline", which would hide the
/// only step that fixes each of them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteHostState {
    /// A link is being opened or reopened for this host.
    Connecting,
    /// The link is up: the hello was answered and the host answers reads.
    Online,
    /// The host stopped answering — two missed probes, or a closed transport.
    Offline,
    /// No live `peers` row carries this device, or the row was revoked.
    NeedsPairing,
    /// This daemon has no device key of its own, so it cannot dial anybody.
    IdentityMissing,
    /// The far daemon's hello did not negotiate what this read needs.
    Unsupported,
    /// The link or the RPC budget is spent; nothing was opened for this ask.
    Busy,
}

/// One host's state change, pushed to the connections that watch that host.
/// Camel-cased like every other frame the app reads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostStatus {
    pub device_id: String,
    pub state: RemoteHostState,
    /// One plain English sentence for the empty state, never an error dump.
    /// Absent when there is nothing to explain (`connecting`, `online`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
    /// The host's workspace revision, on a push that reports one: the host
    /// pushed [`crate::DaemonMessage::HostWorkspaceChanged`] over the link,
    /// and this is the number it carried. A watcher reloads that host's
    /// project/workspace snapshots when the number moves or the link comes
    /// back. Absent on a state change with nothing new to reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}
