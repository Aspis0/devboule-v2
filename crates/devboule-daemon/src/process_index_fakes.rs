//! Scripted probes and fixtures shared by the index's test modules: one
//! probe's reality per refresh, and the identities and proofs it hands out.

use std::collections::HashMap;
use std::sync::Arc;

use crate::process_tree::JobObject;

use super::*;

/// One probe's scripted reality: the members the proof admits, what the OS
/// says about each, and the port table.
pub(super) struct FakeProbe {
    pub(super) members: Vec<u32>,
    pub(super) identities: HashMap<u32, ProcessIdentity>,
    pub(super) ports: Vec<(u16, u32)>,
}

impl ProcessProbe for FakeProbe {
    fn members(&self, _job: &JobObject) -> Result<Vec<u32>, String> {
        Ok(self.members.clone())
    }

    fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.identities.get(&pid).cloned()
    }

    fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.ports.clone()
    }

    fn proof_kind(&self) -> &'static str {
        "job_member"
    }

    /// Modelled on Windows: a member's ppid keeps the dead parent's number.
    fn parent_links_survive_exit(&self) -> bool {
        true
    }
}

pub(super) fn identity(started_at_ms: u64, ppid: u32) -> ProcessIdentity {
    identity_with(started_at_ms, ppid, "/usr/local/bin/tool")
}

pub(super) fn identity_with(started_at_ms: u64, ppid: u32, exe: &str) -> ProcessIdentity {
    ProcessIdentity {
        started_at_ms,
        ppid,
        exe: Some(exe.to_string()),
        argv: vec![exe.to_string(), "--serve".to_string()],
    }
}

pub(super) fn proof(id: &str, workspace: Option<&str>) -> SessionProof {
    SessionProof {
        id: id.to_string(),
        workspace_id: workspace.map(str::to_string),
        label: format!("agent {id}"),
        job: Arc::new(JobObject::new().expect("a proof handle")),
    }
}

pub(super) fn refresh(index: &ProcessIndex, roots: Vec<SessionProof>, probe: &mut FakeProbe) {
    index.refresh_with(roots, probe).expect("fake probe works");
}
