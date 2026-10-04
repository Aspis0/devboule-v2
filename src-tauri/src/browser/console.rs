//! What one tab's page has said on its console: the ring that keeps it and the
//! reading of it.
//!
//! Three events carry a page's voice and nothing else does: what it logged
//! (`Runtime.consoleAPICalled`), what it threw (`Runtime.exceptionThrown`) and
//! what the runtime itself recorded — a resource that failed, a script it
//! blocked (`Log.entryAdded`). Two of the three need their domain enabled
//! first, and both are enabled on the blank bootstrap before the page loads
//! anything, because an error thrown while a page is still loading is one of
//! the ones an agent most needs to read.
//!
//! The ring is memory and the answer is the only way out of it. No entry is
//! written to a log: a console is where tokens, form contents and mail
//! addresses go, and this app has no reason to keep them twice.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;

/// How many entries one tab keeps. The oldest goes first when a two-hundredth
/// arrives: what a page said a moment ago is worth more than what it said an
/// hour ago, and an agent that wants the rest takes `sinceMs`.
pub const RING: usize = 200;

/// How much of one entry's text is kept. A page that logs a whole object can
/// say anything at all, and 500 characters is enough to name the failure.
const TEXT_MAX: usize = 500;

/// The events that carry a page's voice. A page event is not one of these and
/// does not land in the ring.
pub const VOICE: [&str; 3] = [
    "Runtime.consoleAPICalled",
    "Runtime.exceptionThrown",
    "Log.entryAdded",
];

/// Whether an event is one the ring keeps, as opposed to one the settle and
/// the frame watcher read.
pub fn is_voice(event: &str) -> bool {
    VOICE.contains(&event)
}

/// One thing the page said, in the shape the contract names it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// `error`, `warning` or `info`. Everything that is not the first two is
    /// `info`, which `console_logs` only answers for `level: "all"`.
    pub level: String,
    pub text: String,
    /// Where in the runtime it came from — `network`, `javascript`,
    /// `security`, `storage` — and `exception` for a throw. Absent for
    /// something the page itself logged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The runtime's own clock, in milliseconds.
    pub time_ms: f64,
}

#[derive(Default)]
struct Ring {
    entries: VecDeque<Entry>,
    dropped: usize,
}

static RINGS: Mutex<Option<HashMap<String, Ring>>> = Mutex::new(None);

/// What a caller asked for: errors only, warnings and errors, or everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Error,
    Warning,
    All,
}

impl Wanted {
    /// The word as the caller writes it, or None for a word that is not one of
    /// the three — a refusal naming it rather than a silent default.
    pub fn parse(level: Option<&str>) -> Option<Self> {
        match level.unwrap_or("warning") {
            "error" => Some(Wanted::Error),
            "warning" => Some(Wanted::Warning),
            "all" => Some(Wanted::All),
            _ => None,
        }
    }

    fn keeps(self, entry: &Entry) -> bool {
        match self {
            Wanted::Error => entry.level == "error",
            Wanted::Warning => entry.level == "error" || entry.level == "warning",
            Wanted::All => true,
        }
    }
}

/// Put one event into the ring of the tab it came from.
///
/// A page that has never been watched has no ring, and one event is not worth
/// creating one for: `signal_for` at watch time is what opens it. An event
/// whose parameters are not the shape the protocol documents is dropped rather
/// than guessed at — a console that half-answers is worse than one that is
/// quiet about what it did not understand.
pub fn record(id: &str, event: &str, params: &str) {
    if !is_voice(event) {
        return;
    }
    let Ok(answered) = serde_json::from_str::<serde_json::Value>(params) else {
        return;
    };
    let Some(entry) = entry_of(event, &answered) else {
        return;
    };
    with(id, |ring| {
        if ring.entries.len() >= RING {
            ring.entries.pop_front();
            ring.dropped += 1;
        }
        ring.entries.push_back(entry);
    });
}

/// The entry one event is, read the way each of the three writes itself.
fn entry_of(event: &str, answered: &serde_json::Value) -> Option<Entry> {
    let time = || {
        answered
            .get("timestamp")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
    };
    match event {
        "Runtime.consoleAPICalled" => Some(Entry {
            level: level_of(kind(answered, "type")).to_owned(),
            text: clip(&arguments(answered)),
            source: None,
            time_ms: time(),
        }),
        "Runtime.exceptionThrown" => {
            let thrown = answered.get("exceptionDetails")?;
            Some(Entry {
                level: "error".to_owned(),
                text: clip(thrown_text(thrown)),
                source: Some("exception".to_owned()),
                time_ms: time(),
            })
        }
        _ => {
            let entry = answered.get("entry")?;
            let mut text = entry
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            // A failed resource arrives with an address and no words of its own.
            if text.is_empty() {
                text = "an entry with no message".to_owned();
            }
            Some(Entry {
                level: level_of(kind(entry, "level")).to_owned(),
                text: clip(&text),
                source: entry
                    .get("source")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                time_ms: entry
                    .get("timestamp")
                    .and_then(serde_json::Value::as_f64)
                    .or_else(|| {
                        answered
                            .get("timestamp")
                            .and_then(serde_json::Value::as_f64)
                    })
                    .unwrap_or(0.0),
            })
        }
    }
}

