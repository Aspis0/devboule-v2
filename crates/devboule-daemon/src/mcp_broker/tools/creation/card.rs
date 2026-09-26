//! The creation consent card and the facts it prints.

use std::sync::atomic::{AtomicU64, Ordering};

use devboule_protocol::{CreateAgentCard, PermissionOption, SessionEvent, SessionOrigin};

use crate::mcp_broker::caller::McpCaller;
use crate::mcp_broker::ToolsState;
use crate::server::ServerState;

use super::profile::{predicted_unattended, ResolvedProfile};
use super::request::AgentCreateRequest;

/// The card's auto-accept line, worded per the tri-state's own rule — never
/// assert the negative, and never assert the affirmative of the unknown: a
/// mode whose vocabulary is the agent's own is "cannot establish", not "No",
/// because the child may yet run without asking. One function so the wording
/// and its test cannot disagree.
pub(in crate::mcp_broker) fn auto_accept_line(
    answer: devboule_protocol::UnattendedState,
    mode_id: &str,
) -> String {
    match answer {
        devboule_protocol::UnattendedState::Yes => format!("Yes (mode {mode_id})"),
        devboule_protocol::UnattendedState::No => format!("No — mode {mode_id} asks the human"),
        devboule_protocol::UnattendedState::Unknown => format!(
            "Cannot establish — mode {mode_id} belongs to the agent's own vocabulary, so whether it asks is not something Devboule can check"
        ),
    }
}

/// The one consent-surface fact the composition adds (F1, decided): a peer
/// holding `answer_permissions` may answer its own creation card, and a human
/// reading the card must see that the asking device is also a potential
/// answerer. Paseo's model is the reference — its `create_agent_request`
/// needs the capability pair and shows no card at all, the grant IS the
/// consent — so this is not a gate to add but a fact to state on the card we
/// keep as the human's courtesy surface.
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

/// The creation card (`S5` decisions 4 and 5; `create-from-profile`).
///
/// An ordinary [`SessionEvent::PermissionRequest`] with the `create_agent`
/// payload filled in: the same pending entry, the same allow/deny decision
/// frame, the same origin stamp and per-device budget as any other card. The
/// caps are in the text *and* in the payload — the text is what a person reads,
/// the payload is what a surface renders, and both come from one reservation.
///
/// The text states what the human is being asked to **approve**, which is the
/// profile and what it resolves to: the provider, the model, the mode, the
/// thinking option, whether the child will approve prompts in their place (and
/// which mode does the answering), the feature values the child is started with —
/// each with the value the wire will carry, and, for an ACP provider no read has
/// answered, the condition the agent's own handshake will place on it — and the
/// caller's labels. A card that named only the profile would ask for a
/// decision against a word, and the word is the one thing the human cannot
/// check without opening Settings.
///
/// With the creation refusing every value the clients cannot deliver, this
/// text is honest by construction rather than by wording: a card a human can
/// approve into an existing child prints only what the child was delivered.
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

