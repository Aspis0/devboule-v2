//! Which local ports an agent may open in agent browsing (review fix pass).
//!
//! An agent may open this machine only at a port its own processes listen on,
//! or one a terminal of its workspace listens on, or one the person allowed for
//! its session through a card in the chat. The daemon decides the list and
//! sends it with every browser command; the host enforces it. A port the daemon
//! itself listens on is never allowed.

use std::collections::BTreeSet;

use serde_json::Value;

use devboule_protocol::{OwnerId, PermissionOption, SessionEvent, SessionOrigin};

use super::first_use::{ask_card_in_any_mode, card_id};
use crate::process_index::SessionPorts;
use crate::server::ServerState;

/// The ports one session's agent owns without asking: every port its own
/// processes listen on, and every port a terminal of its workspace listens on.
/// Other agents' ports are not among them, even in the same workspace.
pub(in crate::mcp_broker) fn owned_ports(
    sessions: &[SessionPorts],
    session_id: &str,
    workspace_id: Option<&str>,
) -> BTreeSet<u16> {
    sessions
        .iter()
        .filter(|session| {
            session.id == session_id
                || (session.terminal
                    && workspace_id.is_some()
                    && session.workspace_id.as_deref() == workspace_id)
        })
        .flat_map(|session| session.ports.iter().copied())
        .collect()
}

/// The ports the daemon itself listens on, which no agent may open.
pub(in crate::mcp_broker) fn daemon_ports(state: &ServerState) -> Vec<u16> {
    crate::egress_policy::url_port(&state.mcp.url)
        .into_iter()
        .collect()
}

/// The ports the agent may open right now: its owned ports and the ports the
/// person allowed for its session, without the daemon's own. A refresh the
/// platform refuses leaves only the grants, which fails toward asking.
pub(in crate::mcp_broker) fn allowed_ports(
    state: &ServerState,
    session_id: &str,
    workspace_id: Option<&str>,
) -> Vec<u16> {
    let owned = if state
        .process_index
        .refresh(state.sessions.live_process_roots())
        .is_ok()
    {
        owned_ports(
            &state.process_index.listening_by_session(),
            session_id,
            workspace_id,
        )
    } else {
        BTreeSet::new()
    };
    let granted = state.local_port_grants(session_id);
    let own = daemon_ports(state);
    owned
        .into_iter()
        .chain(granted)
        .filter(|port| !own.contains(port))
        .collect::<BTreeSet<u16>>()
        .into_iter()
        .collect()
}

/// The ports for one browser call. A navigation to a local port the agent does
/// not own asks the person in the chat, once; an allowed answer is remembered
/// for the session. Any other answer, and the daemon's own ports, refuse the
/// call with a plain sentence.
pub(in crate::mcp_broker) fn ports_for_call(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    workspace_id: Option<&str>,
    command: &str,
    args: &Value,
) -> Result<Vec<u16>, String> {
    let mut ports = allowed_ports(state, session_id, workspace_id);
    if !matches!(command, "new_tab" | "navigate") {
        return Ok(ports);
    }
    let Some(port) = args
        .get("url")
        .and_then(Value::as_str)
        .and_then(crate::egress_policy::local_url_port)
    else {
        return Ok(ports);
    };
    if ports.contains(&port) {
        return Ok(ports);
    }
    if daemon_ports(state).contains(&port) {
        return Err(format!(
            "localhost:{port} is Devboule's own port, and no agent may open it"
        ));
    }
    let chosen = ask_card_in_any_mode(state, session_id, owner, local_port_card(session_id, port))?;
    if chosen != allow_option(port) {
        return Err(format!("localhost:{port} was not allowed for this session"));
    }
    state.grant_local_port(session_id, port);
    ports.push(port);
    ports.sort_unstable();
    Ok(ports)
}

fn allow_option(port: u16) -> String {
    format!("allow:{port}")
}

