//! Providers domain — pass-3a split of `server.rs`: the provider
//! catalogue surface (`ProvidersList`, updates, native version probing).

use super::*;

pub(super) fn providers_reply(state: &Arc<ServerState>, id: u64, force: bool) -> DaemonMessage {
    // The settings list is also the normal pre-session discovery path. Make
    // sure a Claude session can start with a non-empty model manifest even if
    // the user has not opened the settings panel's Refresh button.
    if !force {
        let _ = state.claude_models();
    }
    let mut discovery = if force {
        refresh_provider_catalog(state)
    } else {
        crate::provider_catalog::discover_catalog(
            &crate::registry::CdnRegistryFetch,
            state.sessions.runtime_dir(),
        )
    };
    append_user_rows(&mut discovery);
    // ProviderInfo.authentication carries the measured last-start outcome for
    // this provider. It is a recorded observation, not an auth probe.
    let providers = discovery
        .agents
        .into_iter()
        .map(|agent| {
            let authentication = state.provider_health(&agent.id);
            wire_provider(state, agent, authentication)
        })
        .collect();
    DaemonMessage::Providers {
        id,
        providers,
        unreadable_dirs: discovery.unreadable_dirs,
    }
}

/// User-declared rows join the discovered catalogue behind the same answer
/// the spawn road gives: `resolve_named` reads the live rows before the
/// PATH/CDN walk, so a discovered row with the same id yields to the
/// declaration here too — one id, one row, the launchable one. (Reserved
/// names keep the two from meeting in practice; this is the rule, not the
/// luck.) The vocabulary needs nothing parallel: a user row rides ACP, and
/// `absent` is the honest vocabulary for an agent-defined dialect.
fn append_user_rows(discovery: &mut crate::provider_catalog::ProviderDiscovery) {
    let rows = crate::session::catalog_registry().user_agents();
    if rows.is_empty() {
        return;
    }
    discovery.agents.retain(|agent| {
        !rows
            .iter()
            .any(|row| row.id.eq_ignore_ascii_case(&agent.id))
    });
    discovery.agents.extend(rows);
}

fn refresh_provider_catalog(
    state: &Arc<ServerState>,
) -> crate::provider_catalog::ProviderDiscovery {
    let directories = crate::provider_catalog::path_directories();
    let mut local = crate::provider_catalog::discover_in_paths(&directories);
    crate::provider_catalog::attach_spawn_path_env(&mut local);
    let cache_dir = state.sessions.runtime_dir().to_path_buf();

    let registry_cache_dir = cache_dir.clone();
    let registry_refresh = std::thread::spawn(move || {
        crate::registry::refresh_npx_entries(
            &crate::registry::CdnRegistryFetch,
            &registry_cache_dir,
        );
    });

    let mut npm_packages = HashSet::new();
    let mut latest_fetches = Vec::new();
    for package in crate::provider_catalog::KNOWN_AGENTS
        .iter()
        .filter_map(|agent| agent.npm_package)
    {
        if !npm_packages.insert(package) {
            continue;
        }
        latest_fetches.push(std::thread::spawn(move || {
            let _ = crate::registry::load_latest_npm_version(
                &crate::registry::CdnNpmVersionFetch,
                package,
                true,
            );
        }));
    }

    let mut version_probes = Vec::new();
    // This fan-out is structurally bounded: native probes are at most one per
    // fixed KNOWN_AGENTS row (plus fixed debug test rows), npm fetches are at
    // most one per distinct const package name, plus the single registry
    // refresh. Do not make this registry-driven without adding an explicit
    // concurrency bound.
    for agent in local
        .agents
        .iter()
        .filter(|agent| agent.install_channel == crate::provider_catalog::InstallChannel::Native)
    {
        let state = Arc::clone(state);
        let agent = agent.clone();
        version_probes.push(std::thread::spawn(move || {
            probe_native_version(&state, &agent)
                .map(|(version, fingerprint)| (agent.id, version, fingerprint))
        }));
    }

    let _ = registry_refresh.join();
    for fetch in latest_fetches {
        let _ = fetch.join();
    }
    for probe in version_probes {
        if let Ok(Some((provider_id, version, fingerprint))) = probe.join() {
            state.record_provider_cli_version(&provider_id, &version, fingerprint);
        }
    }

    let _ = state.claude_models();

    let mut discovery = crate::provider_catalog::discover_catalog_in_paths(
        &crate::registry::CdnRegistryFetch,
        state.sessions.runtime_dir(),
        &directories,
    );
    crate::provider_catalog::attach_spawn_path_env(&mut discovery);
    discovery
}

