//! Process lifecycle and `plugin_invoke` for an installed plugin backend.
//!
//! Spawn, Job Object, framing, and capability checks live in
//! `devboule-plugin-rpc`. This file is the thin Tauri boundary.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use devboule_plugin_rpc::{
    confine_project_path, granted_capabilities, host_owner, next_generation, verify_file_digest,
    workspace_root_for_grant, PluginError, PluginSession, SpawnSpec,
};
use devboule_protocol::{caps, plugin_payload_within_limit, ErrorCode};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::{plugins_root, PluginRegistry};
use crate::backend::error::CommandError;
use crate::oracle::OracleRuntime;

struct Inner {
    sessions: HashMap<String, Arc<PluginSession>>,
    /// One blocking spawn/ping operation per plugin id. The guard is never
    /// held while the session map is locked or while a session is doing IPC.
    ensure_locks: HashMap<String, Arc<Mutex<()>>>,
    /// Bumped on stop so a spawn that finishes after the surface closed is
    /// thrown away instead of becoming an orphan.
    generation: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct PluginRuntime {
    inner: Arc<Mutex<Inner>>,
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                sessions: HashMap::new(),
                ensure_locks: HashMap::new(),
                generation: HashMap::new(),
            })),
        }
    }
}

impl PluginRuntime {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn ensure_session(&self, spec: SpawnSpec) -> Result<u64, CommandError> {
        let plugin_id = spec.plugin_id.clone();
        let ensure_lock = {
            let mut inner = self.lock();
            inner
                .ensure_locks
                .entry(plugin_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _ensure_guard = ensure_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        let existing = self.lock().sessions.get(&plugin_id).cloned();
        if let Some(session) = &existing {
            if session.invoke_in_flight() {
                // The backend is in a long dispatch (city.get / findings.get).
                // A ping would sit unread on the serialized pipe and look like
                // death; killing it mid-record_scan rolls back the ledger.
                // This is still a re-acquire of the lease: bump so a delayed
                // stop(G) from the previous surface cannot match.
                let mut inner = self.lock();
                if inner
                    .sessions
                    .get(&plugin_id)
                    .is_some_and(|current| Arc::ptr_eq(current, session))
                {
                    let generation =
                        next_generation(inner.generation.get(&plugin_id).copied().unwrap_or(0));
                    inner.generation.insert(plugin_id.clone(), generation);
                    return Ok(generation);
                }
            } else if session.ping().is_ok() {
                // A successful re-acquire is a new lease generation even
                // when it reuses the same process. This invalidates any
                // stop command issued for the previous, now-released lease.
                let mut inner = self.lock();
                if inner
                    .sessions
                    .get(&plugin_id)
                    .is_some_and(|current| Arc::ptr_eq(current, session))
                {
                    let generation =
                        next_generation(inner.generation.get(&plugin_id).copied().unwrap_or(0));
                    inner.generation.insert(plugin_id.clone(), generation);
                    return Ok(generation);
                }
            }
        }
        if let Some(stale) = existing {
            let removed = {
                let mut inner = self.lock();
                inner
                    .sessions
                    .get(&plugin_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &stale))
                    .then(|| inner.sessions.remove(&plugin_id).expect("checked session"))
            };
            if let Some(stale) = removed {
                let _ = stale.kill_process();
            }
        }

        let generation = {
            let mut inner = self.lock();
            let generation =
                next_generation(inner.generation.get(&plugin_id).copied().unwrap_or(0));
            inner.generation.insert(plugin_id.clone(), generation);
            generation
        };
        let session = Arc::new(PluginSession::spawn(spec).map_err(command_error)?);
        let mut inner = self.lock();
        let current = inner.generation.get(&plugin_id).copied().unwrap_or(0);
        if current != generation {
            drop(inner);
            let _ = session.kill_process();
            return Err(CommandError::new(
                ErrorCode::Io,
                format!("plugin backend '{plugin_id}' ensure was cancelled"),
            ));
        }
        inner.sessions.insert(plugin_id, session);
        Ok(generation)
    }

