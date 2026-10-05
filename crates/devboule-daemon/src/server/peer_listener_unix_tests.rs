//! The peer listener's Unix refusal: a machine whose LocalAPI is not a
//! filesystem socket must be told exactly that, not left silently disabled.

use crate::paths::RuntimePaths;
use crate::ServerState;

/// The refusal the real transport produces where there is no tailscaled
/// socket — which is what a macOS CI runner (and any Mac running only the
/// Tailscale app) presents. The reason must name the socket it looked for
/// and, on macOS, say where LocalAPI really lives instead of claiming
/// Tailscale is not installed.
#[test]
fn remote_listener_refuses_without_a_localapi_socket_and_says_why() {
    let dir = crate::test_dirs::test_temp_dir("devboule-remote-no-localapi");
    let state = ServerState::with_paths(
        "test-instance".to_string(),
        RuntimePaths::from_dir(dir.clone()),
    )
    .expect("state");

    // No stub transport: the real Tailnet probes this machine's LocalAPI,
    // and the runner exposes no socket for it.
    assert!(
        !state.ensure_remote_listener(),
        "no LocalAPI, no listener: {:?}",
        state.remote_state()
    );
    assert_eq!(state.listener_starts(), 0, "nothing was started");

    let remote = state.remote_state();
    assert_eq!(
        remote.state,
        devboule_protocol::RemoteStateKind::Disabled,
        "the refusal is a Disabled state, never a silent one"
    );
    let reason = remote.reason.expect("a refusal must say why");
    assert!(
        reason.contains(crate::tailscale_localapi::DEFAULT_UNIX_SOCKET),
        "the reason names the socket it looked for: {reason}"
    );
    assert!(reason.contains("does not exist"), "{reason}");
    #[cfg(target_os = "macos")]
    {
        assert!(
            reason.contains("extension"),
            "the macOS reason explains where LocalAPI really is: {reason}"
        );
        assert!(
            reason.contains(crate::tailscale_localapi::ENDPOINT_ENV),
            "the reason names the override that would fix it: {reason}"
        );
    }

    drop(state);
    let _ = std::fs::remove_dir_all(&dir);
}
