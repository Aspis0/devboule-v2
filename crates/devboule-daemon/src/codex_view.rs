//! Translate Codex app-server notifications into Devboule agent events.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ChildStdout;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Instant;

use devboule_protocol::{
    ErrorCode, NoticeSeverity, PermissionOption, SessionEvent, SessionModeStateView,
    SessionModeView, SessionModel, SessionModelEffort, ToolLocation, TurnUsage, WireError,
};
use serde_json::Value;

use crate::shell_unwrap::normalize_command_execution_command;
use crate::tool_paths::relativize_tool_path;

pub(crate) const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) struct CodexStdout {
    receiver: Receiver<io::Result<Vec<u8>>>,
    buffer: Vec<u8>,
}

impl CodexStdout {
    pub(crate) fn spawn(mut stdout: ChildStdout) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("session-codex-stdout".to_string())
            .spawn(move || {
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => return,
                        Ok(length) => {
                            if sender.send(Ok(buffer[..length].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            receiver,
            buffer: Vec::new(),
        })
    }

    pub(crate) fn next_line(&mut self, deadline: Instant) -> io::Result<Option<String>> {
        loop {
            if let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
                if newline.saturating_add(1) > MAX_LINE_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Codex line exceeded 10 MiB",
                    ));
                }
                let line: Vec<u8> = self.buffer.drain(..=newline).collect();
                let line = line.strip_suffix(b"\n").unwrap_or(&line);
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                return Ok(Some(String::from_utf8_lossy(line).into_owned()));
            }
            if self.buffer.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Codex line exceeded 10 MiB",
                ));
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Codex handshake timed out",
                ));
            }
            match self.receiver.recv_timeout(timeout) {
                Ok(Ok(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Codex handshake timed out",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

impl Read for CodexStdout {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        while self.buffer.is_empty() {
            match self.receiver.recv() {
                Ok(Ok(bytes)) => self.buffer.extend_from_slice(&bytes),
                Ok(Err(error)) => return Err(error),
                Err(_) => return Ok(0),
            }
        }
        let length = output.len().min(self.buffer.len());
        output[..length].copy_from_slice(&self.buffer[..length]);
        self.buffer.drain(..length);
        Ok(length)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CodexModel {
    id: String,
    name: String,
    description: String,
    efforts: Vec<SessionModelEffort>,
    context_window: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct CodexCatalog {
    models: Vec<CodexModel>,
    current_model_id: String,
    current_effort: Option<String>,
}

impl CodexCatalog {
    pub(crate) fn apply_thread_response(&mut self, response: &Value) {
        if let Some(model) = response.get("model").and_then(Value::as_str) {
            self.current_model_id = model.to_string();
        }
        if let Some(effort) = response.get("reasoningEffort").and_then(Value::as_str) {
            self.current_effort = Some(effort.to_string());
        }
    }
}

pub(crate) struct CodexState {
    thread_id: String,
    mode_id: Mutex<String>,
    /// The mode a `set_mode` applied after `thread/start`, if any. Paseo
    /// (`hasWorkflowModeOverride`) keeps this set for every later turn; the
    /// thread already carries the preset, so only an explicit change re-sends
    /// the policy on `turn/start`.
    mode_override: Mutex<Option<String>>,
    catalog: Mutex<CodexCatalog>,
    turn_id: Mutex<Option<String>>,
}

impl CodexState {
    pub(crate) fn new(thread_id: String, catalog: CodexCatalog, mode_id: &str) -> Self {
        Self {
            thread_id,
            mode_id: Mutex::new(mode_id.to_string()),
            mode_override: Mutex::new(None),
            catalog: Mutex::new(catalog),
            turn_id: Mutex::new(None),
        }
    }

    pub(crate) fn thread_id(&self) -> String {
        self.thread_id.clone()
    }

    pub(crate) fn mode_override(&self) -> Option<String> {
        self.mode_override.lock().ok().and_then(|mode| mode.clone())
    }

    pub(crate) fn current_turn(&self) -> Option<String> {
        self.turn_id.lock().ok().and_then(|turn| turn.clone())
    }

    pub(crate) fn set_turn(&self, turn_id: Option<String>) {
        if let Ok(mut current) = self.turn_id.lock() {
            *current = turn_id;
        }
    }

    pub(crate) fn mode_id(&self) -> Option<String> {
        self.mode_id.lock().ok().map(|mode| mode.clone())
    }

    pub(crate) fn model_and_effort(&self) -> (String, Option<String>) {
        self.catalog
            .lock()
            .map(|catalog| {
                (
                    catalog.current_model_id.clone(),
                    catalog.current_effort.clone(),
                )
            })
            .unwrap_or_else(|_| (String::new(), None))
    }

    pub(crate) fn manifest(&self) -> SessionEvent {
        let mode_id = self.mode_id().unwrap_or_else(|| "auto".to_string());
        let catalog = self.catalog.lock().expect("Codex catalog lock");
        manifest_from_catalog(&catalog, &mode_id)
    }

    pub(crate) fn set_context_window(&self, context_window: u64) {
        if let Ok(mut catalog) = self.catalog.lock() {
            let current_model_id = catalog.current_model_id.clone();
            if let Some(model) = catalog
                .models
                .iter_mut()
                .find(|model| model.id == current_model_id)
            {
                model.context_window = Some(context_window);
            }
        }
    }

    pub(crate) fn set_model(
        &self,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        let mut catalog = self
            .catalog
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Codex model catalog is unavailable."))?;
        let selected = model_id.unwrap_or(catalog.current_model_id.as_str());
        let model = catalog
            .models
            .iter()
            .find(|model| model.id == selected)
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Codex model '{selected}' is not in model/list."),
                )
            })?;
        let available_efforts = model
            .efforts
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        if let Some(effort) = effort {
            if !available_efforts.iter().any(|entry| entry == effort) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Codex effort '{effort}' is not available for model '{selected}'."),
                ));
            }
        }
        let previous_effort = catalog.current_effort.clone();
        catalog.current_model_id = selected.to_string();
        catalog.current_effort = effort.map(str::to_string).or_else(|| {
            available_efforts
                .iter()
                .any(|entry| Some(entry.as_str()) == previous_effort.as_deref())
                .then_some(previous_effort)
                .flatten()
        });
        Ok(())
    }

    pub(crate) fn set_mode(&self, mode_id: &str) -> Result<(), WireError> {
        validate_mode(mode_id)?;
        *self
            .mode_id
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Codex mode state is unavailable."))? =
            mode_id.to_string();
        *self
            .mode_override
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Io, "Codex mode state is unavailable."))? =
            Some(mode_id.to_string());
        Ok(())
    }
}