fn wire_provider(
    state: &ServerState,
    agent: crate::provider_catalog::InstalledAgent,
    authentication: String,
) -> devboule_protocol::ProviderInfo {
    let installed_version = match agent.install_channel {
        crate::provider_catalog::InstallChannel::Native => agent
            .installed_version
            .clone()
            .or_else(|| state.provider_cli_version(&agent.id, &agent.executable)),
        crate::provider_catalog::InstallChannel::Npm => agent.installed_version.clone(),
        crate::provider_catalog::InstallChannel::NpxRegistry => None,
    };
    let latest_version = agent.latest_version.clone().or_else(|| {
        agent
            .npm_package
            .and_then(crate::registry::cached_latest_npm_version)
    });
    devboule_protocol::ProviderInfo {
        id: agent.id.to_string(),
        executable: agent.executable.to_string_lossy().into_owned(),
        acp_available: agent.acp_command.is_some(),
        authentication,
        protocol: crate::provider_catalog::chat_protocol(&agent).map(str::to_string),
        origin: agent.installed.then(|| agent.origin.as_wire().to_string()),
        launch_args: agent.launch_args,
        pickable: agent.pickable,
        installed_version,
        latest_version,
        agent_version: state.provider_version(&agent.id),
        install_channel: Some(agent.install_channel.as_wire().to_string()),
        installed: agent.installed,
        npm_package: agent.npm_package.map(str::to_string),
        tools: agent.tools,
    }
}

/// Not gate-token'd: the caller is the provider-update worker `dispatch`
/// spawns **after** the gate, and a `&GatePassed` cannot cross that thread
/// boundary. Making the token `Clone` to get it there would make it
/// storable and re-usable for a later request, which is worse than this
/// function staying reachable only from `dispatch.rs`.
pub(super) fn provider_update_reply(
    state: &Arc<ServerState>,
    id: u64,
    provider_id: &str,
) -> DaemonMessage {
    #[cfg(test)]
    let discovery_override = state
        .provider_update_catalog
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let agents: Vec<crate::provider_catalog::InstalledAgent> = if let Some(discovery) = {
        #[cfg(test)]
        {
            discovery_override
        }
        #[cfg(not(test))]
        {
            None::<crate::provider_catalog::ProviderDiscovery>
        }
    } {
        discovery.agents
    } else {
        crate::provider_catalog::discover_catalog(
            &crate::registry::CdnRegistryFetch,
            state.sessions.runtime_dir(),
        )
        .agents
    };
    let agent = agents.into_iter().find(|agent| agent.id == provider_id);
    let Some(agent) = agent else {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Unknown provider '{provider_id}'."),
            )
            .with_id(id),
        );
    };

    match agent.install_channel {
        crate::provider_catalog::InstallChannel::Native => {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Provider '{provider_id}' uses a native installation; npm updates are unavailable for native providers."
                    ),
                )
                .with_id(id),
            );
        }
        crate::provider_catalog::InstallChannel::NpxRegistry => {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Provider '{provider_id}' is an npx-registry wrapper; update its registry entry instead of installing it globally."
                    ),
                )
                .with_id(id),
            );
        }
        crate::provider_catalog::InstallChannel::Npm => {}
    }
    let Some(package) = agent.npm_package else {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Provider '{provider_id}' has no known npm package and cannot be updated with npm."
                ),
            )
            .with_id(id),
        );
    };
    if crate::provider_catalog::known_npm_package(provider_id) != Some(package) {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Provider '{provider_id}' is not a known npm provider row."),
            )
            .with_id(id),
        );
    }

    let npm_command = {
        #[cfg(test)]
        {
            match state
                .provider_update_npm_command
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
            {
                Some(ProviderUpdateNpmCommand::Resolved(program, prefix_args)) => {
                    Some((program, prefix_args))
                }
                Some(ProviderUpdateNpmCommand::Missing) => None,
                None => crate::provider_catalog::resolve_npm_command(
                    &crate::provider_catalog::path_directories(),
                ),
            }
        }
        #[cfg(not(test))]
        {
            crate::provider_catalog::resolve_npm_command(
                &crate::provider_catalog::path_directories(),
            )
        }
    };
    let Some((program, prefix_args)) = npm_command else {
        return DaemonMessage::ProviderUpdated {
            id,
            ok: false,
            exit_code: None,
            log: "npm was not found on PATH; install Node.js/npm and try again.".to_string(),
        };
    };
    let args = vec![
        "install".to_string(),
        "-g".to_string(),
        format!("{package}@latest"),
    ];
    // The install gets its own job, created empty: assigning into a job
    // that already lived through other processes is refused at the kernel
    // with ERROR_ACCESS_DENIED once its hierarchy has parented terminated
    // jobs. It dies with the run, as a git probe's does.
    let job = match crate::process_tree::JobObject::new() {
        Ok(job) => job,
        Err(error) => {
            return DaemonMessage::ProviderUpdated {
                id,
                ok: false,
                exit_code: None,
                log: format!("could not create the npm install job: {error}"),
            };
        }
    };
    let result = state
        .npm_install_runner
        .run(&program, &prefix_args, &args, &job);
    let ok = result.exit_code == Some(0);
    if ok {
        state.invalidate_provider_update_caches(provider_id);
    }
    DaemonMessage::ProviderUpdated {
        id,
        ok,
        exit_code: result.exit_code,
        log: crate::provider_update::bounded_log(result.log.as_bytes()),
    }
}