/// The chat card that asks the person for one port.
fn local_port_card(session_id: &str, port: u16) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: card_id(session_id, &format!("local_port@{port}")),
        title: format!("Allow localhost:{port}?"),
        description: Some(format!(
            "An agent of this session wants to open localhost:{port} in Devboule's browser. \
             Its own processes and the terminals of its workspace are already allowed; this \
             port is not. Allowing it lets this session open that port until the session \
             ends. Nothing else on this machine is opened."
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: allow_option(port),
                name: format!("Allow localhost:{port}"),
                kind: "allow_once".to_owned(),
            },
            PermissionOption {
                option_id: "deny".to_owned(),
                name: "Deny".to_owned(),
                kind: "reject_once".to_owned(),
            },
        ],
        is_chooser: Some(true),
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, workspace: Option<&str>, terminal: bool, ports: &[u16]) -> SessionPorts {
        SessionPorts {
            id: id.to_string(),
            workspace_id: workspace.map(str::to_string),
            terminal,
            ports: ports.to_vec(),
        }
    }

    #[test]
    fn a_local_url_names_its_port_and_another_host_names_none() {
        assert_eq!(
            crate::egress_policy::local_url_port("http://127.0.0.1:4173/"),
            Some(4173)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("https://localhost:5173/x"),
            Some(5173)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("http://localhost/"),
            Some(80)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("http://[::1]:6000/"),
            Some(6000)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("http://[::ffff:127.0.0.1]:6001/"),
            Some(6001)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("http://2130706433:6002/"),
            Some(6002)
        );
        assert_eq!(
            crate::egress_policy::local_url_port("https://example.com:4173/"),
            None
        );
        assert_eq!(
            crate::egress_policy::local_url_port("http://10.0.0.5:8080/"),
            None
        );
    }

    #[test]
    fn an_agent_owns_its_own_ports_and_its_workspace_terminals_only() {
        let sessions = [
            session("s.agent", Some("w1"), false, &[4000]),
            session("s.term", Some("w1"), true, &[5000]),
            session("s.other-term", Some("w2"), true, &[6000]),
            session("s.sibling", Some("w1"), false, &[7000]),
        ];
        let owned = owned_ports(&sessions, "s.agent", Some("w1"));
        assert_eq!(owned.into_iter().collect::<Vec<_>>(), vec![4000, 5000]);
    }

    #[test]
    fn a_session_outside_any_workspace_owns_only_its_own_ports() {
        let sessions = [
            session("s.agent", None, false, &[4000]),
            session("s.term", None, true, &[5000]),
        ];
        let owned = owned_ports(&sessions, "s.agent", None);
        assert_eq!(owned.into_iter().collect::<Vec<_>>(), vec![4000]);
    }

    /// One call on a thread, with the person's answer given to the card it
    /// raises: the answer's option id is the only thing the person chose.
    fn call_answering(
        state: &std::sync::Arc<ServerState>,
        session: &str,
        owner: &OwnerId,
        option: &str,
    ) -> Result<Vec<u16>, String> {
        let thread_state = std::sync::Arc::clone(state);
        let thread_owner = owner.clone();
        let thread_session = session.to_string();
        let handle = std::thread::spawn(move || {
            ports_for_call(
                &thread_state,
                &thread_session,
                &thread_owner,
                Some("w1"),
                "navigate",
                &serde_json::json!({"url": "http://localhost:4173/"}),
            )
        });
        let start = std::time::Instant::now();
        loop {
            let pending = state
                .sessions
                .live_runtime(session, owner)
                .expect("live session")
                .permission_broker()
                .expect("test broker")
                .test_pending_ids();
            if let Some(card) = pending.first() {
                let outcome = if option == "deny" {
                    devboule_protocol::PermissionOutcome::Deny
                } else {
                    devboule_protocol::PermissionOutcome::AllowOnce
                };
                state
                    .sessions
                    .live_runtime(session, owner)
                    .expect("live session")
                    .permission_broker()
                    .expect("test broker")
                    .test_answer(card, outcome, option)
                    .expect("answer the card");
                return handle.join().expect("the call");
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(20),
                "the call neither asked nor answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A port the agent does not own asks the person once: a deny refuses the
    /// call, and an allow is remembered for the session, so the next call asks
    /// nothing and the port is in the agent's list.
    #[test]
    fn a_port_the_agent_does_not_own_asks_the_person_and_an_allow_is_remembered() {
        let state = std::sync::Arc::new(ServerState::new("browser-ports".to_string()));
        let owner = OwnerId::new("S-1-5-21-browser-ports", "browser-ports-client").expect("owner");
        let (workspace, _root) =
            crate::mcp_broker::terminal_write_harness::project_workspace(&state, "browser-ports");
        let session = "s.browser-ports";
        crate::session::insert_test_live_session_in_workspace(
            &state.sessions,
            session,
            owner.clone(),
            devboule_protocol::SessionKind::Pi,
            &workspace,
        );
        let refused = call_answering(&state, session, &owner, "deny");
        assert!(
            refused
                .as_ref()
                .is_err_and(|sentence| sentence.contains("localhost:4173")),
            "{refused:?}"
        );
        assert!(!allowed_ports(&state, session, Some("w1")).contains(&4173));
        let allowed = call_answering(&state, session, &owner, "allow:4173").expect("allowed");
        assert_eq!(allowed, vec![4173]);
        assert!(allowed_ports(&state, session, Some("w1")).contains(&4173));
        assert_eq!(
            ports_for_call(
                &state,
                session,
                &owner,
                Some("w1"),
                "navigate",
                &serde_json::json!({"url": "http://localhost:4173/"}),
            )
            .expect("remembered, so no card"),
            vec![4173]
        );
    }

    /// The daemon's own port is refused with its own sentence and no card.
    #[test]
    fn the_daemon_own_port_is_never_offered_to_the_person() {
        let state = std::sync::Arc::new(ServerState::new("browser-ports-own".to_string()));
        let owner =
            OwnerId::new("S-1-5-21-browser-ports", "browser-ports-own-client").expect("owner");
        let server = state.mcp.start(&state).expect("MCP server");
        let own = daemon_ports(&state);
        assert_eq!(own.len(), 1, "the broker has a port");
        let refused = ports_for_call(
            &state,
            "s.own",
            &owner,
            None,
            "navigate",
            &serde_json::json!({"url": format!("http://127.0.0.1:{}/", own[0])}),
        );
        assert!(
            refused
                .as_ref()
                .is_err_and(|sentence| sentence.contains("Devboule's own port")),
            "{refused:?}"
        );
        drop(server);
    }
}
