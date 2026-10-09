//! Derive a UI view from an inbound Claude stream-json envelope.
//!
//! The envelope is the source of truth. This module never mutates it: callers
//! journal the original object and, separately, publish the derived view.
//!
//! This root file holds the `ClaudeView` state, its seams, the `ingest`
//! dispatch, and the system-frame handlers (`ingest_system`,
//! `ingest_compact_boundary`, `ingest_status`); the other frame handlers live
//! in the `claude_view_*` child modules, each beside its test suite.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use devboule_protocol::{
    AvailableCommandView, NoticeSeverity, SessionEvent, SessionModeStateView, SessionModel,
};
use serde_json::Value;

use crate::acp_tool_content::ToolContentMemory;
use crate::claude_task_state::ClaudeTaskState;
use rate_limit::rate_limit_plan_usage;
use tasks::spawn_depth;

#[path = "claude_view_commands.rs"]
mod commands;
#[path = "claude_view_messages.rs"]
mod messages;
#[path = "claude_view_mode.rs"]
mod mode;
#[path = "claude_view_rate_limit.rs"]
mod rate_limit;
#[path = "claude_view_replay.rs"]
mod replay;
#[path = "claude_view_result.rs"]
mod result;
#[path = "claude_view_stream.rs"]
mod stream;
#[path = "claude_view_tasks.rs"]
mod tasks;
#[path = "claude_view_tools.rs"]
mod tools;

pub(crate) use mode::{mode_state, reported_permission_mode, unattended_answer, DEFAULT_MODE};
pub(crate) use replay::{drive_replay, withheld_finish_marker, WITHHELD_FINISH_MARKER_TYPE};
pub(crate) use result::{is_interrupted_result, total_cost_from_result};

/// The streamed text of one content block. `kind` is the block type as the
/// stream declared it (`content_block_start`, or the delta flavour); a
/// final-envelope block finds its stream block by this declared type plus the
/// accumulated text being a prefix of the envelope text. `consumed` marks a
/// block whose confirming envelope block was answered: one stream block
/// satisfies at most one envelope block.
#[derive(Default)]
struct StreamedBlock {
    text: String,
    kind: Option<String>,
    consumed: bool,
}

/// Stateful mapper: stream-json emits `stream_event` deltas and then
/// consolidated `assistant` messages — in the field one envelope per content
/// block, each with a single-block `content` array. Track streamed text per
/// stream block so an envelope block is matched to the stream block it belongs
/// to by declared type and streamed prefix, and forwarded only as the
/// unstreamed remainder.
pub(crate) struct ClaudeView {
    streamed: HashMap<(Option<String>, u64), StreamedBlock>,
    current_message_ids: HashMap<Option<String>, String>,
    plan_tool_ids: HashSet<String>,
    current_model: Option<String>,
    last_manifest_model: Option<String>,
    current_mode: Option<String>,
    peer_session_id: Option<String>,
    cwd: Option<PathBuf>,
    /// This turn's `AskUserQuestion` call ids, so a granted result drops its
    /// echo text. Cleared at turn end: a late result reads as ordinary.
    question_tool_ids: HashSet<String>,
    /// `tool_use` ids with no matching `tool_result` yet, and the latest
    /// start among them. The reader holds the watchdog clock while the set
    /// is non-empty, but only inside the watch's grace measured from that
    /// start — a use never answered lapses back to ordinary silence instead
    /// of holding forever. Every assistant `tool_use` block counts, top-
    /// level and sidechain inner calls alike: while calls keep starting the
    /// clock is fresh from the lines anyway, and once everything goes quiet
    /// the grace runs from the last start. Cleared at turn end with the
    /// rest of the turn state.
    open_tools: HashSet<String>,
    last_tool_start: Option<Instant>,
    /// The last published command list, by full value: the initialize
    /// handshake's rich list and the init frame's bare names are two readings
    /// of one menu, so a repeat publishes nothing.
    published_commands: Option<Vec<AvailableCommandView>>,
    /// The session's plan checklist, fed by Claude's task tools. Per session
    /// and in the view, so live and replay derive the same snapshots.
    task_state: ClaudeTaskState,
    /// Set by a journalled withholding marker, replay-only: the next `result`
    /// envelope derives its usual events minus the `AgentFinished` the live
    /// pass suppressed. Live stdout never carries the marker.
    withheld_finish_pending: bool,
    /// The compaction latch: a `compact_boundary` announces once. The CLI
    /// repeats the boundary as a heartbeat for the one compaction it marks,
    /// so repeats stay silenced until a turn ends — only a `result` frame
    /// re-arms, because only a turn boundary proves a new compaction.
    compaction_announced: bool,
    /// The CLI process's running `total_cost_usd`, as the view knows it.
    /// The turn's cost is the delta against it; the latch is view state, so
    /// the first result after a daemon restart or a resume reports its whole
    /// running total as that turn's cost — which for a new process is the
    /// same number.
    cost_baseline: CostBaseline,
    /// ACP tool content already derived, for the replay road that runs ACP
    /// envelopes through this view (`claude_view_replay::drive_replay`).
    acp_tool_content: ToolContentMemory,
}

