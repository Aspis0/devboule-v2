//! The Codex commands the app-server answers with a request of its own, the
//! shape a picked command takes as a turn input, and the answers still owed.
//!
//! Translated from
//! `paseo-src/packages/server/src/server/agent/providers/codex-app-server-agent.ts`:
//! `parseSlashCommandInput` :4986-5002, the out-of-band dispatch
//! `tryHandleOutOfBand` :4975-5010, `executeCompactCommand` :5012-5032,
//! `executeGoalSubcommand` :5034-5086, `parseGoalSubcommand` :195-206 and
//! `buildCommandPromptInput` :4028-4056.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use devboule_protocol::AvailableCommandView;
use serde_json::Value;

use super::codex_command_catalog::{self, CommandEntry, CommandOrigin};
use super::codex_prompt_expand::expand_prompt;

/// How many command answers may be outstanding. A request whose response never
/// arrives would otherwise grow the table for the life of the session; hitting
/// the cap costs a notice, never the request, which Codex has already been given.
const MAX_OWED_ANSWERS: usize = 32;

/// One `/goal` subcommand (`GoalSubcommand` :188-193).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Goal {
    Set {
        objective: String,
    },
    Pause,
    Resume,
    Clear,
    /// `/goal` with nothing after it: Paseo answers the usage line and sends no
    /// request at all (:5035-5037).
    Usage,
}

/// A command Codex answers with a request instead of a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Compact,
    Goal(Goal),
}

impl Command {
    /// The app-server request this command is, or `None` when it answers by
    /// itself — the `/goal` usage line, which Paseo returns before touching the
    /// client (:5035-5037).
    pub(crate) fn request(&self, thread_id: &str) -> Option<(&'static str, Value)> {
        let thread_id = Value::String(thread_id.to_string());
        let (method, params) = match self {
            Command::Compact => (
                "thread/compact/start",
                serde_json::json!({ "threadId": thread_id }),
            ),
            // Setting a goal activates it (:5055-5060).
            Command::Goal(Goal::Set { objective }) => (
                "thread/goal/set",
                serde_json::json!({
                    "threadId": thread_id,
                    "objective": objective,
                    "status": "active",
                }),
            ),
            // Pause and resume name the thread and the new status only; no
            // `objective` crosses on either (:5063-5074).
            Command::Goal(Goal::Pause) => (
                "thread/goal/set",
                serde_json::json!({ "threadId": thread_id, "status": "paused" }),
            ),
            Command::Goal(Goal::Resume) => (
                "thread/goal/set",
                serde_json::json!({ "threadId": thread_id, "status": "active" }),
            ),
            Command::Goal(Goal::Clear) => (
                "thread/goal/clear",
                serde_json::json!({ "threadId": thread_id }),
            ),
            Command::Goal(Goal::Usage) => return None,
        };
        Some((method, params))
    }

    /// The line the session shows for this command's outcome (:5021-5031,
    /// :5044-5086). `error` is Codex's own message, or the transport's answer
    /// for a request that never reached it.
    ///
    /// An accepted compaction has nothing to say: the app-server reports it
    /// with `thread/compacted`, which the view turns into its own notice.
    pub(crate) fn outcome(&self, error: Option<&str>) -> Option<String> {
        match (self, error) {
            (Command::Compact, None) => None,
            (Command::Compact, Some(message)) => {
                Some(format!("Failed to compact context: {message}"))
            }
            (Command::Goal(Goal::Usage), _) => {
                Some("Usage: /goal <objective>|pause|resume|clear".to_string())
            }
            (Command::Goal(Goal::Set { objective }), None) => {
                Some(format!("Goal set: {objective}"))
            }
            (Command::Goal(Goal::Pause), None) => Some("Goal paused.".to_string()),
            (Command::Goal(Goal::Resume), None) => Some("Goal resumed.".to_string()),
            (Command::Goal(Goal::Clear), None) => Some("Goal cleared.".to_string()),
            (Command::Goal(_), Some(message)) => Some(format!("Failed to update goal: {message}")),
        }
    }
}

