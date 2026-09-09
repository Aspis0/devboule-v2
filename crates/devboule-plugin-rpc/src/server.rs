use std::time::Duration;

use devboule_daemon::Framed;
use devboule_protocol::{
    negotiate, plugin_frame_limit_for_payload, ClientHello, ClientMessage, DaemonHello,
    DaemonMessage, ErrorCode, Negotiation, WireError,
};

use crate::error::PluginError;
use crate::pipe::{bind_and_accept, verify_pipe_client_pid};
use crate::spawn::HOST_PID_ENV;

const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// The plugin-backend end of a host conversation. One connection.
pub struct PluginBackend {
    framed: Framed,
    hello: ClientHello,
    negotiation: Negotiation,
    max_payload_bytes: usize,
}

impl PluginBackend {
    /// Bind the pipe the host named, accept one client, complete handshake.
    pub fn listen(pipe_name: &str) -> Result<Self, PluginError> {
        let raw_pid = std::env::var(HOST_PID_ENV).map_err(|_| {
            PluginError::Protocol(format!(
                "read {HOST_PID_ENV}: missing; refusing an unidentified host"
            ))
        })?;
        let host_pid = raw_pid
            .parse::<u32>()
            .map_err(|error| PluginError::Protocol(format!("parse {HOST_PID_ENV}: {error}")))?;
        Self::listen_for_host(pipe_name, host_pid)
    }

    /// Testable form of [`Self::listen`] for an in-process host/client pair.
    pub fn listen_for_host(pipe_name: &str, expected_host_pid: u32) -> Result<Self, PluginError> {
        let file = bind_and_accept(pipe_name, ACCEPT_TIMEOUT).map_err(PluginError::Io)?;
        verify_pipe_client_pid(&file, expected_host_pid).map_err(PluginError::from)?;
        let mut framed = Framed::new(file);
        let first: ClientMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
        let ClientMessage::Hello(client_hello) = first else {
            let error = WireError::new(ErrorCode::InvalidRequest, "first frame must be hello");
            let _ = framed.send(&DaemonMessage::Error(error.clone()));
            return Err(PluginError::Handshake(error));
        };
        let Some(payload_bytes) = client_hello.plugin_payload_bytes else {
            let error = WireError::new(
                ErrorCode::InvalidRequest,
                "plugin hello did not carry a payload budget",
            );
            let _ = framed.send(&DaemonMessage::Error(error.clone()));
            return Err(PluginError::Handshake(error));
        };
        let payload_bytes = match usize::try_from(payload_bytes) {
            Ok(payload_bytes) => payload_bytes,
            Err(_) => {
                let error = WireError::new(
                    ErrorCode::InvalidRequest,
                    "plugin payload budget does not fit this host",
                );
                let _ = framed.send(&DaemonMessage::Error(error.clone()));
                return Err(PluginError::Handshake(error));
            }
        };
        if payload_bytes == 0 || payload_bytes > devboule_protocol::PLUGIN_PAYLOAD_CEILING_BYTES {
            let error = WireError::new(
                ErrorCode::InvalidRequest,
                "plugin hello carried an invalid payload budget",
            );
            let _ = framed.send(&DaemonMessage::Error(error.clone()));
            return Err(PluginError::Handshake(error));
        }
        framed.set_max_frame_bytes(plugin_frame_limit_for_payload(payload_bytes));
        let daemon_hello = DaemonHello::plugin_backend(
            format!("plugin-{}", std::process::id()),
            std::process::id(),
        );
        match negotiate(&client_hello, &daemon_hello) {
            Ok(negotiation) => {
                framed.send(&DaemonMessage::Hello(daemon_hello))?;
                Ok(Self {
                    framed,
                    hello: client_hello,
                    negotiation,
                    max_payload_bytes: payload_bytes,
                })
            }
            Err(error) => {
                let _ = framed.send(&DaemonMessage::Error(error.clone()));
                Err(PluginError::Handshake(error))
            }
        }
    }

    pub fn grants(&self) -> &std::collections::BTreeMap<String, String> {
        &self.hello.grants
    }

    pub fn negotiation(&self) -> &Negotiation {
        &self.negotiation
    }

    pub fn payload_limit(&self) -> usize {
        self.max_payload_bytes
    }