pub(crate) fn catalog_from_response(response: &Value) -> Result<CodexCatalog, WireError> {
    let models = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| catalog_error("model/list response had no data"))?;
    let mut parsed = Vec::new();
    let mut default_model = None;
    for model in models {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let efforts = model
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .map(|efforts| {
                efforts
                    .iter()
                    .filter_map(|effort| {
                        Some(SessionModelEffort {
                            id: effort.get("reasoningEffort")?.as_str()?.to_string(),
                            label: effort.get("reasoningEffort")?.as_str()?.to_string(),
                            description: effort
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            default: Some(
                                model.get("defaultReasoningEffort").and_then(Value::as_str)
                                    == effort.get("reasoningEffort").and_then(Value::as_str),
                            ),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if model.get("isDefault").and_then(Value::as_bool) == Some(true) {
            default_model = Some(id.to_string());
        }
        parsed.push(CodexModel {
            id: id.to_string(),
            name: model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string(),
            description: model
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            efforts,
            context_window: None,
        });
    }
    if parsed.is_empty() {
        return Err(catalog_error("model/list response had no models"));
    }
    let current_model_id = default_model.unwrap_or_else(|| parsed[0].id.clone());
    Ok(CodexCatalog {
        models: parsed,
        current_model_id,
        current_effort: None,
    })
}

fn catalog_error(message: &str) -> WireError {
    WireError::new(
        ErrorCode::Io,
        format!("Could not start Codex session: {message}"),
    )
}

fn manifest_from_catalog(catalog: &CodexCatalog, mode_id: &str) -> SessionEvent {
    SessionEvent::SessionManifest {
        provider_id: Some("codex".to_string()),
        current_model_id: Some(catalog.current_model_id.clone()),
        models: catalog
            .models
            .iter()
            .map(|model| SessionModel {
                model_id: model.id.clone(),
                name: model.name.clone(),
                description: (!model.description.is_empty()).then(|| model.description.clone()),
                context_tokens: model.context_window,
                current_effort: (model.id == catalog.current_model_id)
                    .then(|| catalog.current_effort.clone())
                    .flatten(),
                efforts: (!model.efforts.is_empty()).then(|| model.efforts.clone()),
            })
            .collect(),
        modes: Some(SessionModeStateView {
            current_mode_id: mode_id.to_string(),
            available_modes: vec![
                SessionModeView {
                    id: "read-only".to_string(),
                    name: "Read Only".to_string(),
                    description: Some(
                        "Read files and run read-only commands; Codex cannot edit files or access the network."
                            .to_string(),
                    ),
                },
                SessionModeView {
                    id: "auto".to_string(),
                    name: "Default Permissions".to_string(),
                    description: Some(
                        "Edit files and run commands with Codex's default approval flow."
                            .to_string(),
                    ),
                },
                SessionModeView {
                    id: "auto-review".to_string(),
                    name: "Auto-review".to_string(),
                    description: Some("Same workspace-write permissions as Default, but eligible `on-request` approvals are routed through the auto-reviewer subagent.".to_string()),
                },
                SessionModeView {
                    id: "full-access".to_string(),
                    name: "Full Access".to_string(),
                    description: Some("Edit files, run commands, and access the network without additional prompts.".to_string()),
                },
            ],
        }),
    }
}

pub(crate) fn validate_mode(mode_id: &str) -> Result<(), WireError> {
    if matches!(
        mode_id,
        "read-only" | "auto" | "auto-review" | "full-access"
    ) {
        Ok(())
    } else {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!("Codex session mode '{mode_id}' is not available."),
        ))
    }
}

pub(crate) fn mode_values(mode_id: &str) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    match mode_id {
        "read-only" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandboxPolicy".to_string(),
                serde_json::json!({ "type": "readOnly" }),
            );
        }
        "full-access" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("never".to_string()),
            );
            values.insert(
                "sandboxPolicy".to_string(),
                serde_json::json!({ "type": "dangerFullAccess" }),
            );
        }
        "auto-review" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandboxPolicy".to_string(),
                serde_json::json!({
                    "type": "workspaceWrite",
                    "networkAccess": false,
                    "writableRoots": []
                }),
            );
            values.insert(
                "approvalsReviewer".to_string(),
                Value::String("auto_review".to_string()),
            );
        }
        _ => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandboxPolicy".to_string(),
                serde_json::json!({
                    "type": "workspaceWrite",
                    "networkAccess": false,
                    "writableRoots": []
                }),
            );
        }
    }
    values
}