/// What the answer to a command request produced.
pub(crate) enum Answer {
    /// Not one of this table's requests: the client's ordinary handling of the
    /// response — including its own error notice — applies.
    NotOurs,
    /// Ours, and this is the line to show. `None` is Paseo's silent acceptance
    /// of a compaction.
    Ours(Option<String>),
}

/// The commands of one Codex session: the list read at session start, whether
/// this binary has goals, and the answers a command is still owed.
pub(crate) struct CodexCommands {
    entries: Vec<CommandEntry>,
    codex_home: std::path::PathBuf,
    cwd: Option<std::path::PathBuf>,
    goals_enabled: bool,
    /// Command requests written but not yet answered, keyed by the JSON-RPC id
    /// the answer will name.
    ///
    /// Paseo awaits each request on the thread that ran the command
    /// (`executeGoalSubcommand` :5043-5050). The daemon's answers arrive on the
    /// session reader, which is the only thread that may publish an event, so
    /// the line owed travels with the id instead of being awaited: the writer
    /// registers it before the frame goes out, the reader takes it when the
    /// response lands.
    owed: Mutex<HashMap<String, Command>>,
}

impl CodexCommands {
    /// Read the surface once, off the session's start path — Paseo re-walks it
    /// on every prompt (`listCommands` through
    /// `resolveSlashCommandInvocation` :4017), which is the hot path this
    /// daemon keeps it off.
    pub(crate) fn new(codex_home: &Path, cwd: Option<&Path>, goals_enabled: bool) -> Self {
        Self {
            entries: codex_command_catalog::command_table(codex_home, cwd, goals_enabled),
            codex_home: codex_home.to_path_buf(),
            cwd: cwd.map(Path::to_path_buf),
            goals_enabled,
            owed: Mutex::new(HashMap::new()),
        }
    }

    /// The table as the menu sees it.
    pub(crate) fn views(&self) -> Vec<AvailableCommandView> {
        codex_command_catalog::views(&self.entries)
    }

    /// The command this prompt names, when it names one the app-server answers
    /// with a request of its own (`tryHandleOutOfBand` :4975-5010).
    ///
    /// `compact` is always a command. `goal` is one only when the version gate
    /// passed; an older binary then sees `/goal x` as the ordinary prompt it
    /// always was, which is Paseo's own answer when a slash name is not in
    /// `listCommands`: `resolveSlashCommandInvocation` (:4004-4021) hands
    /// `startTurn` the raw text and it goes out as a `turn/start`.
    pub(crate) fn command(&self, text: &str) -> Option<Command> {
        let (name, args) = parse_slash(text)?;
        match name {
            "compact" => Some(Command::Compact),
            "goal" if self.goals_enabled => Some(Command::Goal(parse_goal(args))),
            _ => None,
        }
    }

    /// Register the answer `id` owes this command. `false` when the table is
    /// full or the lock is gone: the request still goes out, only its notice is
    /// dropped.
    pub(crate) fn owe(&self, id: &str, command: &Command) -> bool {
        let Ok(mut owed) = self.owed.lock() else {
            return false;
        };
        if owed.len() >= MAX_OWED_ANSWERS {
            return false;
        }
        owed.insert(id.to_string(), command.clone());
        true
    }

    /// The frame never reached Codex, so no answer will ever come for it.
    pub(crate) fn forget(&self, id: &str) {
        if let Ok(mut owed) = self.owed.lock() {
            owed.remove(id);
        }
    }

    /// Take the answer owed to `id` and say what the session shows for it.
    pub(crate) fn answer(&self, id: &Value, error: Option<&str>) -> Answer {
        let Some(id) = id.as_str() else {
            return Answer::NotOurs;
        };
        let Some(command) = self.owed.lock().ok().and_then(|mut owed| owed.remove(id)) else {
            return Answer::NotOurs;
        };
        Answer::Ours(command.outcome(error))
    }

