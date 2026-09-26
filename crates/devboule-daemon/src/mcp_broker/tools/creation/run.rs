//! Executing one creation end to end.

use std::sync::Arc;

use serde_json::Value;

use crate::mcp_broker::caller::McpCaller;
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::card::{creation_card, self_answer_note};
use super::labels::stamped_labels;
use super::profile::resolve_profile;
use super::request::{creation_fingerprint, AgentCreateRequest};
use super::result::created_result;

/// The `devboule_create_agent` tool (`S5` §2 and §3; `create-from-profile`).
///
/// The caller is the session whose Bearer authenticated the connection: the
/// `registration` is the only identity this function uses, and there is no
/// `from_session` parameter to lie about.
///
/// The order is the checklist's: resolve the profile the caller named from the
/// profiles the human **ticked, read now** (the provider, the model, the mode,
/// the features and the tool overlay come from there and never from the
/// caller), reserve the budget, raise the creation card once per creator
/// session, create through the `SessionCreate` path with the creator's own
/// origin and owner, and answer
/// `{sessionId, taskId, contextId, displayName, state: "submitted"}`.
pub(in crate::mcp_broker) fn create_agent(
    state: &Arc<ServerState>,
    _broker: &McpBroker,
    caller: &McpCaller,
    registration: &RegisteredSession,
    id: &Value,
    request: AgentCreateRequest,
) -> Value {
    let creator_id = registration.session_id.clone();
    // The retry identity, and the payload it must match (`S5` block 7, audit
    // S5-03 and S5-08).
    //
    // An MCP `tools/call` has no idempotency parameter — §2's schema is closed
    // and defines none — so the only identity a *retry* has is the frame's own
    // id, which a client reuses when it re-sends a request whose answer it lost.
    // The fingerprint is everything the answer depends on, so a key reused with
    // a different payload is a conflict, not a retry.
    //
    // The key is held *before* the store is read: a second call that arrives
    // while this one is still raising a card is in flight, not a retry, and is
    // refused without spending a slot. Every refusal below releases it through
    // the hold's own scope.
    //
    // The store is consulted **before the profile is resolved**: a retry
    // arrives after the first attempt's answer was lost, and in that window the
    // human may have renamed or un-ticked the profile the first attempt ran
    // under — the child it created is alive either way. Refusing the retry at
    // the profile check would tell the creator its creation failed, and a
    // second call would spend a second slot on a child it already has. A
    // *new* call — a different frame id — has no remembered answer and still
    // meets the profile check below, with the store exactly as it stands now.
    let retry_key = crate::server::creation_retry_key(&creator_id, id);
    let mut hold = match retry_key.as_deref() {
        Some(key) => match state.sessions.hold_creation_key(key) {
            Ok(hold) => Some(hold),
            Err(error) => return tool_error(id, &error.message),
        },
        None => None,
    };
    let notify_field = if request.notify { "notify" } else { "quiet" };
    let labels_field = request.labels_fingerprint();
    let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
        &creator_id,
        &request,
        notify_field,
        &labels_field,
    ));
    if let Some(key) = retry_key.as_deref() {
        if let Some(existing) = crate::server::idempotent_creation_session(
            state,
            &registration.owner,
            key,
            &fingerprint,
        ) {
            // A retry answers the first call's session and creates nothing:
            // no second card, no second slot, no second child.
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            return created_result(id, &existing, state.mcp.is_registered(&existing.id));
        }
    }
    let profile = match resolve_profile(&state.agent_profiles, &request.profile) {
        Ok(profile) => profile,
        Err(message) => return tool_error(id, &message),
    };
    let creator = match state
        .sessions
        .agent_creator(&creator_id, &registration.owner)
    {
        Ok(creator) => creator,
        Err(error) => return tool_error(id, &error.message),
    };
    // Where the child runs, before anything is spent on it (audit S5-05): a
    // workspace is either the caller's own or the call is refused, and the
    // working directory must stay inside it. The card then states the directory
    // the child will really get.
    let workspace_id = match request.workspace_id.as_deref() {
        Some(requested) if Some(requested) != creator.workspace_id.as_deref() => {
            return tool_error(id, "workspace must be the caller's");
        }
        _ => creator.workspace_id.clone(),
    };
    let cwd = match state
        .sessions
        .resolve_child_cwd(workspace_id.as_deref(), request.cwd.as_deref())
    {
        Ok(cwd) => cwd,
        Err(error) => return tool_error(id, &error.message),
    };
    // The depth comes from the registration, never from the request
    // (`S5` checklist): a session at depth 2 may not create, whatever it says.
    let depth = registration.depth.saturating_add(1);
    if depth > crate::session::MAX_AGENT_DEPTH {
        return tool_error(id, "depth limit; do not retry");
    }
    // The device that owns the creator must still be allowed to create
    // sessions: a child of a peer's session is a session on that peer's device,
    // so the gate the peer already passed for its own `SessionCreate` is the
    // gate its child passes here (`S5` §3, "closed set"). A revoked or
    // capability-stripped device fails closed.
    if !creator.may_create_sessions(state) {
        return tool_error(id, "not allowed for this peer");
    }
    // A provider this daemon cannot launch is refused before a session id, a
    // card or a slot is spent on it (`S5` §2). The provider is the profile's:
    // a creation cannot name one, so this is the only provider that can be
    // missing, and the sentence says what it is about.
    if !provider_is_launchable(&profile.provider) {
        return tool_error(id, "provider not installed");
    }
    // The contradiction the profile alone decides is decided **here**, before
    // the reservation and before the card: a tick over a mode that asks the
    // human is refused without spending the human's consent on a creation
    // the daemon had already decided to refuse, and without the card reading
    // "auto accept: Yes (mode ask)" for exactly that configuration (the R2a
    // audit's F7).
    //
    // The refusal is bounded by **authorship** (the re-audit's P1): this gate
    // concludes only where the daemon owns the rule. Claude and Pi's tick
    // rule is the daemon's own — start in a mode the broker answers — and
    // the profile's mode is the delivered mode for both. Codex's knob
    // (`full-access`) and every ACP agent's modes are the family's own
    // vocabulary, so the daemon refuses nothing there: the client re-judges
    // at spawn time, where the delivered mode is the fact. The old shape
    // judged every provider from the shared table alone and refused a Codex
    // `full-access` profile its own client accepts.
    if matches!(
        crate::provider_catalog::judge_auto_accept_tick(
            &profile.provider,
            &profile.mode,
            &profile.features
        ),
        crate::provider_catalog::AutoAcceptTick::Contradicts
    ) {
        return tool_error(
            id,
            &format!(
                "the profile asks the agent to approve its own permission prompts and also to start in mode '{}', which asks the human; the two contradict, so the creation is refused",
                profile.mode
            ),
        );
    }
    // The child's labels, stamped here where all four facts are known. Stamped
    // into the same map the caller wrote, so a human reads one list; refused if
    // the caller tried to write one of them (`parse_labels`), so the daemon's
    // facts are the daemon's.
    let labels = stamped_labels(
        &request.labels,
        &creator_id,
        &profile,
        depth,
        &creator.origin,
    );
    // Read before `creator` moves into the creation below.
    let context_id = creator.context_id.clone();
    let ticket = match state.sessions.reserve_agent_creation(&creator_id, depth) {
        Ok(ticket) => ticket,
        Err(error) => return tool_error(id, &error.message),
    };
    if ticket.card_owed() {
        // The card is raised on the creator's own session, through the same
        // broker entry every other card uses: the same decision frame answers
        // it, the same per-device budget bounds a peer's, and a refusal leaves
        // the gate shut (`S5` decision 4).
        if state
            .sessions
            .live_runtime(&creator_id, &registration.owner)
            .and_then(|runtime| runtime.permission_broker())
            .is_none()
        {
            return tool_error(id, "permission refused");
        }
        let self_answer_note = self_answer_note(state, caller);
        let card = creation_card(
            &creator_id,
            creator.name(),
            &request,
            &profile,
            &labels,
            &ticket,
            self_answer_note.as_deref(),
        );
        let authorized = state
            .sessions
            .ask_creation_card(&creator_id, &registration.owner, card);
        if !authorized {
            return tool_error(id, "permission refused");
        }
        state.sessions.accept_agent_creation(&creator_id);
    }
    let creator_runtime = state
        .sessions
        .live_runtime(&creator_id, &registration.owner);
    let creation = crate::session::AgentCreation {
        creator_session_id: creator_id.clone(),
        creator_runtime,
        display_name: request.title.clone(),
        creator,
        provider: profile.provider.clone(),
        // The session records the profile's **id** and its **name** is what the
        // creator's transcript shows: a rename later changes nothing about a
        // child that is already running (`Session.profile_id`), while the
        // sentence a human reads names the profile the way they ticked it.
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        // What the card showed is what the child gets: the spawn prompt
        // travels with the creation, resolved once, and the send composes it
        // into the child's first prompt (standing instructions, then this,
        // then the preamble, then the prompt).
        spawn_prompt: profile.spawn_prompt.clone(),
        // What the card named is what the child gets: the profile's own mode,
        // model, thinking option and auto-accept tick, as the one typed
        // delivery the spawn path applies. A value that cannot be delivered
        // refuses the creation; nothing here is substituted.
        delivery: crate::profile_delivery::ProfileDelivery::for_child(
            &profile.mode,
            &profile.model,
            profile.thinking_option_id.as_deref(),
            &profile.features,
        ),
        overlay: profile.overlay.clone(),
        labels,
        context_id: Some(context_id),
        depth,
        cwd,
        initial_prompt: request.initial_prompt,
        notify: request.notify,
        workspace_id,
    };
    match state
        .sessions
        .create_session_for_agent(state, creation, ticket)
    {
        Ok(session) => {
            if let Some(key) = retry_key.as_deref() {
                crate::server::remember_creation_session(
                    state,
                    &registration.owner,
                    key,
                    &fingerprint,
                    &session,
                );
            }
            // The result is remembered, so the key stops being in flight: a
            // client that re-sends now reads the answer above instead of being
            // told a creation is in progress (`S5-03`).
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            created_result(id, &session, state.mcp.is_registered(&session.id))
        }
        // Every refusal above and this failure release the reservation
        // through the ticket's own `Drop` (audit S5B-02): one release path,
        // taken exactly once, whatever happened.
        Err(error) => tool_error(id, &error.message),
    }
}

/// A refusal an agent reads: the sentence, and never a session id.
/// Can this daemon launch `provider`? Two sources, because there are two ways
/// a provider can exist: a catalogue row must be found on PATH, while a
/// user-declared row carries its own argv and is launchable without being on
/// PATH at all (`acp_client::resolve_named` reads the live registry before the
/// PATH/CDN walk). Asking PATH alone refused every user provider on this road
/// while the wire create road spawned it — one provider, two answers,
/// depending on which door the caller came through.
pub(in crate::mcp_broker) fn provider_is_launchable(provider: &str) -> bool {
    crate::session::catalog_registry()
        .user_row_for(provider)
        .is_some()
        || crate::provider_catalog::find_available(provider).is_some()
}

pub(in crate::mcp_broker) fn create_agent_tool(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    caller: McpCaller,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    match AgentCreateRequest::parse(&arguments) {
        Ok(request) => Ok(Some(create_agent(
            state,
            broker,
            &caller,
            registration,
            &id,
            request,
        ))),
        Err(message) => Ok(Some(rpc_error(id, -32602, &message))),
    }
}
