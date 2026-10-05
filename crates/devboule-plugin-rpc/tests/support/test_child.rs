//! Test child for the Unix plugin-spawn tests.
//!
//! One binary, several roles: `DEVBOULE_PLUGIN_ID` names the role the
//! integration test asked for — serve the handshake, exit before it, or fork
//! a grandchild the group kill must also take down. The channel comes in as
//! the inherited fd on `--pipe`/`DEVBOULE_PLUGIN_PIPE`, exactly like a real
//! backend.

#[cfg(not(unix))]
fn main() {
    eprintln!("the plugin test child runs on unix only");
    std::process::exit(1);
}

#[cfg(unix)]
fn main() {
    let role = std::env::var(devboule_plugin_rpc::PLUGIN_ID_ENV).unwrap_or_default();
    match role.as_str() {
        "quitter" => std::process::exit(0),
        "sleeper" => sleeper(),
        "forker" => forker(),
        "exec-probe" => exec_probe(),
        "fd-report" => fd_report(),
        _ => backend(),
    }
}

#[cfg(unix)]
fn endpoint() -> String {
    let args: Vec<String> = std::env::args().collect();
    match devboule_plugin_rpc::pipe_name_from_env_or_argv(&args) {
        Some(endpoint) => endpoint,
        None => {
            eprintln!("test child: no channel endpoint");
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
fn backend() {
    let endpoint = endpoint();
    match devboule_plugin_rpc::PluginBackend::listen(&endpoint) {
        Ok(backend) => serve(backend),
        Err(error) => {
            eprintln!("test child: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
fn forker() {
    use devboule_plugin_rpc::PLUGIN_ID_ENV;

    let endpoint = endpoint();
    let backend = match devboule_plugin_rpc::PluginBackend::listen(&endpoint) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("test child: {error}");
            std::process::exit(1);
        }
    };
    let grandchild = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .env(PLUGIN_ID_ENV, "sleeper")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match grandchild {
        Ok(_) => serve(backend),
        Err(error) => {
            eprintln!("test child: fork grandchild: {error}");
            std::process::exit(1);
        }
    }
}

/// Adopt the channel through the real listen (which restores close-on-exec
/// on it), spawn a reporter that checks whether the fd survived its exec,
/// then serve normally: the host handshake succeeds and the report file
/// carries the verdict. The reporter stays in this process group, so the
/// session teardown still reaps it.
#[cfg(unix)]
fn exec_probe() {
    let endpoint = endpoint();
    let backend = match devboule_plugin_rpc::PluginBackend::listen(&endpoint) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("test child: listen: {error}");
            std::process::exit(1);
        }
    };
    if std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg("--pipe")
        .arg(&endpoint)
        .env(devboule_plugin_rpc::PLUGIN_ID_ENV, "fd-report")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_err()
    {
        eprintln!("test child: spawn reporter");
        std::process::exit(1);
    }
    serve(backend);
}

/// Check whether the channel fd survived this process's own exec and report
/// through a pid-suffixed file the test polls. Runs with no channel use:
/// the verdict is about inheritance, not the protocol.
#[cfg(unix)]
fn fd_report() {
    let endpoint = endpoint();
    let held = endpoint.parse::<std::os::raw::c_int>().is_ok_and(|fd| {
        // SAFETY: F_GETFD only reads the descriptor flags.
        (unsafe { libc::fcntl(fd, libc::F_GETFD, 0) }) != -1
    });
    let host_pid = std::env::var(devboule_plugin_rpc::HOST_PID_ENV).unwrap_or_default();
    let path = std::env::temp_dir().join(format!("devboule-plugin-fdreport-{host_pid}"));
    let verdict = if held { "held" } else { "closed" };
    if std::fs::write(&path, verdict).is_err() {
        std::process::exit(1);
    }
}

/// Claim a pid file the test polls, then stay alive until the group signal.
#[cfg(unix)]
fn sleeper() {
    let host_pid = std::env::var(devboule_plugin_rpc::HOST_PID_ENV).unwrap_or_default();
    let path = std::env::temp_dir().join(format!("devboule-plugin-orphan-{host_pid}"));
    if let Err(error) = std::fs::write(&path, std::process::id().to_string()) {
        eprintln!("test child: write {}: {error}", path.display());
        std::process::exit(1);
    }
    std::thread::sleep(std::time::Duration::from_secs(3600));
}

#[cfg(unix)]
fn serve(backend: devboule_plugin_rpc::PluginBackend) -> ! {
    use devboule_protocol::{ClientMessage, DaemonMessage, ErrorCode, WireError};

    loop {
        let request = match backend.recv(std::time::Duration::from_secs(30)) {
            Ok(request) => request,
            Err(_) => std::process::exit(0),
        };
        match request {
            ClientMessage::Invoke { id, payload, .. } => {
                let value = payload.unwrap_or(serde_json::Value::Null);
                if backend
                    .send(&DaemonMessage::InvokeResult { id, value })
                    .is_err()
                {
                    std::process::exit(0);
                }
            }
            ClientMessage::Ping { id } => {
                let reply = DaemonMessage::Pong {
                    id,
                    ts_ms: devboule_plugin_rpc::unix_millis(),
                };
                if backend.send(&reply).is_err() {
                    std::process::exit(0);
                }
            }
            ClientMessage::Shutdown { id } => {
                let _ = backend.send(&DaemonMessage::Shutdown {
                    id,
                    accepted: true,
                    reason: None,
                });
                std::process::exit(0);
            }
            other => {
                let error = WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("test child does not answer {other:?}"),
                );
                if backend.send(&DaemonMessage::Error(error)).is_err() {
                    std::process::exit(0);
                }
            }
        }
    }
}