    /// Input for a picked command (`buildCommandPromptInput` :4028-4056).
    /// Custom prompts are expanded here because app-server text input does not
    /// expand them; skills carry the same skill and text blocks Paseo builds.
    pub(crate) fn prompt_input_checked(&self, text: &str) -> Result<Option<Value>, String> {
        let Some((name, args)) = parse_slash(text) else {
            return Ok(None);
        };
        // An out-of-band name never becomes a prompt. Paseo cannot reach a
        // prompt builder with one: its intercept runs first, and a prompt
        // carrying images is not a string, so `resolveSlashCommandInvocation`
        // answers `None` and the raw text goes out (:4009). That same case does
        // reach this daemon's writer, and leaving it alone is what keeps a
        // `/compact` with a picture attached from travelling as a `$compact`
        // prompt Codex has no command for.
        if self.command(text).is_some() {
            return Ok(None);
        }
        let entries = codex_command_catalog::command_table(
            &self.codex_home,
            self.cwd.as_deref(),
            self.goals_enabled,
        );
        let Some(entry) = entries.iter().find(|entry| entry.name == name) else {
            if self.entries.iter().any(|entry| entry.name == name) {
                return Err(format!("Codex command /{name} is no longer available."));
            }
            return Ok(None);
        };
        match (&entry.origin, args) {
            (CommandOrigin::Prompt { path }, args) => Ok(Some(serde_json::json!([
                { "type": "text", "text": expand_prompt(&prompt_body(path)?, args.unwrap_or_default()) }
            ]))),
            (CommandOrigin::Skill { path }, args) => {
                let text = match args {
                    Some(args) => format!("${} {}", entry.name, args),
                    None => format!("${}", entry.name),
                };
                Ok(Some(serde_json::json!([
                    { "type": "skill", "name": entry.name, "path": path },
                    { "type": "text", "text": text },
                ])))
            }
            _ => Ok(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn prompt_input(&self, text: &str) -> Option<Value> {
        self.prompt_input_checked(text).ok().flatten()
    }

    pub(crate) fn is_picked_command(&self, text: &str) -> bool {
        parse_slash(text).is_some_and(|(name, _)| {
            self.entries.iter().any(|entry| entry.name == name)
                || codex_command_catalog::command_table(
                    &self.codex_home,
                    self.cwd.as_deref(),
                    self.goals_enabled,
                )
                .iter()
                .any(|entry| entry.name == name)
        })
    }
}

/// The body of one custom prompt file, front matter removed
/// (`buildCommandPromptInput` :4034-4037 reads the file again at send time, so
/// an edited prompt takes effect without restarting the session). An unreadable
/// file answers `None`, which leaves the prompt as the text the human typed.
fn prompt_body(path: &Path) -> Result<String, String> {
    let content = codex_command_catalog::read_command_file(path)
        .ok_or_else(|| "Codex could not read the selected prompt file.".to_string())?;
    Ok(codex_command_catalog::front_matter(&content).1)
}

/// `parseSlashCommandInput` :4986-5002: a lone `/` is not a command, a name
/// holding a second `/` is not a command, and the arguments are the trimmed
/// remainder — so `/goal   fix the bug ` names `goal` with `fix the bug`.
pub(crate) fn parse_slash(text: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') || trimmed.len() < 2 {
        return None;
    }
    let without_prefix = &trimmed[1..];
    let boundary = without_prefix.find(char::is_whitespace);
    let name = match boundary {
        Some(index) => &without_prefix[..index],
        None => without_prefix,
    };
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let args = match boundary {
        Some(index) => without_prefix[index..].trim(),
        None => "",
    };
    Some((name, (!args.is_empty()).then_some(args)))
}

/// `parseGoalSubcommand` :195-206: the three words are matched
/// case-insensitively and anything else is an objective.
fn parse_goal(args: Option<&str>) -> Goal {
    let trimmed = args.unwrap_or_default().trim();
    if trimmed.is_empty() {
        return Goal::Usage;
    }
    match trimmed.to_lowercase().as_str() {
        "pause" => Goal::Pause,
        "resume" => Goal::Resume,
        "clear" => Goal::Clear,
        _ => Goal::Set {
            objective: trimmed.to_string(),
        },
    }
}

#[cfg(test)]
#[path = "codex_commands_tests.rs"]
mod tests;
