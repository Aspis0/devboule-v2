//! Journal domain — pass-3a split of `server.rs`: the `dispatch_journal`
//! request handler and its usage-shape helper.

use super::*;

pub(super) fn dispatch_journal(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    _passed: &GatePassed,
) -> DaemonMessage {
    match request {
        ClientMessage::JournalUsage { id } => match state.sessions.journal_usage() {
            Ok(usage) => DaemonMessage::JournalUsage {
                id,
                usage: wire_journal_usage(usage),
            },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::JournalRetentionGet { id } => match state.sessions.journal_retention_get() {
            Ok(retention) => DaemonMessage::JournalRetention { id, retention },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::JournalRetentionSet {
            id,
            max_age_ms,
            max_bytes,
            max_sessions,
            session_max_bytes,
            idempotency_key,
        } => {
            let fingerprint = format!(
                "retention:{max_age_ms:?}:{max_bytes:?}:{max_sessions:?}:{session_max_bytes:?}"
            );
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.journal_retention_set(RetentionPatch {
                max_age_ms,
                max_bytes,
                max_sessions,
                session_max_bytes,
            }) {
                Ok(retention) => {
                    let reply = DaemonMessage::JournalRetention { id, retention };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::SessionDelete {
            id,
            session_id,
            idempotency_key,
        } => {
            let fingerprint = format!("delete:{session_id}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.delete_session(&session_id, owner) {
                Ok(()) => {
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::ProjectsList { id } => match state.sessions.projects_list() {
            Ok(projects) => DaemonMessage::Projects { id, projects },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::ProjectAdd { id, path } => match state.sessions.project_add(&path) {
            Ok(project) => DaemonMessage::Project { id, project },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::WorkspacesList { id, project_id } => {
            match state.sessions.workspaces_list(&project_id) {
                Ok(workspaces) => DaemonMessage::Workspaces { id, workspaces },
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::WorkspaceGitStatus { id, workspace_id } => {
            crate::workspace_git_status::reply(state, id, &workspace_id)
        }
        ClientMessage::WorkspaceCreate {
            id,
            project_id,
            isolation,
            branch,
        } => match state
            .sessions
            .workspace_create(&project_id, isolation, branch)
        {
            Ok(workspace) => DaemonMessage::Workspace { id, workspace },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::WorkspaceDelete {
            id,
            workspace_id,
            force,
        } => match state.sessions.workspace_delete(&workspace_id, force) {
            Ok(()) => DaemonMessage::Ok { id },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("unexpected journal frame {other:?}"),
        )),
    }
}

fn wire_journal_usage(usage: crate::journal::JournalUsage) -> WireJournalUsage {
    WireJournalUsage {
        total_bytes: usage.total_bytes,
        session_count: usage.session_count,
        deleted_by_user: usage.deleted_by_user,
        deleted_by_retention: usage.deleted_by_retention,
        unreclaimable: WireUnreclaimable {
            bytes_over: usage.unreclaimable.bytes_over,
            sessions_over: usage.unreclaimable.sessions_over,
            aged_out: usage.unreclaimable.aged_out,
        },
        limits: WireJournalLimits {
            snapshot_every_bytes: usage.limits.snapshot_every_bytes,
            session_max_bytes: usage.limits.session_max_bytes,
            max_bytes: usage.limits.max_bytes,
            max_sessions: usage.limits.max_sessions,
            max_age_ms: usage.limits.max_age_ms,
        },
        per_session: usage
            .per_session
            .into_iter()
            .map(|session| WireJournalSessionUsage {
                id: session.id,
                title: session.title,
                display_name: session.display_name,
                kind: session.kind,
                bytes: session.bytes,
                updated_at_ms: session.updated_at_ms,
            })
            .collect(),
    }
}
