//! Process lifecycle and `plugin_invoke` for an installed plugin backend.
//!
//! Spawn, Job Object, framing, and capability checks live in
//! `devboule-plugin-rpc`. This file is the thin Tauri boundary.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use devboule_plugin_rpc::{
    granted_capabilities, host_owner, PluginError, PluginSession, SpawnSpec,
};
use devboule_protocol::{caps, ErrorCode};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::{plugins_root, PluginRegistry};
use crate::backend::error::CommandError;
use crate::oracle::OracleRuntime;

struct Inner {
    sessions: HashMap<String, PluginSession>,
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

    fn ensure_session(&self, spec: SpawnSpec) -> Result<(), CommandError> {
        let plugin_id = spec.plugin_id.clone();
        let gen = {
            let mut inner = self.lock();
            if let Some(session) = inner.sessions.get(&plugin_id) {
                if session.ping().is_ok() {
                    return Ok(());
                }
            }
            if let Some(mut stale) = inner.sessions.remove(&plugin_id) {
                let _ = stale.kill_process();
            }
            inner.generation.get(&plugin_id).copied().unwrap_or(0)
        };
        let session = PluginSession::spawn(spec).map_err(command_error)?;
        let mut inner = self.lock();
        let current = inner.generation.get(&plugin_id).copied().unwrap_or(0);
        if current != gen {
            let mut session = session;
            let _ = session.kill_process();
            return Ok(());
        }
        inner.sessions.insert(plugin_id, session);
        Ok(())
    }

    fn invoke(
        &self,
        plugin_id: &str,
        spec: SpawnSpec,
        method: &str,
        payload: Option<Value>,
    ) -> Result<Value, CommandError> {
        self.ensure_session(spec)?;
        let inner = self.lock();
        let session = inner.sessions.get(plugin_id).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Io,
                format!("plugin backend '{plugin_id}' is not running"),
            )
        })?;
        let value = session.invoke(method, payload).map_err(command_error)?;
        if method == caps::WORKSPACE_ROOT {
            Ok(with_handshake(value, session))
        } else {
            Ok(value)
        }
    }

    fn hello_status(
        &self,
        plugin_id: &str,
        spec: SpawnSpec,
    ) -> Result<PluginBackendStatus, CommandError> {
        self.ensure_session(spec)?;
        let inner = self.lock();
        let session = inner.sessions.get(plugin_id).ok_or_else(|| {
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
        })
    }

    pub fn stop(&self, plugin_id: &str) {
        let mut inner = self.lock();
        let generation = inner.generation.entry(plugin_id.to_string()).or_insert(0);
        *generation = generation.wrapping_add(1);
        if let Some(mut session) = inner.sessions.remove(plugin_id) {
            let _ = session.kill_process();
        }
    }

    pub fn stop_all(&self) {
        let mut inner = self.lock();
        for generation in inner.generation.values_mut() {
            *generation = generation.wrapping_add(1);
        }
        for (_, mut session) in inner.sessions.drain() {
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
    let workspace = app.state::<OracleRuntime>().workspace().path;
    let (capabilities, grants) =
        granted_capabilities(&manifest.capabilities, workspace.as_deref());
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
pub async fn plugin_backend_stop(app: AppHandle, plugin_id: String) -> Result<(), CommandError> {
    app.state::<PluginRuntime>().stop(&plugin_id);
    Ok(())
}

#[tauri::command]
pub async fn plugin_invoke(
    app: AppHandle,
    plugin_id: String,
    method: String,
    payload: Option<Value>,
) -> Result<Value, CommandError> {
    let spec = spawn_spec(&app, &plugin_id)?;
    let runtime = (*app.state::<PluginRuntime>()).clone();
    tauri::async_runtime::spawn_blocking(move || runtime.invoke(&plugin_id, spec, &method, payload))
        .await
        .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
