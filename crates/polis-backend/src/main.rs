use std::time::Duration;

use devboule_plugin_rpc::{pipe_name_from_env_or_argv, PluginBackend};
use devboule_protocol::{ClientMessage, DaemonMessage, ErrorCode, WireError};
use polis_backend::dispatch;

fn main() {
    if let Err(error) = run() {
        eprintln!("polis-backend: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let Some(pipe_name) = pipe_name_from_env_or_argv(&args) else {
        return Err(
            "missing pipe name: pass --pipe \\\\.\\pipe\\... or set DEVBOULE_PLUGIN_PIPE".into(),
        );
    };
    let backend = PluginBackend::listen(&pipe_name)?;
    loop {
        let request = match backend.recv(Duration::from_secs(60 * 60)) {
            Ok(request) => request,
            Err(error) => {
                // Host closed the pipe or the process is being torn down.
                eprintln!("polis-backend: connection ended ({error})");
                return Ok(());
            }
        };
        if matches!(request, ClientMessage::Shutdown { .. }) {
            let id = request.request_id().unwrap_or(0);
            backend.send(&DaemonMessage::Shutdown { id, accepted: true })?;
            return Ok(());
        }
        let reply = dispatch(
            backend.grants(),
            &backend.negotiation().capabilities,
            request,
        );
        backend.send(&reply)?;
        if let DaemonMessage::Error(WireError {
            code: ErrorCode::ShuttingDown,
            ..
        }) = reply
        {
            return Ok(());
        }
    }
}