pub(super) fn probe_native_version(
    state: &ServerState,
    agent: &crate::provider_catalog::InstalledAgent,
) -> Option<(String, CliVersionFingerprint)> {
    #[cfg(test)]
    {
        // Counted above the stubbed body: the entry is the seam a real
        // spawn flows through, so a test that pins "no process" watches
        // this counter and not `provider_health`.
        state.version_probe_entries.fetch_add(1, Ordering::SeqCst);
        let _ = agent;
        None
    }
    #[cfg(not(test))]
    {
        if agent.id == "claude" && std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
            return None;
        }
        // The production arm reads the agent row it was handed and gives the
        // probe its own empty job; `state` carries only the test-build probe
        // counter above.
        let _ = state;
        let fingerprint = executable_fingerprint(&agent.executable)?;
        let mut command = Command::new(&agent.executable);
        command
            .args(&agent.prefix_args)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Same rule as a session spawn: a provider found through a registry
        // PATH folder is probed with that folder visible.
        if let Some((key, value)) = &agent.spawn_path_env {
            command.env(key, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        // The probe gets its own job, created empty, like a git probe's:
        // assigning into a job that already lived through other processes
        // is refused at the kernel with ERROR_ACCESS_DENIED once its
        // hierarchy has parented terminated jobs. The binding must live to
        // the end of this function — the child is reaped below — because
        // dropping the job earlier fires KILL_ON_JOB_CLOSE and kills the
        // probe mid-run.
        #[cfg(windows)]
        let job = match crate::process_tree::JobObject::new() {
            Ok(job) => job,
            Err(_) => {
                return None;
            }
        };
        let mut child = command.spawn().ok()?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            if job.assign(child.as_raw_handle()).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        let version = parse_version_token(&output.stdout)?;
        Some((version, fingerprint))
    }
}

pub(super) fn executable_fingerprint(path: &std::path::Path) -> Option<CliVersionFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(CliVersionFingerprint {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

pub(super) fn cli_version_cache_is_current(
    cached: &CliVersionFingerprint,
    current: Option<&CliVersionFingerprint>,
) -> bool {
    current == Some(cached)
}

pub(super) fn parse_version_token(output: &[u8]) -> Option<String> {
    let mut run = String::new();
    let inspect = |run: &mut String| {
        let candidate = std::mem::take(run);
        let components: Vec<&str> = candidate.split('.').collect();
        (components.len() >= 3
            && components.iter().all(|component| {
                !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit())
            }))
        .then_some(candidate)
    };
    for byte in output {
        if byte.is_ascii_digit() || *byte == b'.' {
            run.push(*byte as char);
        } else if let Some(version) = inspect(&mut run) {
            return crate::provider_catalog::cap_external_version(&version);
        }
    }
    inspect(&mut run).and_then(|version| crate::provider_catalog::cap_external_version(&version))
}

/// Collapse an error message to a single line for the provider-health
/// string: newlines, tabs and repeated spaces become single spaces, then
/// the result is truncated to 200 chars.
pub(super) fn collapse_health_reason(message: &str) -> String {
    let mut reason = String::with_capacity(message.len());
    let mut pending_space = false;
    for ch in message.chars() {
        if ch.is_whitespace() {
            pending_space = !reason.is_empty();
        } else {
            if pending_space {
                reason.push(' ');
                pending_space = false;
            }
            reason.push(ch);
        }
    }
    if reason.chars().count() > 200 {
        reason = reason.chars().take(200).collect();
    }
    reason
}
