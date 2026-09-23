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
        ClientMessage::WorkspaceGitDiff {
            id,
            workspace_id,
            path,
        } => crate::workspace_git_diff::reply(state, id, &workspace_id, &path),
        // The four git writes: keyed like the file writes below (a retry
        // with the same key replays the first success instead of acting
        // twice — a second commit would find nothing staged), and only a
        // success is remembered: a refusal costs nothing to retry, and
        // caching it would freeze a sentence over an index that has since
        // changed.
        ClientMessage::WorkspaceGitStage {
            id,
            workspace_id,
            paths,
            idempotency_key,
        } => {
            let fingerprint = format!("stage:{workspace_id}:{paths:?}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply = crate::workspace_git_write::reply_stage(state, id, &workspace_id, &paths);
            if matches!(&reply, DaemonMessage::WorkspaceGitWrite { error: None, .. }) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        ClientMessage::WorkspaceGitUnstage {
            id,
            workspace_id,
            paths,
            idempotency_key,
        } => {
            let fingerprint = format!("unstage:{workspace_id}:{paths:?}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply = crate::workspace_git_write::reply_unstage(state, id, &workspace_id, &paths);
            if matches!(&reply, DaemonMessage::WorkspaceGitWrite { error: None, .. }) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        ClientMessage::WorkspaceGitDiscard {
            id,
            workspace_id,
            paths,
            idempotency_key,
        } => {
            let fingerprint = format!("discard:{workspace_id}:{paths:?}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply = crate::workspace_git_write::reply_discard(state, id, &workspace_id, &paths);
            if matches!(&reply, DaemonMessage::WorkspaceGitWrite { error: None, .. }) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        ClientMessage::WorkspaceGitCommit {
            id,
            workspace_id,
            message,
            idempotency_key,
        } => {
            let fingerprint = format!("commit:{workspace_id}:{message}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply =
                crate::workspace_git_write::reply_commit(state, id, &workspace_id, &message);
            if matches!(&reply, DaemonMessage::WorkspaceGitWrite { error: None, .. }) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        ClientMessage::WorkspaceFilesList {
            id,
            workspace_id,
            path,
        } => crate::workspace_files::reply(state, id, &workspace_id, &path),
        ClientMessage::WorkspaceFileRead {
            id,
            workspace_id,
            path,
            from_line,
            line_count,
        } => crate::workspace_file_read::reply(
            state,
            id,
            &workspace_id,
            &path,
            from_line,
            line_count,
        ),
        // The two write acts: keyed like the other keyed writes here (a
        // retry with the same key replays the first success instead of
        // acting twice — the second rename would find nothing to rename),
        // and only a success is remembered: a refusal costs nothing to
        // retry, and caching it would freeze a sentence over a tree that
        // has since changed.
        ClientMessage::WorkspaceFileRename {
            id,
            workspace_id,
            path,
            name,
            idempotency_key,
        } => {
            let fingerprint = format!("rename:{workspace_id}:{path}:{name}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply = crate::workspace_file_mutations::reply_rename(
                state,
                id,
                &workspace_id,
                &path,
                &name,
            );
            if matches!(
                &reply,
                DaemonMessage::WorkspaceFileRenamed {
                    change: devboule_protocol::WorkspaceFileMutation {
                        new_path: Some(_),
                        ..
                    },
                    ..
                }
            ) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        ClientMessage::WorkspaceFileDuplicate {
            id,
            workspace_id,
            path,
            idempotency_key,
        } => {
            let fingerprint = format!("duplicate:{workspace_id}:{path}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply =
                crate::workspace_file_mutations::reply_duplicate(state, id, &workspace_id, &path);
            if matches!(
                &reply,
                DaemonMessage::WorkspaceFileDuplicated {
                    change: devboule_protocol::WorkspaceFileMutation {
                        new_path: Some(_),
                        ..
                    },
                    ..
                }
            ) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        // The delete is keyed like the two writes above — the one act where
        // a replayed "success" over an already-dead path would answer
        // `does not exist` instead of the first success — and only a
        // success is remembered (a refusal costs nothing to retry, and
        // caching it would freeze a sentence over a tree that has since
        // changed). The success carries no new path: the shape this match
        // pins is "no error", the delete's own silence.
        ClientMessage::WorkspaceFileDelete {
            id,
            workspace_id,
            path,
            idempotency_key,
        } => {
            let fingerprint = format!("delete:{workspace_id}:{path}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply =
                crate::workspace_file_mutations::reply_delete(state, id, &workspace_id, &path);
            if matches!(
                &reply,
                DaemonMessage::WorkspaceFileDeleted {
                    change: devboule_protocol::WorkspaceFileMutation { error: None, .. },
                    ..
                }
            ) {
                remember(
                    state,
                    owner,
                    idempotency_key.as_deref(),
                    &fingerprint,
                    &reply,
                );
            }
            reply
        }
        // The preview's two frames: no idempotency key on either, and none
        // wanted — a re-staged copy is the same copy (every stage clears
        // the folder first) and a re-run unstage deletes a folder that is
        // already gone, so replaying either is the same act.
        ClientMessage::WorkspaceFilePreviewStage {
            id,
            workspace_id,
            path,
        } => crate::workspace_file_preview::reply_stage(state, id, &workspace_id, &path),
        ClientMessage::WorkspaceFilePreviewUnstage { id } => {
            crate::workspace_file_preview::reply_unstage(state, id)
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