/// What the view knows about the running total its deltas are measured
/// against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CostBaseline {
    /// The total at the cursor, from the journal scan: `None` when no result
    /// sat at or before it, so the next result is the process's first.
    Known(Option<f64>),
    /// The scan failed (a busy journal's deadline among them): the delta
    /// cannot be verified, so the first result carries no cost, and its own
    /// total becomes the baseline every later delta needs.
    Unknown,
}

impl ClaudeView {
    pub(crate) fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            streamed: HashMap::new(),
            current_message_ids: HashMap::new(),
            plan_tool_ids: HashSet::new(),
            current_model: None,
            last_manifest_model: None,
            current_mode: None,
            peer_session_id: None,
            cwd,
            published_commands: None,
            question_tool_ids: HashSet::new(),
            open_tools: HashSet::new(),
            last_tool_start: None,
            task_state: ClaudeTaskState::default(),
            withheld_finish_pending: false,
            compaction_announced: false,
            cost_baseline: CostBaseline::Known(None),
            acp_tool_content: ToolContentMemory::default(),
        }
    }

    /// The checklist state, for the replay seam and the live restart seed:
    /// both rebuild one continuous machine from the same journalled
    /// envelopes instead of starting empty.
    pub(crate) fn snapshot_task_state(&self) -> ClaudeTaskState {
        self.task_state.clone()
    }

    pub(crate) fn restore_task_state(&mut self, state: ClaudeTaskState) {
        self.task_state = state;
    }

    /// The cost latch, for the replay seam: a pull resuming at a cursor
    /// inside the current generation seeds its fresh view with the running
    /// total the journal recorded before the cursor, so its first result
    /// costs the delta the live reader produced. Live and replay therefore
    /// agree on every supported attach whose lookback scan succeeded; an
    /// unreadable journal leaves the replayed turn without a cost rather
    /// than with a wrong one.
    pub(crate) fn restore_cost_baseline(&mut self, baseline: CostBaseline) {
        self.cost_baseline = baseline;
    }

    /// Whether the view still owes a task-tool result: the seed's gate for
    /// parsing result envelopes.
    pub(crate) fn has_pending_task_calls(&self) -> bool {
        self.task_state.has_pending_calls()
    }

    pub(crate) fn peer_session_id(&self) -> Option<&str> {
        self.peer_session_id.as_deref()
    }

    pub(crate) fn set_mode(&mut self, mode_id: &str) {
        self.current_mode = Some(mode_id.to_string());
    }

    /// The open set's latest tool start, if any call is still unanswered.
    /// The reader holds the watchdog clock while this is `Some`; the watch
    /// bounds the hold by its grace from the stamp.
    pub(crate) fn open_tool_since(&self) -> Option<Instant> {
        if self.open_tools.is_empty() {
            None
        } else {
            self.last_tool_start
        }
    }

    #[cfg(test)]
    pub(crate) fn current_mode_id(&self) -> Option<&str> {
        self.current_mode.as_deref()
    }

    /// Map one parsed envelope to zero or more view events. Unknown or
    /// journal-only frames yield an empty vec.
    pub(crate) fn ingest(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let frame_type = envelope.get("type").and_then(Value::as_str);
        // The withholding marker owns exactly the result row written after
        // it. Any other frame between the two means the journal being read
        // is not the journal the marker was written for — a hole dropped the
        // result — so the marker expires here rather than silence a later
        // turn's completion.
        if frame_type != Some(WITHHELD_FINISH_MARKER_TYPE) && frame_type != Some("result") {
            self.withheld_finish_pending = false;
        }
        // One compaction, one marker: repeats of the boundary — even with
        // unrelated frames between them — belong to the same compaction
        // until a turn ends. Only a `result` frame re-arms the latch.
        if frame_type == Some("result") {
            self.compaction_announced = false;
        }
        match frame_type {
            Some("system") => self.ingest_system(envelope),
            Some("stream_event") => self.ingest_stream_event(envelope),
            Some("assistant") => self.ingest_assistant(envelope),
            Some("user") => self.ingest_user(envelope),
            Some("result") => self.ingest_result(envelope),
            Some("control_response") => self.ingest_control_response(envelope),
            Some("rate_limit_event") => rate_limit_plan_usage(envelope).into_iter().collect(),
            Some(WITHHELD_FINISH_MARKER_TYPE) => {
                self.withheld_finish_pending = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn ingest_system(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let subtype = envelope.get("subtype").and_then(Value::as_str);
        if subtype != Some("init") {
            return match subtype {
                Some("status") => self.ingest_status(envelope),
                Some("task_started") => self.ingest_task_started(envelope),
                Some("task_notification") => self.ingest_task_notification(envelope),
                Some("background_tasks_changed") => self.ingest_background_tasks_changed(envelope),
                Some("compact_boundary") => self.ingest_compact_boundary(envelope),
                Some("task_updated") => {
                    // Claude sends a partial patch here; lifecycle bookends are
                    // the only complete task state this view can reconcile.
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }
        // A root init is a new CLI process, and the running `total_cost_usd`
        // starts at zero with it — the latch resets with the process. Not a
        // child's: a positive `spawn_depth` marks the child, and its
        // in-band init is no process boundary. (Absence and an explicit 0
        // are both root; no capture shows a root init carrying the key.)
        if !spawn_depth(envelope).is_some_and(|depth| depth > 0) {
            self.cost_baseline = CostBaseline::Known(Some(0.0));
        }
        if let Some(session_id) = envelope
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            // Another CLI session on the same view is a rebind: its
            // checklist starts empty. A repeated init for the same session
            // changes nothing.
            if self
                .peer_session_id
                .as_deref()
                .is_some_and(|peer| peer != session_id)
            {
                self.task_state.reset();
            }
            self.peer_session_id = Some(session_id.to_string());
        }
        let model = envelope
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .map(str::to_string);
        if let Some(mode) = reported_permission_mode(envelope) {
            self.current_mode = Some(mode.to_string());
        }
        if let Some(model) = model.clone() {
            self.current_model = Some(model);
        }
        self.last_manifest_model = model.clone();
        let models = match &model {
            Some(model) => vec![SessionModel {
                accepts_images: true,
                provider_id: None,
                model_id: model.clone(),
                name: model.clone(),
                provider: None,
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
            None => Vec::new(),
        };
        let mut events = vec![SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: model,
            models,
            modes: self.mode_state(),
            current_model_provider_id: None,
        }];
        if let Some(commands) = Self::slash_commands_from(envelope) {
            let merged = self.merge_with_published(commands);
            if self.note_published(&merged) {
                events.push(SessionEvent::AvailableCommands { commands: merged });
            }
        }
        events
    }

    /// The compaction marker, the same notice pi's `compaction_end` shows:
    /// the sentence names the trigger, and the latch in `ingest` keeps the
    /// CLI's repeated boundary frames to one announcement per compaction.
    /// The metadata key has three spellings in the wild; the first present
    /// one wins, and anything but an explicit `manual` trigger reads
    /// automatic — so a manual `/compact` under a camel key still says
    /// manual instead of confidently announcing the wrong trigger.
    fn ingest_compact_boundary(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        if self.compaction_announced {
            return Vec::new();
        }
        self.compaction_announced = true;
        let manual = ["compact_metadata", "compactMetadata", "compactionMetadata"]
            .iter()
            .filter_map(|key| envelope.get(key))
            .filter_map(|metadata| metadata.get("trigger").and_then(Value::as_str))
            .next()
            == Some("manual");
        vec![SessionEvent::SessionNotice {
            text: if manual {
                "Context manually compacted"
            } else {
                "Context automatically compacted"
            }
            .to_string(),
            severity: NoticeSeverity::Info,
        }]
    }

    fn ingest_status(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(mode) = reported_permission_mode(envelope) else {
            return Vec::new();
        };
        if self.current_mode.as_deref() == Some(mode) {
            return Vec::new();
        }
        self.current_mode = Some(mode.to_string());
        let model = self.last_manifest_model.clone();
        let models = model
            .as_ref()
            .map(|model_id| SessionModel {
                accepts_images: true,
                provider_id: None,
                model_id: model_id.clone(),
                name: model_id.clone(),
                provider: None,
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            })
            .into_iter()
            .collect();
        vec![SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: model,
            models,
            modes: self.mode_state(),
            current_model_provider_id: None,
        }]
    }

    fn mode_state(&self) -> Option<SessionModeStateView> {
        self.current_mode.as_deref().map(mode_state)
    }
}

#[cfg(test)]
#[path = "claude_view_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "claude_view_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "claude_view_notices_tests.rs"]
mod notices_tests;

#[cfg(test)]
#[path = "claude_view_websearch_tests.rs"]
mod websearch_tests;
