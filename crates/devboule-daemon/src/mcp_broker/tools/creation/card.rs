//! The creation consent card and the facts it prints.

use std::sync::atomic::{AtomicU64, Ordering};

use devboule_protocol::{CreateAgentCard, PermissionOption, SessionEvent, SessionOrigin};

use crate::mcp_broker::caller::McpCaller;
use crate::mcp_broker::ToolsState;
use crate::server::ServerState;

use super::profile::ResolvedProfile;
use super::request::AgentCreateRequest;

/// The one consent-surface fact the composition adds: a peer holding
/// `answer_permissions` may answer its own creation card, and a human
/// reading the card must see that the asking device is also a potential
/// answerer. Nothing gates that — holding `answer_permissions` is the
/// whole grant, so this is a note on the card, not a check: the human
/// keeps the courtesy surface, and the card tells them who else could
/// have answered.
pub(in crate::mcp_broker) fn self_answer_note(
    state: &ServerState,
    caller: &McpCaller,
) -> Option<String> {
    let McpCaller::Peer {
        device_id, caps, ..
    } = caller
    else {
        return None;
    };
    if !caps
        .iter()
        .any(|cap| cap == crate::peer_policy::CAP_ANSWER_PERMISSIONS)
    {
        return None;
    }
    let name = state
        .peer_get(device_id.as_str())
        .ok()
        .flatten()
        .map(|record| record.display_name)
        .unwrap_or_else(|| device_id.clone());
    Some(format!(
        "The asking device '{name}' holds answer_permissions and may answer this card itself."
    ))
}

/// Every line the prompt breaks itself into, split on **all** the
/// terminators a renderer may honour: LF and CRLF, a lone CR, the Unicode
/// separators U+2028/U+2029, NEL U+0085, VT and FF. `str::lines()` sees only
/// LF and CRLF, so a separator from this set would ride inside one marked
/// line and the card could show a visual break without the `| ` prefix that
/// makes the line the profile's rather than the daemon's.
fn marked_prompt_lines(prompt: &str) -> Vec<String> {
    fn is_break(c: char) -> bool {
        matches!(
            c,
            '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{0085}' | '\u{000B}' | '\u{000C}'
        )
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = prompt.chars().peekable();
    while let Some(c) = chars.next() {
        let cr_lf = c == '\r' && chars.peek() == Some(&'\n');
        if is_break(c) {
            lines.push(std::mem::take(&mut current));
            if cr_lf {
                chars.next();
            }
        } else {
            current.push(c);
        }
    }
    lines.push(current);
    lines
}

/// What the approval buys, in the card's own words.
///
/// The gate is the **asking** agent's, so the sentence names that agent
/// rather than leaving "this session" to be read off the line above, which
/// names the child. Its lifetime is the creator entry's, which lives in this
/// daemon's memory: a close or a restart asks again.
pub(crate) const CREATION_GATE_SENTENCE: &str = "Approving lets the agent that asked — not the new one — keep starting agents without asking again, for as long as it stays open.";

/// The creation card (`S5` decisions 4 and 5; `create-from-profile`).
///
/// An ordinary [`SessionEvent::PermissionRequest`] with the `create_agent`
/// payload filled in: the same pending entry, the same allow/deny decision
/// frame, the same origin stamp and per-device budget as any other card. The
/// first line says what will be created, the way a provider's own card names
/// its action; the second says what the answer buys, which neither button
/// name carries. The caps stay in the payload, not in the text.
///
/// A saved spawn prompt is text the person approves, so it is printed in full
/// under the daemon's own lines. Every line of it starts with `| `, and that
/// prefix is the boundary: a prompt cannot put an unmarked line on the card.
pub(in crate::mcp_broker) fn creation_card(
    creator_session_id: &str,
    request: &AgentCreateRequest,
    profile: &ResolvedProfile,
    ticket: &crate::session::AgentCreationTicket<'_>,
    self_answer_note: Option<&str>,
) -> SessionEvent {
    let caps = ticket.caps().clone();
    let card_tools = card_tools_for_provider(&profile.provider);
    let mut description = if profile.model.is_empty() {
        format!("{} · {} (default model)", request.title, profile.provider)
    } else {
        format!("{} · {} {}", request.title, profile.provider, profile.model)
    };
    description.push('\n');
    description.push_str(CREATION_GATE_SENTENCE);
    if let Some(note) = self_answer_note {
        description.push('\n');
        description.push_str(note);
    }
    if !profile.spawn_prompt.is_empty() {
        let quoted = marked_prompt_lines(&profile.spawn_prompt)
            .into_iter()
            .map(|line| format!("| {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        description.push_str(&format!(
            "\n\nSpawn prompt — every line below starts with '|' and is the profile's, not the daemon's:\n{quoted}"
        ));
    }
    SessionEvent::PermissionRequest {
        tool_call_id: creation_permission_id(),
        title: "Create an agent".to_string(),
        description: Some(description),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: "allow".to_string(),
                name: "Create once".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        is_chooser: None,
        kind: None,
        plan: None,
        questions: None,
        // A placeholder: the permission broker stamps the creator's own origin
        // on the way in, exactly as it does for a provider's own card.
        origin: SessionOrigin::unknown(),
        create_agent: Some(Box::new(CreateAgentCard {
            creator_session_id: creator_session_id.to_string(),
            provider: profile.provider.clone(),
            profile: profile.name.clone(),
            title: request.title.clone(),
            tools: card_tools.as_str().to_string(),
            caps,
        })),
    }
}

/// The tools state a creation card promises, from the profile's provider (S2).
///
/// One function so the card cannot drift from the catalog: the provider name
/// is resolved through `provider_catalog::session_kind_for` — the one place a
/// provider name is consulted — and only the resulting `SessionKind` is
/// matched (never a provider string here, per the open-provider rule). S9: all
/// agent families host carriers, so every family promises `Hosted`, with the
/// description carrying "will be verified at start" per the precedence rule
/// (the card promises verification, the result/roster report it).
pub(crate) fn card_tools_for_provider(provider: &str) -> ToolsState {
    let _kind = crate::provider_catalog::session_kind_for(provider);
    ToolsState::Hosted
}

/// The correlation id of one creation card. Distinct per call, like the
/// terminal gate's, so two creations from one session cannot collide in the
/// pending table.
fn creation_permission_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "create:{:x}-{:x}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
