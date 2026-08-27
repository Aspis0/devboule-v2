use std::fs::File;
use std::path::Path;
use std::time::Duration;

use devboule_protocol::{
    ClientHello, ClientMessage, DaemonHello, DaemonMessage, DaemonStatusBody, OwnerId,
};

use crate::error::DaemonError;
use crate::framing::Framed;
use crate::paths::RuntimePaths;
use crate::spawn::{resolve_daemon_binary, spawn_daemon};
use crate::transport;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const SPAWN_ATTEMPTS: u32 = 50;
const SPAWN_SLEEP: Duration = Duration::from_millis(100);

pub struct DaemonClient {
    framed: Framed,
    next_id: u64,
    hello: DaemonHello,
}

impl DaemonClient {
    pub fn hello(&self) -> &DaemonHello {
        &self.hello
    }

    pub fn ping(&mut self) -> Result<u64, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Ping { id })? {
            DaemonMessage::Pong { ts_ms, .. } => Ok(ts_ms),
            other => unexpected(other),
        }
    }

    pub fn status(&mut self) -> Result<DaemonStatusBody, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Status { id })? {
            DaemonMessage::Status { body, .. } => Ok(body),
            other => unexpected(other),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Shutdown { id })? {
            DaemonMessage::Shutdown { accepted, .. } if accepted => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn roundtrip(&mut self, message: ClientMessage) -> Result<DaemonMessage, DaemonError> {
        self.framed.send(&message)?;
        let reply: DaemonMessage = self.framed.recv()?;
        Ok(reply)
    }

    /// Write a frame without reading a reply. A client that stops reading
    /// uses this so we can prove other connections still make progress.
    pub fn write_frame(&mut self, message: &ClientMessage) -> Result<(), DaemonError> {
        self.framed.send(message)
    }

    #[cfg(windows)]
    pub fn pipe_dacl_sddl(&self) -> std::io::Result<String> {
        crate::transport::inspect_pipe_dacl(self.framed.as_file())
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

pub fn connect(paths: &RuntimePaths, hello: ClientHello) -> Result<DaemonClient, DaemonError> {
    let file = transport::connect(paths)?;
    handshake(file, hello)
}

/// Connect, spawning the daemon binary if the pipe is not up yet. Racing
/// callers converge on one daemon because the loser of the file lock exits.
pub fn connect_or_spawn(
    paths: &RuntimePaths,
    hello: ClientHello,
    daemon_binary: Option<&Path>,
) -> Result<DaemonClient, DaemonError> {
    let binary = match daemon_binary {
        Some(path) => path.to_path_buf(),
        None => resolve_daemon_binary()?,
    };
    let mut spawned = false;
    for attempt in 0..SPAWN_ATTEMPTS {
        match connect(paths, hello.clone()) {
            Ok(client) => return Ok(client),
            Err(error) => {
                if attempt + 1 == SPAWN_ATTEMPTS {
                    return Err(error);
                }
            }
        }
        if !spawned {
            match spawn_daemon(&binary, paths) {
                Ok(child) => {
                    drop(child);
                    spawned = true;
                }
                Err(error) => {
                    if attempt + 1 == SPAWN_ATTEMPTS {
                        return Err(error);
                    }
                }
            }
        }
        std::thread::sleep(SPAWN_SLEEP);
    }
    Err(DaemonError::timed_out("connecting to the daemon"))
}

pub fn handshake(file: File, hello: ClientHello) -> Result<DaemonClient, DaemonError> {
    let mut framed = Framed::new(file);
    framed.send(&ClientMessage::Hello(hello))?;
    let reply: DaemonMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    match reply {
        DaemonMessage::Hello(daemon_hello) => Ok(DaemonClient {
            framed,
            next_id: 1,
            hello: daemon_hello,
        }),
        DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
        other => unexpected(other),
    }
}

pub fn test_owner(client: &str) -> Result<OwnerId, DaemonError> {
    #[cfg(windows)]
    {
        let user = crate::security::current_user_sid()?;
        OwnerId::new(user, client).map_err(DaemonError::Protocol)
    }
    #[cfg(not(windows))]
    {
        OwnerId::new("unix", client).map_err(DaemonError::Protocol)
    }
}

fn unexpected<T>(message: DaemonMessage) -> Result<T, DaemonError> {
    Err(DaemonError::Protocol(format!(
        "unexpected daemon frame: {message:?}"
    )))
}