pub(crate) fn thread_mode_values(mode_id: &str) -> serde_json::Map<String, Value> {
    let mut values = serde_json::Map::new();
    match mode_id {
        "read-only" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandbox".to_string(),
                Value::String("read-only".to_string()),
            );
        }
        "full-access" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("never".to_string()),
            );
            values.insert(
                "sandbox".to_string(),
                Value::String("danger-full-access".to_string()),
            );
        }
        "auto-review" => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandbox".to_string(),
                Value::String("workspace-write".to_string()),
            );
            values.insert(
                "approvalsReviewer".to_string(),
                Value::String("auto_review".to_string()),
            );
        }
        _ => {
            values.insert(
                "approvalPolicy".to_string(),
                Value::String("on-request".to_string()),
            );
            values.insert(
                "sandbox".to_string(),
                Value::String("workspace-write".to_string()),
            );
        }
    }
    values
}

pub(crate) fn permission_request_event(params: &Value, file_change: bool) -> Option<SessionEvent> {
    let item_id = params.get("itemId").and_then(Value::as_str)?;
    Some(SessionEvent::PermissionRequest {
        tool_call_id: item_id.to_string(),
        title: if file_change {
            "Allow Codex to edit files?".to_string()
        } else {
            "Allow Codex to run this command?".to_string()
        },
        description: params
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        command: (!file_change)
            .then(|| params.get("command").and_then(Value::as_str))
            .flatten()
            .map(str::to_string),
        args: None,
        cwd: params
            .get("cwd")
            .or_else(|| params.get("grantRoot"))
            .and_then(Value::as_str)
            .map(str::to_string),
        env: None,
        options: vec![
            PermissionOption {
                option_id: "allow".to_string(),
                name: "Allow once".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
    })
}

pub(crate) struct CodexView {
    cwd: Option<PathBuf>,
    usage: Option<TurnUsage>,
    context_window_update: Option<u64>,
}

impl CodexView {
    pub(crate) fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            cwd,
            usage: None,
            context_window_update: None,
        }
    }

    pub(crate) fn ingest(&mut self, value: &Value) -> Vec<SessionEvent> {
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            return Vec::new();
        };
        let params = value.get("params").unwrap_or(&Value::Null);
        match method {
            "item/agentMessage/delta" => {
                delta_event(params, "AgentMessage", "itemId", "delta", |id, text| {
                    SessionEvent::AgentMessage {
                        message_id: id,
                        text,
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    }
                })
            }
            "item/reasoning/summaryTextDelta" => {
                delta_event(params, "AgentThought", "itemId", "delta", |id, text| {
                    SessionEvent::AgentThought {
                        message_id: id,
                        text,
                        parent_tool_use_id: None,
                        spawn_depth: None,
                    }
                })
            }
            "item/commandExecution/outputDelta" => tool_delta(params, "execute"),
            "item/fileChange/outputDelta" => tool_delta(params, "edit"),
            "item/started" => item_event(params.get("item"), false, self.cwd.as_deref()),
            "item/completed" => item_event(params.get("item"), true, self.cwd.as_deref()),
            "thread/tokenUsage/updated" => {
                self.note_usage(params.get("tokenUsage"));
                Vec::new()
            }
            "turn/completed" => turn_completed(params, self.usage.take()),
            "error" => params
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(|message| {
                    vec![SessionEvent::SessionNotice {
                        text: message.to_string(),
                        severity: NoticeSeverity::Warning,
                    }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn take_context_window_update(&mut self) -> Option<u64> {
        self.context_window_update.take()
    }

    fn note_usage(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        if let Some(context_window) = value.get("modelContextWindow").and_then(Value::as_u64) {
            self.context_window_update = Some(context_window);
        }
        let Some(last) = value.get("last") else {
            return;
        };
        let usage = TurnUsage {
            input_tokens: last.get("inputTokens").and_then(Value::as_u64),
            output_tokens: last.get("outputTokens").and_then(Value::as_u64),
            total_tokens: last.get("totalTokens").and_then(Value::as_u64),
            thought_tokens: last.get("reasoningOutputTokens").and_then(Value::as_u64),
        };
        if usage.input_tokens.is_some()
            || usage.output_tokens.is_some()
            || usage.total_tokens.is_some()
            || usage.thought_tokens.is_some()
        {
            self.usage = Some(usage);
        }
    }
}

#[cfg(test)]
pub(crate) fn events_from_envelope(value: &Value) -> Vec<SessionEvent> {
    CodexView::new(None).ingest(value)
}

/// Shared by the codex view and client tests to replay a measured wire file.
#[cfg(test)]
pub(crate) fn fixture_frames(source: &str) -> Vec<Value> {
    source
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|row| row.get("raw").and_then(Value::as_str).map(str::to_string))
        .filter_map(|raw| {
            let start = raw.find('{')?;
            serde_json::from_str(&raw[start..]).ok()
        })
        .collect()
}

fn delta_event<F>(
    params: &Value,
    _label: &str,
    id_field: &str,
    text_field: &str,
    build: F,
) -> Vec<SessionEvent>
where
    F: FnOnce(Option<String>, String) -> SessionEvent,
{
    let Some(text) = params.get(text_field).and_then(Value::as_str) else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
    vec![build(
        params
            .get(id_field)
            .and_then(Value::as_str)
            .map(str::to_string),
        text.to_string(),
    )]
}

fn tool_delta(params: &Value, kind: &str) -> Vec<SessionEvent> {
    let Some(text) = params.get("delta").and_then(Value::as_str) else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
    let Some(id) = params.get("itemId").and_then(Value::as_str) else {
        return Vec::new();
    };
    vec![SessionEvent::AgentToolUpdate {
        tool_call_id: id.to_string(),
        status: None,
        text: Some(text.to_string()),
        title: None,
        kind: Some(kind.to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    }]
}

fn item_event(
    item: Option<&Value>,
    completed: bool,
    cwd: Option<&std::path::Path>,
) -> Vec<SessionEvent> {
    let Some(item) = item else {
        return Vec::new();
    };
    let Some(id) = item.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    match item.get("type").and_then(Value::as_str) {
        Some("commandExecution") => {
            let kind = Some("execute".to_string());
            if completed {
                vec![SessionEvent::AgentToolUpdate {
                    tool_call_id: id.to_string(),
                    status: item.get("status").and_then(Value::as_str).map(status_name),
                    text: item
                        .get("aggregatedOutput")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string),
                    title: None,
                    kind,
                    locations: None,
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            } else {
                vec![SessionEvent::AgentToolCall {
                    tool_call_id: id.to_string(),
                    title: normalize_command_execution_command(
                        item.get("command").unwrap_or(&Value::Null),
                    )
                    .unwrap_or_else(|| "Command execution".to_string()),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(status_name)
                        .unwrap_or_else(|| "in_progress".to_string()),
                    kind,
                    locations: None,
                    subagent_type: None,
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            }
        }
        Some("fileChange") => {
            let locations = locations(item.get("changes"), cwd);
            if completed {
                vec![SessionEvent::AgentToolUpdate {
                    tool_call_id: id.to_string(),
                    status: item.get("status").and_then(Value::as_str).map(status_name),
                    text: None,
                    title: None,
                    kind: Some("edit".to_string()),
                    locations,
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            } else {
                vec![SessionEvent::AgentToolCall {
                    tool_call_id: id.to_string(),
                    title: first_change_path(item.get("changes"), cwd).unwrap_or_default(),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(status_name)
                        .unwrap_or_else(|| "in_progress".to_string()),
                    kind: Some("edit".to_string()),
                    locations,
                    subagent_type: None,
                    parent_tool_use_id: None,
                    spawn_depth: None,
                }]
            }
        }
        _ => Vec::new(),
    }
}

fn first_change_path(value: Option<&Value>, cwd: Option<&std::path::Path>) -> Option<String> {
    let changes = value?.as_array()?;
    let path = changes
        .iter()
        .find_map(|change| change.get("path")?.as_str())?;
    Some(relativize_tool_path(path, cwd))
}

fn locations(value: Option<&Value>, cwd: Option<&std::path::Path>) -> Option<Vec<ToolLocation>> {
    let changes = value?.as_array()?;
    let locations = changes
        .iter()
        .filter_map(|change| {
            let path = change.get("path").and_then(Value::as_str)?;
            Some(ToolLocation {
                path: relativize_tool_path(path, cwd),
                line: None,
            })
        })
        .collect::<Vec<_>>();
    (!locations.is_empty()).then_some(locations)
}

fn status_name(value: &str) -> String {
    match value {
        "inProgress" => "in_progress".to_string(),
        other => other.to_string(),
    }
}

fn turn_completed(params: &Value, usage: Option<TurnUsage>) -> Vec<SessionEvent> {
    let Some(turn) = params.get("turn") else {
        return Vec::new();
    };
    match turn.get("status").and_then(Value::as_str) {
        Some("completed") => vec![SessionEvent::AgentFinished {
            stop_reason: "completed".to_string(),
            model_id: turn
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage,
        }],
        Some("interrupted") => vec![SessionEvent::AgentFinished {
            stop_reason: "interrupted".to_string(),
            model_id: turn
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage,
        }],
        Some("failed") => vec![SessionEvent::SessionNotice {
            text: turn
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Codex turn failed")
                .to_string(),
            severity: NoticeSeverity::Warning,
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        catalog_from_response, events_from_envelope, fixture_frames, permission_request_event,
        CodexState, CodexView,
    };
    use devboule_protocol::SessionEvent;

    fn response_frame(source: &str, id: u64) -> serde_json::Value {
        fixture_frames(source)
            .into_iter()
            .find(|frame| frame.get("id").and_then(serde_json::Value::as_u64) == Some(id))
            .expect("measured frame")
    }

    #[test]
    fn measured_handshake_responses_build_the_codex_manifest() {
        let source = include_str!("../fixtures/wire/codex/E1-step1-handshake.jsonl");
        let models = response_frame(source, 2);
        let thread = response_frame(source, 3);
        let mut catalog = catalog_from_response(&models["result"]).expect("model/list catalog");
        catalog.apply_thread_response(&thread["result"]);
        let state = CodexState::new("thread-1".to_string(), catalog, "auto");
        state.set_context_window(258400);

        let SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(modes),
        } = state.manifest()
        else {
            panic!("Codex manifest");
        };
        assert_eq!(provider_id.as_deref(), Some("codex"));
        assert_eq!(current_model_id.as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(models.len(), 5);
        assert_eq!(models[0].efforts.as_ref().expect("efforts").len(), 6);
        let current = models
            .iter()
            .find(|model| model.model_id == "gpt-5.6-terra")
            .expect("current model");
        assert_eq!(current.current_effort.as_deref(), Some("xhigh"));
        assert_eq!(current.context_tokens, Some(258400));
        assert_eq!(modes.current_mode_id, "auto");
        assert_eq!(
            modes
                .available_modes
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>(),
            vec!["read-only", "auto", "auto-review", "full-access"]
        );
    }

    #[test]
    fn read_only_mode_uses_the_measured_read_only_presets() {
        assert_eq!(
            super::mode_values("read-only"),
            serde_json::json!({
                "approvalPolicy": "on-request",
                "sandboxPolicy": { "type": "readOnly" }
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert_eq!(
            super::thread_mode_values("read-only"),
            serde_json::json!({
                "approvalPolicy": "on-request",
                "sandbox": "read-only"
            })
            .as_object()
            .expect("object")
            .clone()
        );
        assert!(super::validate_mode("read-only").is_ok());
    }

    #[test]
    fn context_window_stays_with_the_model_that_reported_it() {
        let catalog = catalog_from_response(&serde_json::json!({
            "data": [
                { "id": "model-a", "displayName": "Model A", "isDefault": true },
                { "id": "model-b", "displayName": "Model B", "isDefault": false }
            ]
        }))
        .expect("catalog");
        let state = CodexState::new("thread".to_string(), catalog, "auto");
        state.set_context_window(258400);
        state
            .set_model(Some("model-b"), None)
            .expect("switch model");

        let SessionEvent::SessionManifest { models, .. } = state.manifest() else {
            panic!("Codex manifest");
        };
        assert_eq!(
            models
                .iter()
                .find(|model| model.model_id == "model-a")
                .and_then(|model| model.context_tokens),
            Some(258400)
        );
        assert_eq!(
            models
                .iter()
                .find(|model| model.model_id == "model-b")
                .and_then(|model| model.context_tokens),
            None
        );

        state.set_context_window(131072);
        let SessionEvent::SessionManifest { models, .. } = state.manifest() else {
            panic!("Codex manifest");
        };
        assert_eq!(
            models
                .iter()
                .find(|model| model.model_id == "model-b")
                .and_then(|model| model.context_tokens),
            Some(131072)
        );
    }

    #[test]
    fn schema_approval_params_publish_command_and_file_permissions() {
        let command = serde_json::json!({
            "itemId": "exec-1",
            "startedAtMs": 1,
            "threadId": "thread-1",
            "turnId": "turn-1",
            "command": "git status",
            "cwd": "C:\\work",
            "reason": "needs to inspect the repository"
        });
        let Some(SessionEvent::PermissionRequest {
            tool_call_id,
            command: Some(command_text),
            cwd: Some(cwd),
            options,
            ..
        }) = permission_request_event(&command, false)
        else {
            panic!("command permission");
        };
        assert_eq!(tool_call_id, "exec-1");
        assert_eq!(command_text, "git status");
        assert_eq!(cwd, "C:\\work");
        assert_eq!(options.len(), 2);

        let file = serde_json::json!({
            "itemId": "file-1",
            "startedAtMs": 1,
            "threadId": "thread-1",
            "turnId": "turn-1",
            "grantRoot": "C:\\work",
            "reason": "needs to edit a file"
        });
        assert!(matches!(
            permission_request_event(&file, true),
            Some(SessionEvent::PermissionRequest { command: None, .. })
        ));
    }

    #[test]
    fn measured_turn_notifications_are_translated_in_order() {
        let frames = fixture_frames(include_str!(
            "../fixtures/wire/codex/E1-step1-handshake.jsonl"
        ));
        let turn_id = frames
            .iter()
            .find_map(|frame| {
                let params = frame.get("params")?;
                (params.pointer("/item/type")?.as_str()? == "userMessage")
                    .then(|| params.get("turnId")?.as_str())
                    .flatten()
            })
            .expect("measured turn");
        let mut view = CodexView::new(None);
        let mut events = Vec::new();
        for frame in frames.iter().filter(|frame| {
            frame
                .pointer("/params/turnId")
                .and_then(|value| value.as_str())
                == Some(turn_id)
                || frame
                    .pointer("/params/turn/id")
                    .and_then(|value| value.as_str())
                    == Some(turn_id)
        }) {
            events.extend(view.ingest(frame));
        }
        assert_eq!(view.take_context_window_update(), Some(258400));
        let text = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "ok");
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentFinished { usage: Some(usage), .. }
                if usage.total_tokens == Some(21059)
        )));
    }

    fn parse(value: &str) -> serde_json::Value {
        serde_json::from_str(value).expect("json")
    }

    #[test]
    fn file_change_locations_and_failed_turn_are_preserved() {
        let start = parse(
            r#"{"jsonrpc":"2.0","method":"item/started","params":{"item":{"type":"fileChange","id":"f1","status":"inProgress","changes":[{"diff":"x","kind":"update","path":"src/lib.rs"}]},"startedAtMs":1,"threadId":"th","turnId":"tu"}}"#,
        );
        assert!(matches!(
            events_from_envelope(&start).as_slice(),
            [SessionEvent::AgentToolCall { title, locations: Some(locations), .. }]
                if title == "src/lib.rs" && locations[0].path == "src/lib.rs"
        ));
        let failed = parse(
            r#"{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"failed","error":{"message":"nope"}}}}"#,
        );
        assert_eq!(
            events_from_envelope(&failed),
            vec![SessionEvent::SessionNotice {
                text: "nope".to_string(),
                severity: devboule_protocol::NoticeSeverity::Warning,
            }]
        );
    }

    #[test]
    fn file_change_title_skips_a_pathless_first_change() {
        let mut view = CodexView::new(Some(std::path::PathBuf::from(r"C:\w")));
        let events = view.ingest(&parse(
            r#"{"jsonrpc":"2.0","method":"item/started","params":{"item":{"type":"fileChange","id":"f1","status":"inProgress","changes":[{"kind":"delete"},{"kind":"update","path":"C:\\w\\src\\lib.rs"}]},"startedAtMs":1,"threadId":"th","turnId":"tu"}}"#,
        ));
        let expected = std::path::PathBuf::from("src")
            .join("lib.rs")
            .to_string_lossy()
            .into_owned();
        match events.as_slice() {
            [SessionEvent::AgentToolCall {
                title, locations, ..
            }] => {
                assert_eq!(title, &expected);
                let locations = locations.as_ref().expect("locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(locations[0].path, expected);
            }
            other => panic!("expected fileChange tool call, got {other:?}"),
        }
    }

    #[test]
    fn interrupted_turn_is_a_non_error_finish() {
        let mut view = CodexView::new(None);
        assert_eq!(
            view.ingest(&parse(
                r#"{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"th","turn":{"id":"tu","status":"interrupted","items":[]}}}"#,
            )),
            vec![SessionEvent::AgentFinished {
                stop_reason: "interrupted".to_string(),
                model_id: None,
                usage: None,
            }]
        );
    }

    #[test]
    fn command_execution_title_unwraps_the_shell_wrapper() {
        // Measured wire: the full pwsh invocation must not become the row title.
        let mut view = CodexView::new(None);
        let events = view.ingest(&parse(
            r#"{"jsonrpc":"2.0","method":"item/started","params":{"item":{"type":"commandExecution","id":"exec-1","status":"inProgress","command":"\"C:\\Users\\gualt\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe\" -Command 'git status'"},"threadId":"th","turnId":"tu"}}"#,
        ));
        match events.as_slice() {
            [SessionEvent::AgentToolCall { title, kind, .. }] => {
                assert_eq!(title, "git status");
                assert_eq!(kind.as_deref(), Some("execute"));
            }
            other => panic!("expected commandExecution tool call, got {other:?}"),
        }
    }

    #[test]
    fn change_path_keeps_the_full_path_when_it_equals_the_cwd() {
        let cwd = std::path::Path::new(r"C:\w");
        assert_eq!(
            crate::tool_paths::relativize_tool_path(r"C:\w", Some(cwd)),
            r"C:\w"
        );
    }

    #[cfg(windows)]
    #[test]
    fn change_path_relativizes_despite_drive_letter_case() {
        let cwd = std::path::Path::new(r"C:\Work");
        assert_eq!(
            crate::tool_paths::relativize_tool_path(r"c:\Work\src\lib.rs", Some(cwd)),
            std::path::PathBuf::from("src")
                .join("lib.rs")
                .to_string_lossy()
                .into_owned()
        );
    }
}