pub(in crate::mcp_broker) fn creation_card(
    creator_session_id: &str,
    creator_name: &str,
    request: &AgentCreateRequest,
    profile: &ResolvedProfile,
    labels: &std::collections::BTreeMap<String, String>,
    ticket: &crate::session::AgentCreationTicket<'_>,
    self_answer_note: Option<&str>,
) -> SessionEvent {
    let caps = ticket.caps().clone();
    // `Auto accept: Yes` is the one phrase that has to be readable at a glance:
    // it is the difference between a child that will ask this human and one that
    // will not.
    //
    // `autoAccept` is a constraint on the delivered mode, so the auto-accept
    // line renders it — naming the **mode** that does the answering, because
    // consent to a mechanism is not consent to a word. Every other key printed
    // here is read through the same function the child's delivery is built by,
    // so the card and the delivery cannot name two different lists: what this
    // line prints **is** what the child is started with.
    //
    // The old sentence — "not interpreted by this daemon; carried but never
    // delivered" — is gone because nothing that can reach this line is like
    // that any more. `check_profile` prunes a key the family does not declare,
    // and each client refuses a value it cannot put on the wire rather than
    // starting a child without it; the keys left are exactly the ones the
    // spawn applies. Absent stays a third state here, never a silence and never
    // a claim.
    let features = if profile.features.is_empty() {
        "none".to_string()
    } else {
        let delivered = crate::profile_delivery::delivered_features(&profile.features);
        if delivered.is_empty() {
            "none".to_string()
        } else {
            let listed = delivered
                .iter()
                .map(|feature| format!("{}={}", feature.id(), feature.printed()))
                .collect::<Vec<_>>()
                .join(", ");
            // ACP is the one family whose list the daemon does not own, so where
            // no read has answered it yet the card says what happens when the
            // child's own handshake disagrees: the creation is refused, not
            // started without the value. Every other family's table is the
            // daemon's own, so a key that reached this line is a key the launch
            // applies — and there a caveat would be a hedge about nothing.
            if crate::provider_catalog::session_kind_for(&profile.provider)
                == devboule_protocol::SessionKind::Acp
                && crate::provider_feature_probe::cached_declarations(
                    &crate::provider_feature_probe::ProbeKey::new(&profile.provider),
                )
                .is_none()
            {
                format!(
                    "{listed} (each set on the child as the agent declares it; a feature this agent does not declare refuses the creation rather than starting without it)"
                )
            } else {
                listed
            }
        }
    };
    // The card's auto-accept line reads the mode, and only the mode, through
    // the same prediction the birth will apply: the tick over an asking mode
    // is refused before this card is raised (the R2a audit's F7), so on every
    // card carrying a tick, "Yes" names the mode that does the answering.
    let auto = auto_accept_line(
        predicted_unattended(&profile.provider, &profile.mode),
        &profile.mode,
    );
    let thinking = profile.thinking_option_id.as_deref().unwrap_or("none");
    // The spawn prompt the child will receive, on the card in full: approving
    // this card approves injected text, so hiding it behind a profile name
    // would make the approval say less than it does. It is a delimited block
    // whose **every line is prefixed with `| `**, and that prefix is the
    // boundary: a prompt may contain quotes, newlines, or lines that imitate
    // the card's own "Labels:"/"Caps:" metadata, and an inline interpolation
    // — quoted or not — would let any of them forge it. Prefixed, a forged
    // terminator is just another marked line, and the daemon-written metadata
    // stays unprefixed and recognisable. The store bounds the prompt, so
    // echoing it by value is the same rule every bounded value here follows.
    // Absent stays silent — a profile without one adds no block and no
    // promise.
    let spawn_block = if profile.spawn_prompt.is_empty() {
        String::new()
    } else {
        let quoted = marked_prompt_lines(&profile.spawn_prompt)
            .into_iter()
            .map(|line| format!("| {line}"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        format!(
            "
 Spawn prompt — every line below starts with '|' and is the profile's, not the daemon's:
{quoted}
"
        )
    };
    // The caller's own labels, and only those: the daemon's four `devboule.`
    // keys are stamped at the creation and would tell the human nothing they are
    // not already reading on this card.
    let labels = if labels.is_empty() {
        "none".to_string()
    } else {
        labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // S2 honesty: the card promises verification (precedence rule). The tools
    // state comes from the one function above; the sentence is glued to the
    // description and the word rides the payload.
    let card_tools = card_tools_for_provider(&profile.provider);
    let tools_sentence = card_tools_sentence(card_tools);
    // The consent surface names its own composition (F1, decided): a paired
    // device holding `answer_permissions` may answer this card itself, and a
    // human reading it must be able to see that the asking device is also a
    // potential answerer. Paseo's model is the reference: its
    // `create_agent_request` needs the capability pair and shows no card at
    // all - the grant IS the consent. Ours keeps the card as the human's
    // courtesy surface and states the fact on it.
    let self_answer = match self_answer_note {
        Some(note) => format!(" {note}"),
        None => String::new(),
    };
    // The profile facts and the decision metadata are built apart, because a
    // spawn-prompt block slots between them: its lines are all marked with
    // `| `, and the metadata that follows is the card's own unprefixed voice.
    let facts = format!(
        "Asked for by '{creator_name}'. Profile '{name}' ({id}): provider {provider}, model {model}, mode {mode}, thinking {thinking}, features {features}, auto accept: {auto}.",
        name = profile.name,
        id = profile.id,
        provider = profile.provider,
        model = profile.model,
        mode = profile.mode,
        auto = auto,
    );
    let metadata = format!(
        "Labels: {labels}. Caps: live children {} of {}, creations this hour {} of {}, depth {} of {}, live agent sessions {} of {}.{tools_sentence}{self_answer}",
        caps.live_children,
        caps.max_live_children,
        caps.creations_this_hour,
        caps.max_creations_per_hour,
        caps.depth,
        caps.max_depth,
        caps.live_agent_sessions,
        caps.max_live_agent_sessions,
        tools_sentence = tools_sentence,
        self_answer = self_answer,
    );
    let description = if spawn_block.is_empty() {
        format!("{facts} {metadata}")
    } else {
        format!(
            "{facts}{spawn_block}
{metadata}"
        )
    };
    SessionEvent::PermissionRequest {
        tool_call_id: creation_permission_id(),
        title: format!("Create an agent: {} ({})", request.title, profile.name),
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

/// The card's tools sentence for one promised state (S2). The unavailable
/// sentence is the plan's words; the hosted sentence keeps the precedence
/// rule's required phrase.
pub(crate) fn card_tools_sentence(state: ToolsState) -> &'static str {
    match state {
        ToolsState::Unavailable => {
            " The child will start without Devboule tools: it cannot create, message or list agents."
        }
        ToolsState::Hosted => " The child will host Devboule tools and will be verified at start.",
        ToolsState::Unverified => " The child's tools are unverified and will be verified at start.",
    }
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
