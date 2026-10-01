//! The slash-command menu: the initialize handshake's rich list, the init
//! frame's bare names, their merge and dedup, and the bounds both parses
//! share.

use std::collections::HashSet;

use devboule_protocol::{AvailableCommandView, SessionEvent};
use serde_json::Value;

use super::ClaudeView;

/// How many command names one list carries at most: the handshake's rich
/// entries and the init frame's bare names share it, and so does pi's reply.
const MAX_CLAUDE_COMMANDS: usize = 1000;
/// How many raw entries one list parse inspects at most: the reader-thread
/// bound pi's reply keeps too — the event cap counts accepted entries, this
/// one counts the scan.
const MAX_INSPECTED_COMMANDS: usize = 10_000;

impl ClaudeView {
    /// The initialize handshake's answer: the SDK's `supportedCommands()` is
    /// this response's `commands` array cached, so the menu fills before the
    /// first prompt instead of waiting for the init frame's names. Anything
    /// else shaped as a control response — mode answers, errors, older CLIs —
    /// carries no commands and yields nothing.
    pub(super) fn ingest_control_response(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(commands) = Self::commands_from_initialize(envelope) else {
            return Vec::new();
        };
        if !self.note_published(&commands) {
            return Vec::new();
        }
        vec![SessionEvent::AvailableCommands { commands }]
    }

    /// Record the published list; `false` when the menu already shows exactly
    /// this, so a repeat — the init frame echoing the handshake — publishes
    /// nothing, live and on replay alike.
    pub(super) fn note_published(&mut self, commands: &[AvailableCommandView]) -> bool {
        if self.published_commands.as_deref() == Some(commands) {
            return false;
        }
        self.published_commands = Some(commands.to_vec());
        true
    }

    /// The one published list from both readings: the handshake's entries
    /// first, then init names it never listed, appended bare in init order.
    /// Either reading alone republishes nothing; both derivations stay
    /// order-deterministic, live and on replay alike. No `/rewind`:
    /// synthesizing one needs a native rewind road to back it with, and none
    /// exists here — the menu shows what the CLI lists.
    pub(super) fn merge_with_published(
        &self,
        commands: Vec<AvailableCommandView>,
    ) -> Vec<AvailableCommandView> {
        let Some(published) = self.published_commands.as_ref() else {
            return commands;
        };
        let mut seen: HashSet<String> = published.iter().map(|known| known.name.clone()).collect();
        let mut merged = published.clone();
        for command in commands {
            if seen.contains(&command.name) {
                continue;
            }
            seen.insert(command.name.clone());
            merged.push(command);
        }
        merged
    }

    /// The handshake's `commands`: name, description and argument hint per
    /// entry (aliases and builtin have no menu field). The bound counts
    /// accepted entries, like the init's.
    fn commands_from_initialize(envelope: &Value) -> Option<Vec<AvailableCommandView>> {
        if envelope.get("type").and_then(Value::as_str) != Some("control_response") {
            return None;
        }
        let response = envelope.get("response")?;
        if response.get("subtype").and_then(Value::as_str) != Some("success") {
            return None;
        }
        let entries = response.get("response")?.get("commands")?.as_array()?;
        // The first row per name wins; later rows with a repeated name
        // never reach the menu.
        let mut seen: HashSet<&str> = HashSet::new();
        let mut commands = Vec::new();
        for entry in entries.iter().take(MAX_INSPECTED_COMMANDS) {
            if commands.len() >= MAX_CLAUDE_COMMANDS {
                break;
            }
            let Some(name) = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            // The daemon owns `/goal`: a provider-advertised rival never
            // reaches the menu.
            if crate::session::session_goal::is_reserved_goal_command(name) {
                continue;
            }
            if !seen.insert(name) {
                continue;
            }
            commands.push(AvailableCommandView {
                name: name.to_string(),
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                hint: entry
                    .get("argumentHint")
                    .and_then(Value::as_str)
                    .filter(|hint| !hint.is_empty())
                    .map(str::to_string),
            });
        }
        Some(commands)
    }

    /// `slash_commands` off the init frame: a flat array of names carrying
    /// no descriptions and no hints, so each publishes
    /// with the empty description `AvailableCommandView` requires. The field
    /// is cwd-dependent and not every build sends it; absent means no event,
    /// so the menu keeps whatever it had. `terminal_slash_commands` is a
    /// separate list the CLI answers itself and is deliberately not / published.
    pub(super) fn slash_commands_from(envelope: &Value) -> Option<Vec<AvailableCommandView>> {
        // Both bounds: at most this many raw entries inspected on the reader
        // thread, at most a thousand accepted names in the event (review
        // A5-2 #5) — the same split `pi_view` puts on a `get_commands` reply.
        let names = envelope.get("slash_commands")?.as_array()?;
        Some(
            names
                .iter()
                .take(MAX_INSPECTED_COMMANDS)
                .filter_map(|name| {
                    let name = name.as_str()?.trim();
                    // The daemon owns `/goal`: a provider-advertised rival
                    // never reaches the menu.
                    if name.is_empty()
                        || crate::session::session_goal::is_reserved_goal_command(name)
                    {
                        return None;
                    }
                    Some(AvailableCommandView {
                        name: name.to_string(),
                        description: String::new(),
                        hint: None,
                    })
                })
                .take(MAX_CLAUDE_COMMANDS)
                .collect(),
        )
    }
}

#[cfg(test)]
#[path = "claude_view_commands_tests.rs"]
mod tests;