    fn invoke(
        &self,
        plugin_id: &str,
        spec: SpawnSpec,
        method: &str,
        payload: Option<Value>,
    ) -> Result<Value, CommandError> {
        self.ensure_session(spec)?;
        let session = self
            .lock()
            .sessions
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Io,
                    format!("plugin backend '{plugin_id}' is not running"),
                )
            })?;
        let value = session.invoke(method, payload).map_err(command_error)?;
        if method == caps::WORKSPACE_ROOT {
            Ok(with_handshake(value, &session))
        } else {
            Ok(value)
        }
    }

    fn hello_status(
        &self,
        plugin_id: &str,
        spec: SpawnSpec,
    ) -> Result<PluginBackendStatus, CommandError> {
        let generation = self.ensure_session(spec)?;
        let session = self
            .lock()
            .sessions
            .get(plugin_id)
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Io,
                    format!("plugin backend '{plugin_id}' is not running"),
                )
            })?;
        let ping_ok = session.ping().is_ok();
        let hello = session.hello();
        Ok(PluginBackendStatus {
            pid: session.pid(),
            instance_id: hello.instance_id.clone(),
            protocol_version: hello.protocol_version,
            capabilities: hello
                .capabilities
                .iter()
                .map(|capability| capability.as_str().to_string())
                .collect(),
            ping_ok,
            generation,
        })
    }

    pub fn stop(&self, plugin_id: &str, expected_generation: Option<u64>) {
        let session = {
            let mut inner = self.lock();
            let current = inner.generation.get(plugin_id).copied().unwrap_or(0);
            if expected_generation.is_some_and(|expected| expected != current) {
                return;
            }
            inner
                .generation
                .insert(plugin_id.to_string(), next_generation(current));
            inner.sessions.remove(plugin_id)
        };
        if let Some(session) = session {
            let _ = session.kill_process();
        }
    }

    pub fn stop_all(&self) {
        let mut inner = self.lock();
        for generation in inner.generation.values_mut() {
            *generation = next_generation(*generation);
        }
        let sessions: Vec<_> = inner.sessions.drain().map(|(_, session)| session).collect();
        drop(inner);
        for session in sessions {
            let _ = session.kill_process();
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginBackendStatus {
    pub pid: u32,
    pub instance_id: String,
    pub protocol_version: u32,
    pub capabilities: Vec<String>,
    pub ping_ok: bool,
    pub generation: u64,
}

fn with_handshake(value: Value, session: &PluginSession) -> Value {
    let hello = session.hello();
    let mut object = match value {
        Value::Object(object) => object,
        other => return other,
    };
    let capabilities: Vec<&str> = hello
        .capabilities
        .iter()
        .map(|capability| capability.as_str())
        .collect();
    object.insert(
        "handshake".to_string(),
        json!({
            "protocolVersion": hello.protocol_version,
            "instanceId": hello.instance_id,
            "pid": hello.pid,
            "capabilities": capabilities,
        }),
    );
    Value::Object(object)
}

fn spawn_spec<R: Runtime>(app: &AppHandle<R>, plugin_id: &str) -> Result<SpawnSpec, CommandError> {
    let root = plugins_root(app).ok_or_else(|| {
        CommandError::new(
            ErrorCode::Internal,
            "this machine did not say where application data belongs, so there is nowhere to find a plugin backend",
        )
    })?;
    let manifest = app
        .state::<PluginRegistry>()
        .ready_manifest(&root, plugin_id)
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::InvalidRequest,
                format!("plugin '{plugin_id}' is not installed and verified"),
            )
        })?;
    let Some(backend) = manifest.backend_entry.as_ref() else {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            format!("plugin '{plugin_id}' did not declare a backend"),
        ));
    };
    let binary = root.join(plugin_id).join(backend);
    if !binary.is_file() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            format!(
                "plugin '{plugin_id}' backend {} is missing",
                binary.display()
            ),
        ));
    }
    let expected_digest = manifest.files.get(backend).ok_or_else(|| {
        CommandError::new(
            ErrorCode::InvalidRequest,
            format!("plugin '{plugin_id}' backend is not covered by its verified manifest"),
        )
    })?;
    verify_file_digest(&binary, expected_digest).map_err(|error| {
        CommandError::new(
            ErrorCode::InvalidRequest,
            format!("plugin '{plugin_id}' backend verification failed: {error}"),
        )
    })?;
    let workspace = if manifest
        .capabilities
        .iter()
        .any(|capability| capability == caps::WORKSPACE_ROOT)
    {
        let workspace = app
            .state::<OracleRuntime>()
            .workspace()
            .path
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "workspace.root is unavailable because no project is open",
                )
            })?;
        let workspace =
            confine_project_path(std::path::Path::new(&workspace)).map_err(|error| {
                CommandError::new(
                    ErrorCode::WorkspaceConfinementRefused,
                    format!("workspace.root refused: {error}"),
                )
            })?;
        Some(workspace_root_for_grant(&workspace))
    } else {
        None
    };
    let (capabilities, grants) = granted_capabilities(&manifest.capabilities, workspace.as_deref());
    Ok(SpawnSpec {
        binary,
        plugin_id: plugin_id.to_string(),
        capabilities,
        grants,
        owner: host_owner().map_err(command_error)?,
        hang_ms: None,
    })
}

fn command_error(error: PluginError) -> CommandError {
    match error {
        PluginError::Handshake(wire) => CommandError::from(wire),
        PluginError::CapabilityNotSupported(method) => CommandError::new(
            ErrorCode::CapabilityNotSupported,
            format!("plugin method '{method}' was not in the granted capability set"),
        ),
        PluginError::TimedOut(what) => {
            CommandError::new(ErrorCode::Io, format!("timed out: {what}"))
        }
        PluginError::Io(error) => CommandError::new(ErrorCode::Io, error.to_string()),
        PluginError::ProcessExited => CommandError::new(
            ErrorCode::Io,
            "the plugin backend process exited during the request",
        ),
        PluginError::Protocol(message) => CommandError::new(ErrorCode::Internal, message),
    }
}

