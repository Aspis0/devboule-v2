//! Lifecycle: the loop keeps serving while a git command is held (the
//! property the offload exists for), a moved arm answers with the id and
//! shape it always answered with, a peer without the journal capability is
//! refused before anything is resolved, and a worker answering a closed
//! connection is inert. The gate lives in the git runner, so on a tree that
//! runs an arm inline the held command blocks the loop itself and the Ping
//! below times out — the red is the blocked loop, not a missing seam.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use devboule_protocol::ClientHello;

use super::test_support::*;
use super::*;

/// While a git status is held in the runner, the loop keeps serving: another
/// session's transcript frame and a cheap RPC both reach the client before
/// the git reply does.
#[cfg(windows)]
#[test]
fn queued_frames_flow_while_a_git_status_waits_off_dispatch() {
    let repo = TestRepo::new("git-off-loop-stall");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-stall-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);

    use crate::transport::{connect_pipe, Listener, NamedPipeListener};
    let owner = OwnerId::new(
        crate::security::current_user_sid().expect("current user"),
        format!("process-{}", std::process::id()),
    )
    .expect("owner");
    let other =
        crate::session::insert_test_live_agent(&state.sessions, "s.git.other", owner.clone());
    let paths = RuntimePaths::from_dir(path.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let mut listener = NamedPipeListener::bind(&paths, stop).expect("bind test pipe");
    let server_state = Arc::clone(&state);
    let server = std::thread::spawn(move || {
        let file = listener.accept().expect("accept test pipe");
        handle_client(Framed::new(file), server_state, None, QuitIntent::default())
    });
    let client = Framed::new(connect_pipe(&paths.pipe_name).expect("connect test pipe"));
    client
        .send(&ClientMessage::Hello(ClientHello::m3a(
            owner,
            "server-test",
        )))
        .expect("send hello");
    assert!(matches!(
        client
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("hello reply"),
        DaemonMessage::Hello(_)
    ));
    client
        .send(&ClientMessage::SessionAttach {
            id: 40,
            session_id: "s.git.other".to_string(),
            subscription_id: 1,
            from_cursor: None,
        })
        .expect("attach the other session");
    assert!(matches!(
        client
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("attach reply"),
        DaemonMessage::SessionAttached { .. }
    ));
    client
        .send(&ClientMessage::WorkspaceGitStatus {
            id: 50,
            workspace_id,
        })
        .expect("send git status");
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the status's git command was held");
    assert!(other.publish_agent_error("transcript frame".to_string()));
    client
        .send(&ClientMessage::Ping { id: 51 })
        .expect("send ping");
    let mut transcript_arrived = false;
    let mut pong_arrived = false;
    for _ in 0..32 {
        let frame = client
            .recv_timeout::<DaemonMessage>(Duration::from_secs(1))
            .expect("a frame while the git command is held — the loop is blocked");
        match frame {
            DaemonMessage::Event(envelope) | DaemonMessage::SubscriptionEvent { envelope, .. } => {
                if matches!(envelope.event, SessionEvent::AgentError { .. }) {
                    transcript_arrived = true;
                }
            }
            DaemonMessage::Pong { .. } => pong_arrived = true,
            _ => {}
        }
        if transcript_arrived && pong_arrived {
            break;
        }
    }
    assert!(
        transcript_arrived,
        "another session's transcript did not flow during the git command"
    );
    assert!(
        pong_arrived,
        "a cheap RPC did not answer during the git command"
    );
    release_tx.send(()).expect("release the git command");
    assert!(matches!(
        client
            .recv_timeout::<DaemonMessage>(Duration::from_secs(5))
            .expect("git status reply"),
        DaemonMessage::WorkspaceGit { id: 50, .. }
    ));
    drop(client);
    server
        .join()
        .expect("server thread")
        .expect("handle client");
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The positive control: through the worker road, a moved arm answers with
/// the id and the wire shape it has always answered with. It asserts the
/// road, so a regression to inline dispatch fails it.
#[test]
fn a_moved_git_arm_answers_with_the_same_id_and_shape() {
    let repo = TestRepo::new("git-off-loop-shape");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-shape-state", &repo.root);
    let owner = test_owner();
    let conn = ConnHandle::new(94);
    let mut backlog = std::collections::VecDeque::new();

    assert!(
        dispatch(
            &state,
            &owner,
            ClientMessage::WorkspaceGitStatus {
                id: 7,
                workspace_id: workspace_id.clone(),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .is_none(),
        "the git arms answer through the worker road, not inline"
    );
    match wait_for_worker_reply(&conn, &mut backlog) {
        DaemonMessage::WorkspaceGit { id, status } => {
            assert_eq!(id, 7);
            assert!(status.is_git, "the registered root is a repository");
            assert!(!status.dirty, "the base commit leaves the tree clean");
            assert!(status.error.is_none());
        }
        other => panic!("wrong reply shape: {other:?}"),
    }

    assert!(
        dispatch(
            &state,
            &owner,
            ClientMessage::WorkspaceGitStage {
                id: 8,
                workspace_id: "w.absent".to_string(),
                paths: vec!["x.txt".to_string()],
                idempotency_key: None,
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .is_none(),
        "the git arms answer through the worker road, not inline"
    );
    match wait_for_worker_reply(&conn, &mut backlog) {
        DaemonMessage::WorkspaceGitWrite { id, error } => {
            assert_eq!(id, 8);
            assert!(error.is_some(), "an unknown workspace must refuse");
        }
        other => panic!("wrong reply shape: {other:?}"),
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A peer without the journal capability is refused on the loop, before the
/// request resolves a workspace — the decision the inline road made, kept
/// ahead of the interception. An inline refusal returns the reply; a worker
/// road would return `None` and enqueue, so the assertion covers both.
#[test]
fn a_peer_without_the_journal_capability_is_refused_before_any_resolution() {
    let marker = crate::test_dirs::test_temp_dir("git-off-loop-caps-marker");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-caps", &marker);
    let owner = test_owner();
    let conn = ConnHandle::new(99);
    let refused = dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStatus {
            id: 9,
            workspace_id,
        },
        &conn,
        true,
        false,
        true,
        true,
    )
    .expect("refused inline, not offloaded");
    match refused {
        DaemonMessage::Error(error) => {
            assert_eq!(error.id, Some(9));
            assert!(
                error.message.contains("capability"),
                "expected the capability refusal, got: {}",
                error.message
            );
        }
        other => panic!("wrong refusal shape: {other:?}"),
    }
    assert!(conn.outbound.pull_replies().is_empty());
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_dir_all(&marker);
}

/// A worker answering a connection that already closed is inert: the reply
/// lands in a deque nobody drains and is dropped with the connection — no
/// panic, no write to a dead handle.
#[test]
fn a_worker_replying_after_the_connection_closed_does_not_panic() {
    let repo = TestRepo::new("git-off-loop-closed");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-closed-state", &repo.root);
    let owner = test_owner();
    let conn = ConnHandle::new(101);
    conn.outbound.close();
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStatus {
            id: 11,
            workspace_id,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    let mut backlog = std::collections::VecDeque::new();
    match wait_for_worker_reply(&conn, &mut backlog) {
        DaemonMessage::WorkspaceGit { id, .. } => assert_eq!(id, 11),
        other => panic!("wrong reply shape: {other:?}"),
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
