//! Executing one creation end to end.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::McpCaller;
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::card::{creation_card, self_answer_note};
use super::labels::stamped_labels;
use super::profile::resolve_profile;
use super::request::{creation_fingerprint, AgentCreateRequest};
use super::result::created_result;

/// The `devboule_create_agent` tool (`create-from-profile`).
///
/// The caller is the session whose Bearer authenticated the connection: the
/// `registration` is the only identity this function uses, and there is no
/// `from_session` parameter to lie about.
///
/// The order: resolve the profile the caller named from the
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
    // A creation that needs the person's card does not hold the creator's
    // turn: the checks run here and answer as they always did, and once a card
    // is owed the card waits for the person in the background, with the outcome
    // coming back to the creator as a message that wakes it.
    run_creation(state, caller, registration, id, request, true)
}

/// The creations whose card is waiting for the person, by creator and frame id.
/// A retry of a waiting frame is answered as waiting, and never raises a
/// second card.
static AWAITING_CARD: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn await_card_in_background(
    state: &Arc<ServerState>,
    caller: &McpCaller,
    registration: &RegisteredSession,
    id: &Value,
    request: AgentCreateRequest,
) -> Value {
    let waiting = crate::server::creation_retry_key(&registration.session_id, id);
    let awaiting = |text: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{"type": "text", "text": text}],
                "structuredContent": {"state": "awaiting_approval"},
                "isError": false,
            },
        })
    };
    if let Some(key) = waiting.as_deref() {
        let mut guard = AWAITING_CARD
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let set = guard.get_or_insert_with(Default::default);
        if !set.insert(key.to_string()) {
            return awaiting("This creation is already waiting for the person's approval.");
        }
    }
    let worker_state = Arc::clone(state);
    let worker_caller = caller.clone();
    let worker_registration = registration.clone();
    let worker_id = id.clone();
    let worker_key = waiting.clone();
    let spawned = std::thread::Builder::new()
        .name("agent-creation-card".to_string())
        .spawn(move || {
            let answer = create_agent_now(
                &worker_state,
                &worker_caller,
                &worker_registration,
                &worker_id,
                request,
            );
            if let Some(key) = worker_key.as_deref() {
                let mut guard = AWAITING_CARD
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(set) = guard.as_mut() {
                    set.remove(key);
                }
            }
            let text = creation_outcome_text(&answer);
            if let Err(error) = worker_state.sessions.wake_creator(
                &worker_registration.session_id,
                &worker_registration.owner,
                &text,
            ) {
                eprintln!("the creation outcome was not delivered: {}", error.message);
            }
        });
    match spawned {
        Ok(_) => awaiting(
            "Asked the person to approve creating this agent. Its session and start reach you as a message that wakes you: end your turn now, and do not poll for it.",
        ),
        Err(error) => {
            if let Some(key) = waiting.as_deref() {
                let mut guard = AWAITING_CARD.lock().unwrap_or_else(|error| error.into_inner());
                if let Some(set) = guard.as_mut() {
                    set.remove(key);
                }
            }
            tool_error(id, &format!("the creation could not wait for approval: {error}"))
        }
    }
}