#[tauri::command]
pub async fn plugin_backend_ensure(
    app: AppHandle,
    plugin_id: String,
) -> Result<PluginBackendStatus, CommandError> {
    let spec = spawn_spec(&app, &plugin_id)?;
    let runtime = (*app.state::<PluginRuntime>()).clone();
    tauri::async_runtime::spawn_blocking(move || runtime.hello_status(&plugin_id, spec))
        .await
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
}

#[tauri::command]
pub async fn plugin_backend_stop(
    app: AppHandle,
    plugin_id: String,
    generation: Option<u64>,
) -> Result<(), CommandError> {
    app.state::<PluginRuntime>().stop(&plugin_id, generation);
    Ok(())
}

#[tauri::command]
pub async fn plugin_invoke(
    app: AppHandle,
    plugin_id: String,
    method: String,
    payload: Option<Value>,
) -> Result<Value, CommandError> {
    if !plugin_payload_within_limit(payload.as_ref()) {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "plugin invoke payload is too large (maximum 1 MiB)",
        ));
    }
    let spec = spawn_spec(&app, &plugin_id)?;
    let runtime = (*app.state::<PluginRuntime>()).clone();
    let value = tauri::async_runtime::spawn_blocking(move || {
        runtime.invoke(&plugin_id, spec, &method, payload)
    })
    .await
    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))??;
    if !plugin_payload_within_limit(Some(&value)) {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "plugin response is too large (maximum 1 MiB)",
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_protocol::plugin_backend_capabilities;

    #[test]
    fn capability_errors_keep_their_code() {
        let error = command_error(PluginError::CapabilityNotSupported(
            "oracle.search".to_string(),
        ));
        assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
        assert!(error.message.contains("oracle.search"));
    }

    #[test]
    fn a_dead_backend_is_an_io_error_on_the_bridge() {
        let error = command_error(PluginError::ProcessExited);
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("exited"));
    }

    #[cfg(windows)]
    fn polis_backend_binary() -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.push("target");
        path.push("debug");
        path.push("polis-backend.exe");
        path
    }

    #[cfg(windows)]
    fn runtime_spec(root: &std::path::Path) -> SpawnSpec {
        let mut grants = std::collections::BTreeMap::new();
        grants.insert(
            caps::WORKSPACE_ROOT.to_string(),
            root.to_string_lossy().into_owned(),
        );
        SpawnSpec {
            binary: polis_backend_binary(),
            plugin_id: "polis-stale-stop".to_string(),
            capabilities: plugin_backend_capabilities(),
            grants,
            owner: host_owner().expect("owner"),
            hang_ms: Some(6_000),
        }
    }

    #[cfg(windows)]
    #[test]
    fn inflight_reensure_bumps_generation_so_stale_stop_is_a_noop() {
        let binary = polis_backend_binary();
        assert!(
            binary.is_file(),
            "need a built polis-backend at {}",
            binary.display()
        );
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("src.rs"), "pub fn x() {}\n").expect("source");
        let spec = runtime_spec(dir.path());
        let plugin_id = spec.plugin_id.clone();
        let runtime = PluginRuntime::default();

        let worker_runtime = runtime.clone();
        let worker_spec = spec.clone();
        let worker_id = plugin_id.clone();
        let worker = std::thread::spawn(move || {
            worker_runtime.invoke(&worker_id, worker_spec, caps::FINDINGS_GET, None)
        });
        std::thread::sleep(std::time::Duration::from_millis(400));

        let g = runtime
            .lock()
            .generation
            .get(&plugin_id)
            .copied()
            .expect("worker ensure must have registered a generation");
        let pid = runtime
            .lock()
            .sessions
            .get(&plugin_id)
            .expect("session")
            .pid();
        assert!(
            runtime
                .lock()
                .sessions
                .get(&plugin_id)
                .is_some_and(|session| session.invoke_in_flight()),
            "slow invoke must be in flight before the remount ensure"
        );

        let g2 = runtime.ensure_session(spec).expect("inflight re-ensure");
        assert!(
            g2 > g,
            "inflight re-ensure must bump the lease (got {g2}, had {g})"
        );

        runtime.stop(&plugin_id, Some(g));
        assert_eq!(
            runtime
                .lock()
                .sessions
                .get(&plugin_id)
                .map(|session| session.pid()),
            Some(pid),
            "stop(G) after a bumped re-ensure must not kill the busy backend"
        );
        assert!(
            runtime.lock().sessions.contains_key(&plugin_id),
            "session must stay registered"
        );

        worker
            .join()
            .expect("worker")
            .expect("slow invoke must complete after a stale stop");
        assert_eq!(
            runtime
                .lock()
                .sessions
                .get(&plugin_id)
                .map(|session| session.pid()),
            Some(pid)
        );
    }
}
