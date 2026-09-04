//! PTY child used to prove the announcement channel.
//!
//! It reads the `DEVBOULE_*` environment the daemon injected, reopens the
//! named pipe, and sends `session_report_agent`. It does not touch any
//! user CLI configuration.

use std::io::{self, Write};
use std::time::Duration;

use devboule_daemon::{
    connect_pipe, handshake, DEVBOULE_ENV, DEVBOULE_SESSION_ID, DEVBOULE_SOCKET_PATH,
};
use devboule_protocol::{AgentActivityState, ClientHello, OwnerId};

fn main() -> io::Result<()> {
    let marker = std::env::var(DEVBOULE_ENV).unwrap_or_default();
    let session_id = std::env::var(DEVBOULE_SESSION_ID).unwrap_or_default();
    let socket = std::env::var(DEVBOULE_SOCKET_PATH).unwrap_or_default();
    let dump = format!(
        "DEVBOULE_ENV={marker}\nDEVBOULE_SESSION_ID={session_id}\nDEVBOULE_SOCKET_PATH={socket}\n"
    );
    print!("{dump}");
    io::stdout().flush()?;
    if let Ok(path) = std::env::var("DEVBOULE_AGENT_STUB_ENV_FILE") {
        let _ = std::fs::write(path, &dump);
    }
    if marker != "1" || session_id.is_empty() || socket.is_empty() {
        eprintln!("missing Devboule session environment");
        std::process::exit(2);
    }

    let file = connect_pipe(&socket)?;
    let owner =
        OwnerId::new("stub", format!("stub-{}", std::process::id())).map_err(io::Error::other)?;
    let client = handshake(file, ClientHello::m3a(owner, "devboule-agent-stub"))
        .map_err(|error| io::Error::other(error.to_string()))?;
    client
        .session_report_agent(
            &session_id,
            "devboule:stub",
            "stub",
            AgentActivityState::Working,
            Some(1),
            Some("stub-session".to_string()),
            None,
            Some("startup".to_string()),
            None,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
    // Stay alive so the test can attach to a live session. Close/kill ends this.
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}