/// The message a creator gets when a card-waiting creation has run its course:
/// the new session and its name, or the reason it did not start.
fn creation_outcome_text(answer: &Value) -> String {
    if let Some(session_id) = answer
        .pointer("/result/structuredContent/sessionId")
        .and_then(Value::as_str)
    {
        let name = answer
            .pointer("/result/structuredContent/displayName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return format!(
            "The person approved the agent creation: '{name}' started as session {session_id}. Its finish reaches you as a message that wakes you."
        );
    }
    let reason = answer
        .pointer("/result/content/0/text")
        .or_else(|| answer.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or("the creation did not start");
    format!("The agent creation did not start: {reason}")
}

/// The creation with its card awaited in the call: the road the background
/// worker runs once the person has been asked, and the one a creator that owes
/// no card takes.
pub(in crate::mcp_broker) fn create_agent_now(
    state: &Arc<ServerState>,
    caller: &McpCaller,
    registration: &RegisteredSession,
    id: &Value,
    request: AgentCreateRequest,
) -> Value {
    run_creation(state, caller, registration, id, request, false)
}

/// One creation. `background` answers a card-owed creation at once: every
/// check that spends nothing has run by then, so a refusal still comes back
/// here, and the card is raised by the worker that this call starts.
fn run_creation(
    state: &Arc<ServerState>,
    caller: &McpCaller,
    registration: &RegisteredSession,
    id: &Value,
    request: AgentCreateRequest,
    background: bool,
) -> Value {
    let creator_id = registration.session_id.clone();
    // The retry identity, and the payload it must match.
    //
    // An MCP `tools/call` has no idempotency parameter — the schema is closed
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
            return created_result(
                id,
                &existing,
                state.mcp.is_registered(&existing.id),
                request.notify,
            );
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
    // Where the child runs, before anything is spent on it: a
    // workspace is either the caller's own or the call is refused, and the
    // working directory must stay inside it. The card then states the directory
    // the child will really get.
    let workspace_id = match request.workspace_id.as_deref() {
        Some(requested) if Some(requested) != creator.workspace_id.as_deref() => {
            return tool_error(id, "workspace must be the caller's");
        }
        _ => creator.workspace_id.clone(),
    };
    // A child starts in its creator's workspace. A creator that has none (an
    // older session) cannot give one, and the child would start in the
    // daemon's own directory.
    if workspace_id.is_none() {
        return tool_error(id, "A child needs a workspace. The caller has none.");
    }
    let cwd = match state
        .sessions
        .resolve_child_cwd(workspace_id.as_deref(), request.cwd.as_deref())
    {
        Ok(cwd) => cwd,
        Err(error) => return tool_error(id, &error.message),
    };
    // The depth comes from the registration, never from the request:
    // a session at depth 2 may not create, whatever it says.
    let depth = registration.depth.saturating_add(1);
    if depth > crate::session::MAX_AGENT_DEPTH {
        return tool_error(id, "depth limit; do not retry");
    }
    // The device that owns the creator must still be allowed to create
    // sessions: a child of a peer's session is a session on that peer's device,
    // so the gate the peer already passed for its own `SessionCreate` is the
    // gate its child passes here. A revoked or
    // capability-stripped device fails closed.
    if !creator.may_create_sessions(state) {
        return tool_error(id, "not allowed for this peer");
    }
    // A provider this daemon cannot launch is refused before a session id, a
    // card or a slot is spent on it. The provider is the profile's:
    // a creation cannot name one, so this is the only provider that can be
    // missing, and the sentence says what it is about.
    if !provider_is_launchable(&profile.provider) {
        return tool_error(id, "provider not installed");
    }
    // A profile combination never fails the creation (owner bug #16). The
    // tick over a mode that answers its own permission prompts — the
    // daemon's broker or the provider itself, like Claude's `auto`
    // classifier — stands; over a mode that asks the human it resolves to
    // off, so the child asks and the card says so. Resolving toward asking
    // is the safe direction: escalating the mode would grant a privilege
    // nobody approved. The client re-checks the resolved delivery at spawn.
    let ticked = crate::profile_delivery::feature_is_true(
        &profile.features,
        crate::provider_catalog::AUTO_ACCEPT_FEATURE,
    );
    let auto_accept =
        crate::profile_delivery::resolve_auto_accept(&profile.provider, &profile.mode, ticked);
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
    // The creator's mode, read on every call and before the reservation: a
    // plan or read-only mode of the creator's own family refuses even with
    // an open gate, and an automatic mode skips the card without opening one.
    let creator_runtime = state
        .sessions
        .live_runtime(&creator_id, &registration.owner);
    let creator_gate = creator_runtime.as_ref().map(|runtime| runtime.mode_gate());
    if let Some(crate::provider_catalog::ModeGate::Refuse(sentence)) = &creator_gate {
        return tool_error(id, sentence);
    }
    let needs_card = !matches!(creator_gate, Some(crate::provider_catalog::ModeGate::Auto));
    if background && needs_card {
        drop(hold);
        return await_card_in_background(state, caller, registration, id, request);
    }
    let ticket = match state
        .sessions
        .reserve_agent_creation(&creator_id, depth, needs_card)
    {
        Ok(ticket) => ticket,
        Err(error) => return tool_error(id, &error.message),
    };
    if ticket.card_owed() {
        // The card is raised on the creator's own session, through the same
        // broker entry every other card uses: the same decision frame answers
        // it, the same per-device budget bounds a peer's, and a refusal leaves the
        // gate shut.
        if creator_runtime
            .as_ref()
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
        // model, thinking option and the *resolved* auto-accept answer, as
        // the one typed delivery the spawn path applies. A value that cannot
        // be delivered refuses the creation; nothing here is substituted —
        // except the tick over an asking mode, which resolved off above and
        // is carried resolved here, so the spawn-time check agrees with the
        // card.
        delivery: crate::profile_delivery::ProfileDelivery::for_child_resolved(
            &profile.mode,
            &profile.model,
            profile.model_provider.as_deref(),
            profile.thinking_option_id.as_deref(),
            &profile.features,
            auto_accept,
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
            // told a creation is in progress.
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            created_result(
                id,
                &session,
                state.mcp.is_registered(&session.id),
                request.notify,
            )
        }
        // Every refusal above and this failure release the reservation
        // through the ticket's own `Drop`: one release path,
        // taken exactly once, whatever happened.
        Err(error) => tool_error(id, &error.message),
    }
}

/// Can this daemon launch `provider`? Two sources, because there are two ways
/// a provider can exist: a catalogue row must be found on PATH, while a
/// user-declared row carries its own argv and is launchable without being on
/// PATH at all (`acp_client::resolve_named` reads the live registry before the
/// PATH/CDN walk). Asking PATH alone refuses every user provider on this road
/// while the wire create road spawns it — one provider, two answers,
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
