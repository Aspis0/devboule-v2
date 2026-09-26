use std::sync::Arc;

use devboule_protocol::OwnerId;

use crate::server::ServerState;

pub(super) struct CloseSessionResult {
    pub(super) closed_session_ids: Vec<String>,
    pub(super) audited_session_ids: Vec<String>,
}

#[derive(Debug)]
pub(super) struct CloseSessionsFailure {
    pub(super) message: String,
    pub(super) closed_session_ids: Vec<String>,
}

pub(super) fn close_sessions(
    state: &Arc<ServerState>,
    sessions: Vec<(String, OwnerId, String)>,
    after_close: &mut impl FnMut(),
) -> Result<CloseSessionResult, CloseSessionsFailure> {
    let mut closed_session_ids = Vec::new();
    let mut audited_session_ids = Vec::new();
    for (session_id, owner, _) in sessions {
        match state.sessions.close(&session_id, &owner, &None) {
            Ok(true) => {
                state.session_finished();
                closed_session_ids.push(session_id.clone());
                audited_session_ids.push(session_id);
            }
            Ok(false) => closed_session_ids.push(session_id),
            Err(error) => {
                return Err(CloseSessionsFailure {
                    message: format!("Could not close session {session_id}: {}", error.message),
                    closed_session_ids,
                });
            }
        }
        after_close();
    }
    Ok(CloseSessionResult {
        closed_session_ids,
        audited_session_ids,
    })
}

pub(super) fn describe_sessions(
    sessions: &[(String, OwnerId, String)],
    caller: &OwnerId,
) -> String {
    let own_sessions = sessions
        .iter()
        .filter(|(_, owner, _)| owner == caller)
        .collect::<Vec<_>>();
    let other_user_count = sessions.len() - own_sessions.len();
    let names = own_sessions
        .iter()
        .take(3)
        .map(|(_, _, title)| quote_title(title))
        .collect::<Vec<_>>();
    let own_remaining = own_sessions.len().saturating_sub(names.len());
    let mut parts = Vec::new();
    if !names.is_empty() {
        parts.push(names.join(", "));
    }
    if own_remaining > 0 {
        parts.push(format!("and {own_remaining} more"));
    }
    if other_user_count > 0 {
        let other_sessions = if other_user_count == 1 {
            "1 session of another user".to_string()
        } else {
            format!("{other_user_count} sessions of other users")
        };
        parts.push(format!("and {other_sessions}"));
    }
    let summary = if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    };
    format!("{} session(s): {summary}", sessions.len())
}

pub(super) fn quote_title(title: &str) -> String {
    let title = title
        .chars()
        .take(120)
        .map(|char| {
            if char.is_control() || matches!(char, '\\' | '"' | '\'') {
                '_'
            } else {
                char
            }
        })
        .collect::<String>();
    format!("\"{title}\"")
}
