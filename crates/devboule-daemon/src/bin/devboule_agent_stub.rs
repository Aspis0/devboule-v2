//! PTY child used to prove the announcement channel.
//!
//! It reads the `DEVBOULE_*` environment the daemon injected, reopens the
//! named pipe, and sends `session_report_agent`. It does not touch any
//! user CLI configuration. The pipe is Windows-only, so off Windows this
//! stays a stub that says so; the one test driving it is Windows-only too.

#[cfg(windows)]
use std::io::{self, Write};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use devboule_daemon::{
    connect_pipe, handshake, DEVBOULE_ENV, DEVBOULE_SESSION_ID, DEVBOULE_SOCKET_PATH,
};
#[cfg(windows)]
use devboule_protocol::{AgentActivityState, ClientHello, OwnerId};

#[cfg(windows)]
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
    // The integration restart/resume tests drive a life's counter and its
    // start source from outside; the defaults keep the single-report
    // contract the original test asserts.
    let agent_session_id = std::env::var("DEVBOULE_AGENT_STUB_SESSION_ID")
        .unwrap_or_else(|_| "stub-session".to_string());
    let seqs: Vec<u64> = std::env::var("DEVBOULE_AGENT_STUB_SEQS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|part| part.trim().parse::<u64>().ok())
                .collect::<Vec<u64>>()
        })
        .filter(|seqs| !seqs.is_empty())
        .unwrap_or_else(|| vec![1]);
    let start =
        std::env::var("DEVBOULE_AGENT_STUB_START").unwrap_or_else(|_| "startup".to_string());
    let session_start_source = (!start.is_empty()).then_some(start);

    let file = connect_pipe(&socket)?;
    let owner =
        OwnerId::new("stub", format!("stub-{}", std::process::id())).map_err(io::Error::other)?;
    let client = handshake(file, ClientHello::m3a(owner, "devboule-agent-stub"))
        .map_err(|error| io::Error::other(error.to_string()))?;
    for seq in seqs {
        client
            .session_report_agent(
                &session_id,
                "devboule:stub",
                "stub",
                AgentActivityState::Working,
                Some(seq),
                Some(agent_session_id.clone()),
                None,
                session_start_source.clone(),
                None,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    // Stay alive so the test can attach to a live session. Close/kill ends this.
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("devboule-agent-stub targets Windows only");
    std::process::exit(2);
}
