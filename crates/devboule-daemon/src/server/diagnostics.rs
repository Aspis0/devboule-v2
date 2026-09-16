//! Diagnostics domain — pass-3a split of `server.rs`: the
//! `DaemonDiagnostics` request handler and the report it serves.
//! `diagnostics_report` composes `providers_reply`; that name resolves
//! through the parent module (which re-exports this child's items).

use super::*;

pub(super) fn diagnostics_reply(
    state: &Arc<ServerState>,
    request_id: u64,
    owner: &OwnerId,
) -> DaemonMessage {
    match diagnostics_report(state, owner) {
        Ok(report) => match serde_json::to_value(report) {
            Ok(report) => DaemonMessage::Diagnostics {
                id: request_id,
                report,
            },
            Err(error) => DaemonMessage::Error(
                WireError::new(
                    ErrorCode::Internal,
                    format!("could not encode diagnostics: {error}"),
                )
                .with_id(request_id),
            ),
        },
        Err(error) => DaemonMessage::Error(error.with_id(request_id)),
    }
}

#[cfg(windows)]
pub(super) fn host_os_version() -> String {
    use std::mem::MaybeUninit;
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut info = MaybeUninit::<OSVERSIONINFOW>::zeroed();
    let size = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    // RtlGetVersion reports the kernel version without the GetVersionEx
    // compatibility shim, which otherwise makes an unmanifested process look
    // like Windows 8. This is a bounded, local API; no shell command or
    // user-provided executable path is involved.
    unsafe {
        (*info.as_mut_ptr()).dwOSVersionInfoSize = size;
        if RtlGetVersion(info.as_mut_ptr()) == 0 {
            let info = info.assume_init();
            return format!(
                "Windows {}.{}.{} ({})",
                info.dwMajorVersion,
                info.dwMinorVersion,
                info.dwBuildNumber,
                std::env::consts::ARCH
            );
        }
    }
    format!("Windows ({})", std::env::consts::ARCH)
}

#[cfg(not(windows))]
pub(super) fn host_os_version() -> String {
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

pub(super) fn diagnostics_report(
    state: &Arc<ServerState>,
    owner: &OwnerId,
) -> Result<DiagnosticsReport, WireError> {
    state.sessions.refresh_journal_degradation();
    let sessions = state.sessions.list(owner)?;
    let output_metrics = state.sessions.output_metrics();
    let lifecycle = state
        .lifecycle
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let clients = lifecycle.clients;
    let daemon_sessions = lifecycle.sessions;
    drop(lifecycle);

    // These are the same bounded provider discovery and journal queries used
    // by existing RPCs: registry fetches and journal worker calls have finite
    // deadlines. Diagnostics never waits on a child process or an unbounded
    // database operation.
    let providers = match providers_reply(state, 0, false) {
        DaemonMessage::Providers { providers, .. } => providers,
        _ => Vec::new(),
    };
    let (journal_file_bytes, journal_file_error) = match state.sessions.journal_file_bytes() {
        Some(Ok(bytes)) => (Some(bytes), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let mut journal_error = state
        .journal_error
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .or_else(|| {
            state
                .sessions
                .has_live_journal_degradation()
                .then(|| "Journal output is degraded; some output may not be saved.".to_string())
        });
    if let Some(error) = journal_file_error {
        journal_error = Some(match journal_error {
            Some(previous) => format!("{previous}; journal file size: {error}"),
            None => format!("journal file size: {error}"),
        });
    }

    Ok(DiagnosticsReport::new(DiagnosticsInput {
        instance_id: state.instance_id.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        pid: std::process::id(),
        uptime_ms: u64::try_from(state.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        clients,
        daemon_sessions,
        capabilities: m3a_daemon_capabilities()
            .into_iter()
            .map(|capability| capability.as_str().to_string())
            .collect(),
        peak_ring_bytes: output_metrics.peak_pending_bytes,
        ring_evicted_bytes: output_metrics.coalesced_bytes,
        ring_dropped_frames: output_metrics.coalesced_frames,
        journal_stats: state.sessions.journal_stats(),
        journal_error,
        journal_schema_version: JOURNAL_SCHEMA_VERSION,
        journal_file_bytes,
        sessions,
        providers,
        os_version: host_os_version(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_dir: state.sessions.runtime_dir().to_string_lossy().into_owned(),
        pipe_name: state.sessions.pipe_name().to_string(),
        login_shell_capture: login_shell_capture_outcome(),
    }))
}