fn kind(answered: &serde_json::Value, field: &str) -> String {
    answered
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_lowercase()
}

/// The level the answer filters on. A console call that is not a warning or an
/// error is information, whatever the page called it.
fn level_of(kind: String) -> &'static str {
    match kind.as_str() {
        "error" | "assert" => "error",
        "warning" | "warn" => "warning",
        _ => "info",
    }
}

/// What a logged call said: its arguments, the way a console prints them.
fn arguments(answered: &serde_json::Value) -> String {
    let Some(args) = answered.get("args").and_then(serde_json::Value::as_array) else {
        return String::new();
    };
    args.iter().map(argument).collect::<Vec<_>>().join(" ")
}

/// One argument as a console prints it. A value it can show is the value — a
/// string without the quotes JSON puts on it — and an object is its class and
/// how much of it there is (`Array(3)`), which is what the runtime sends as
/// the description: a console prints the shape of an object, not its contents.
fn argument(arg: &serde_json::Value) -> String {
    if let Some(text) = arg.get("value").and_then(serde_json::Value::as_str) {
        return text.to_owned();
    }
    if let Some(number) = arg
        .get("value")
        .filter(|value| value.is_number() || value.is_boolean())
    {
        return number.to_string();
    }
    arg.get("description")
        .or_else(|| arg.get("unserializableValue"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| arg.to_string(), str::to_owned)
}

/// What a throw said: the exception's own description, which carries its first
/// line and its stack, and the runtime's `text` when there was no description.
fn thrown_text(thrown: &serde_json::Value) -> &str {
    thrown
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| thrown.get("text").and_then(serde_json::Value::as_str))
        .unwrap_or("the page threw something with no message")
}

/// Keep the first `TEXT_MAX` characters, at a character boundary.
fn clip(text: &str) -> String {
    if text.chars().count() <= TEXT_MAX {
        return text.to_owned();
    }
    text.chars().take(TEXT_MAX).collect()
}

/// The entries of one tab the caller asked for, oldest first, and how many the
/// ring threw away to make room.
///
/// `since_ms` is the last that many milliseconds of the page's own clock, read
/// against the newest entry there is. The runtime's clock is not this
/// process's, so an age is measured from the page's own last word rather than
/// from a clock the two never shared.
pub fn entries(id: &str, wanted: Wanted, since_ms: Option<f64>) -> (Vec<Entry>, usize) {
    let since = since_ms.map(|age| (newest(id) - age).max(0.0));
    with_read(id, |ring| {
        let kept: Vec<Entry> = ring
            .entries
            .iter()
            .filter(|entry| wanted.keeps(entry))
            .filter(|entry| since.is_none_or(|since| entry.time_ms >= since))
            .cloned()
            .collect();
        (kept, ring.dropped)
    })
}

fn newest(id: &str) -> f64 {
    with_read(id, |ring| {
        ring.entries
            .iter()
            .map(|entry| entry.time_ms)
            .fold(f64::MIN, f64::max)
    })
}

/// A new document clears the ring: what the page said about the page it left is
/// not what the page says now. A move within a document does not, which is
/// what `committed` and `within_document` are for.
pub fn clear(id: &str) {
    with(id, |ring| {
        ring.entries.clear();
        ring.dropped = 0;
    });
}

/// Drop a tab's ring with the tab.
pub fn forget(id: &str) {
    if let Some(rings) = RINGS.lock().expect("browser console poisoned").as_mut() {
        rings.remove(id);
    }
}

/// Open a tab's ring, as `cdp_events::watch` does for the counters it keeps.
pub fn open(id: &str) {
    with(id, |_| {});
}

fn with(id: &str, update: impl FnOnce(&mut Ring)) {
    let mut rings = RINGS.lock().expect("browser console poisoned");
    update(
        rings
            .get_or_insert_with(HashMap::new)
            .entry(id.to_owned())
            .or_default(),
    );
}

fn with_read<T>(id: &str, read: impl FnOnce(&Ring) -> T) -> T {
    let rings = RINGS.lock().expect("browser console poisoned");
    match rings.as_ref().and_then(|rings| rings.get(id)) {
        Some(ring) => read(ring),
        // A tab that was never watched has no ring, and an empty one answers
        // the same way: nothing said, nothing dropped.
        None => read(&Ring::default()),
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
