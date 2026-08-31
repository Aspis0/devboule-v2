use std::time::Duration;

use devboule_daemon::Framed;
use devboule_protocol::{
    negotiate, ClientHello, ClientMessage, DaemonHello, DaemonMessage, ErrorCode, Negotiation,
    WireError,
};

use crate::error::PluginError;
use crate::pipe::bind_and_accept;

const ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// The plugin-backend end of a host conversation. One connection.
pub struct PluginBackend {
    framed: Framed,
    hello: ClientHello,
    negotiation: Negotiation,
}

impl PluginBackend {
    /// Bind the pipe the host named, accept one client, complete handshake.
    pub fn listen(pipe_name: &str) -> Result<Self, PluginError> {
        let file = bind_and_accept(pipe_name, ACCEPT_TIMEOUT)?;
        let framed = Framed::new(file);
        let first: ClientMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
        let ClientMessage::Hello(client_hello) = first else {
            let error = WireError::new(ErrorCode::InvalidRequest, "first frame must be hello");
            let _ = framed.send(&DaemonMessage::Error(error.clone()));
            return Err(PluginError::Handshake(error));
        };
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
        caps, plugin_backend_capabilities, ClientHello, ClientMessage, DaemonMessage,
    };
    use std::collections::BTreeMap;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn host_and_backend_handshake_on_a_named_pipe() {
        let pipe_name = unique_pipe_name("rpc-test");
        let server_name = pipe_name.clone();
        let server = thread::spawn(move || PluginBackend::listen(&server_name));

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
        let framed = Framed::new(file);
        framed
            .send(&ClientMessage::Hello(ClientHello::plugin_host(
                owner,
                "test",
                plugin_backend_capabilities(),
                grants,
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

        let backend = server.join().expect("join").expect("listen");
        assert_eq!(
            backend.grants().get(caps::WORKSPACE_ROOT).map(String::as_str),
            Some(r"C:\repo")
        );
        assert!(backend.capability_granted(caps::WORKSPACE_ROOT));
    }
}