    pub fn recv(&self, timeout: Duration) -> Result<ClientMessage, PluginError> {
        Ok(self.framed.recv_timeout(timeout)?)
    }

    pub fn send(&self, message: &DaemonMessage) -> Result<(), PluginError> {
        self.framed.send(message).map_err(PluginError::from)
    }

    pub fn capability_granted(&self, name: &str) -> bool {
        self.negotiation
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == name)
    }
}

pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::spawn::unique_pipe_name;
    use devboule_daemon::{connect_pipe, Framed};
    use devboule_protocol::{
        caps, plugin_backend_capabilities, plugin_frame_limit_for_payload, ClientHello,
        ClientMessage, DaemonMessage, DEFAULT_PLUGIN_PAYLOAD_BYTES,
    };
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn host_and_backend_handshake_on_a_named_pipe() {
        let pipe_name = unique_pipe_name("rpc-test");
        let server_name = pipe_name.clone();
        let server = thread::spawn(move || {
            let backend = PluginBackend::listen_for_host(&server_name, std::process::id())?;
            let request = backend.recv(Duration::from_secs(2))?;
            Ok::<_, PluginError>((backend, request))
        });

        let mut last_err = None;
        let file = (0..50)
            .find_map(|_| match connect_pipe(&pipe_name) {
                Ok(file) => Some(file),
                Err(error) => {
                    last_err = Some(error);
                    thread::sleep(Duration::from_millis(50));
                    None
                }
            })
            .unwrap_or_else(|| panic!("connect: {last_err:?}"));

        let owner = crate::host_owner().expect("owner");
        let mut grants = BTreeMap::new();
        grants.insert(caps::WORKSPACE_ROOT.to_string(), r"C:\repo".to_string());
        let framed = Framed::with_limit(
            file,
            plugin_frame_limit_for_payload(DEFAULT_PLUGIN_PAYLOAD_BYTES),
        );
        framed
            .send(&ClientMessage::Hello(ClientHello::plugin_host(
                owner,
                "test",
                plugin_backend_capabilities(),
                grants,
                DEFAULT_PLUGIN_PAYLOAD_BYTES,
            )))
            .expect("hello");
        let reply: DaemonMessage = framed.recv_timeout(Duration::from_secs(2)).expect("reply");
        match reply {
            DaemonMessage::Hello(hello) => {
                assert!(hello
                    .capabilities
                    .iter()
                    .any(|capability| capability.as_str() == caps::WORKSPACE_ROOT));
            }
            other => panic!("expected hello, got {other:?}"),
        }

        let payload_limit = 2 * 1024 * 1024;
        framed
            .send(&ClientMessage::Invoke {
                id: 1,
                method: caps::WORKSPACE_ROOT.to_string(),
                payload: Some(Value::String("x".repeat(payload_limit - 2))),
            })
            .expect("payload above the daemon frame cap");

        let (backend, request) = server.join().expect("join").expect("listen");
        assert_eq!(
            backend
                .grants()
                .get(caps::WORKSPACE_ROOT)
                .map(String::as_str),
            Some(r"C:\repo")
        );
        assert!(backend.capability_granted(caps::WORKSPACE_ROOT));
        match request {
            ClientMessage::Invoke { payload, .. } => {
                assert_eq!(payload, Some(Value::String("x".repeat(payload_limit - 2))));
            }
            other => panic!("expected invoke, got {other:?}"),
        }
    }

    #[test]
    fn a_client_from_the_wrong_host_pid_is_rejected_before_handshake() {
        let pipe_name = unique_pipe_name("rpc-pid-test");
        let server_name = pipe_name.clone();
        let wrong_pid = std::process::id().wrapping_add(1).max(1);
        let server = thread::spawn(move || PluginBackend::listen_for_host(&server_name, wrong_pid));

        let mut last_err = None;
        let file = (0..50)
            .find_map(|_| match connect_pipe(&pipe_name) {
                Ok(file) => Some(file),
                Err(error) => {
                    last_err = Some(error);
                    thread::sleep(Duration::from_millis(50));
                    None
                }
            })
            .unwrap_or_else(|| panic!("connect: {last_err:?}"));
        drop(file);

        match server.join().expect("join") {
            Err(PluginError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("not expected PID"));
            }
            Ok(_) => panic!("expected PID refusal, but the connection was accepted"),
            Err(other) => panic!("expected PID refusal, got {other}"),
        }
    }
}
