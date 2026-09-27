//! Per-provider on/off switches: which providers the daemon may start.
//!
//! The daemon is the only writer. The app sends `ProviderSetEnabled` over the
//! named pipe; every spawn, probe and vocabulary decision reads the store at
//! the moment it decides (the read-cadence rule `delegation_store.rs` states
//! for its switch), so turning a provider off stops the next start rather
//! than the next restart. The store is one JSON document beside the journal
//! (`provider-switches.json`) holding the ids switched **off** — absent
//! means on, which is also the first-run state, so the file only ever names
//! the exceptions. This is deliberately not a row on the tool-policy
//! document: that row gates the daemon's tools, this switch gates the
//! provider itself, and one door must never imply the other.
//!
//! A document that cannot be read or parsed is quarantined like a corrupt
//! tool-policy file, and the store then reads all-on. Unlike delegation
//! (whose failure must withhold autonomous answers), a provider switch is
//! availability config and all-on is the default the file's absence also
//! means; the quarantine is logged, so re-saving the switches in Settings
//! is discoverable rather than silent.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use devboule_protocol::{ErrorCode, WireError};

/// The file inside the runtime directory. Named, not hashed, so a support
/// session can read it.
pub(crate) const SWITCHES_FILE: &str = "provider-switches.json";

/// Why a switch write was refused.
#[derive(Debug)]
pub(crate) enum SwitchError {
    /// The request names no provider, an over-long one, or one the catalog
    /// does not publish. Nothing was written and nothing changed.
    InvalidRequest(String),
    /// The file could not be replaced. The store kept the switches it
    /// already had, so a restart still reads what the caller was told is
    /// in force.
    Io(io::Error),
}

impl From<io::Error> for SwitchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for SwitchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(reason) => formatter.write_str(reason),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

/// The stored document: the canonical ids switched off, sorted on write so
/// the file is deterministic. `deny_unknown_fields` refuses a document
/// carrying anything else, the way the delegation switch refuses a shape
/// it does not hold.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SwitchesDocument {
    disabled: Vec<String>,
}

/// The one plain sentence every spawn, probe and vocabulary refusal answers
/// with. The id is a canonical catalog spelling the daemon resolved
/// itself — bounded, never caller free text — so naming it tells the human
/// which switch to flip.
pub(crate) fn disabled_sentence(provider_id: &str) -> String {
    format!(
        "'{provider_id}' is turned off; turn it on in Settings > Providers to start new sessions with it."
    )
}

/// Refuse a start for a switched-off provider: canonicalise the id the way
/// the set door does, then read the store at this moment. Unknown ids —
/// nothing the set door could have switched off — read as on.
pub(crate) fn refuse_if_disabled(
    switches: &ProviderSwitchStore,
    provider_id: &str,
) -> Result<(), WireError> {
    let canonical = crate::provider_catalog::catalog_provider_id(provider_id)
        .unwrap_or_else(|| provider_id.to_string());
    if switches.is_enabled(&canonical) {
        Ok(())
    } else {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            disabled_sentence(&canonical),
        ))
    }
}

/// The switch, in memory, with the file as its durable copy.
///
/// One mutex covers the set and the write, the same shape as the tool-policy
/// and delegation stores: two writes arriving together must not interleave
/// their read-and-replace of the file.
pub(crate) struct ProviderSwitchStore {
    path: PathBuf,
    inner: Mutex<HashSet<String>>,
}

impl ProviderSwitchStore {
    /// Read `runtime_dir/provider-switches.json`.
    ///
    /// A missing file is the normal first run: every provider reads on. A
    /// file that cannot be read or parsed is quarantined — moved aside
    /// first, best effort, never overwritten — and the store reads all-on
    /// with one log line naming the file and the reason.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(SWITCHES_FILE);
        let disabled = match read_switches(&path) {
            Ok(disabled) => disabled,
            Err(reason) => {
                match quarantine(&path) {
                    Some(kept) => eprintln!(
                        "provider-switches: {} is unusable ({reason}); moved to {} and starting with every provider on",
                        path.display(),
                        kept.display()
                    ),
                    None => eprintln!(
                        "provider-switches: {} is unusable ({reason}); it could not be moved aside, starting with every provider on",
                        path.display()
                    ),
                }
                HashSet::new()
            }
        };
        Self {
            path,
            inner: Mutex::new(disabled),
        }
    }

    /// Whether `provider_id` — a canonical catalog spelling — may start.
    /// Absent means on: the file only names the exceptions.
    pub(crate) fn is_enabled(&self, provider_id: &str) -> bool {
        !self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(provider_id)
    }

    /// Switch one provider off or back on and persist before the in-memory
    /// copy moves. A failed write leaves both as they were. Unknown ids are
    /// refused: nothing the catalog does not publish can be switched.
    pub(crate) fn set(&self, provider_id: &str, enabled: bool) -> Result<(), SwitchError> {
        let id = provider_id.trim();
        if id.is_empty() {
            return Err(SwitchError::InvalidRequest(
                "a provider id is required".to_string(),
            ));
        }
        if id.len() > crate::tool_policy::MAX_POLICY_NAME_BYTES {
            return Err(SwitchError::InvalidRequest(format!(
                "the provider id is {} bytes, over the {}-byte cap",
                id.len(),
                crate::tool_policy::MAX_POLICY_NAME_BYTES
            )));
        }
        let canonical = crate::provider_catalog::catalog_provider_id(id).ok_or_else(|| {
            SwitchError::InvalidRequest(format!("'{id}' is not a provider the catalog publishes"))
        })?;
        let mut disabled = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = disabled.clone();
        if enabled {
            next.remove(canonical.as_str());
        } else {
            if next.len() >= crate::tool_policy::MAX_POLICY_ROWS
                && !next.contains(canonical.as_str())
            {
                return Err(SwitchError::InvalidRequest(format!(
                    "too many providers switched off; the limit is {}",
                    crate::tool_policy::MAX_POLICY_ROWS
                )));
            }
            next.insert(canonical);
        }
        write_switches(&self.path, &next)?;
        *disabled = next;
        Ok(())
    }
}

/// Read the file: the canonicalised known ids, dropping whatever the
/// catalog no longer publishes (an uninstalled provider's row cannot be
/// consulted, so keeping it would be a switch that does nothing).
fn read_switches(path: &Path) -> Result<HashSet<String>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > crate::tool_policy::MAX_POLICY_FILE_BYTES {
        return Err(format!(
            "{} is {} bytes, over the {}-byte cap",
            path.display(),
            metadata.len(),
            crate::tool_policy::MAX_POLICY_FILE_BYTES
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let document: SwitchesDocument = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "{} is not a provider-switch document: {error}",
            path.display()
        )
    })?;
    let mut admitted = HashSet::new();
    let mut dropped = 0;
    for id in document.disabled {
        match crate::provider_catalog::catalog_provider_id(id.trim()) {
            Some(canonical) => {
                admitted.insert(canonical);
            }
            None => dropped += 1,
        }
    }
    if dropped > 0 {
        eprintln!(
            "provider-switches: {} names {dropped} providers the catalog no longer publishes; dropped",
            path.display()
        );
    }
    Ok(admitted)
}

fn write_switches(path: &Path, disabled: &HashSet<String>) -> io::Result<()> {
    let mut sorted: Vec<&str> = disabled.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let bytes = serde_json::to_vec_pretty(&SwitchesDocument {
        disabled: sorted.into_iter().map(str::to_string).collect(),
    })
    .map_err(io::Error::other)?;
    crate::atomic::write_protected_bytes(path, &bytes)
}

/// Move a damaged file aside, best effort. The quarantine name carries the
/// reason it exists: a switches file no load could admit.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let kept = path.with_extension(format!(
        "quarantined-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0)
    ));
    std::fs::rename(path, &kept).ok().map(|()| kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        crate::test_dirs::test_temp_dir(&format!("devboule-switches-{tag}"))
    }

    #[test]
    fn an_absent_file_reads_every_provider_on() {
        let dir = temp_dir("absent");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled("claude"));
        assert!(store.is_enabled("grok"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_switch_round_trips_through_its_file_and_survives_a_reload() {
        let dir = temp_dir("round-trip");
        let store = ProviderSwitchStore::load(&dir);
        store.set("grok", false).expect("switch off");
        assert!(!store.is_enabled("grok"));
        assert!(store.is_enabled("claude"));
        // Aliases and case resolve to the catalog spelling on the way in.
        store.set("GROK", true).expect("switch back on");
        assert!(store.is_enabled("grok"));
        // A fresh load reads the durable copy, not the memory.
        let reopened = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled("grok"));
        assert!(reopened.is_enabled("grok"));
        store.set("qwen", false).expect("second switch off");
        let reread = ProviderSwitchStore::load(&dir);
        assert!(!reread.is_enabled("qwen"));
        assert!(reread.is_enabled("grok"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_provider_is_refused_and_changes_nothing() {
        let dir = temp_dir("unknown");
        let store = ProviderSwitchStore::load(&dir);
        let error = store
            .set("not-a-provider", false)
            .expect_err("unknown ids cannot be switched");
        assert!(
            error.to_string().contains("not-a-provider"),
            "the refusal names the id: {error}"
        );
        assert!(store.is_enabled("not-a-provider"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_file_quarantines_into_all_on() {
        let dir = temp_dir("damaged");
        std::fs::write(dir.join(SWITCHES_FILE), b"{nope").expect("seed damage");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled("grok"));
        assert!(
            std::fs::read_dir(&dir).expect("list").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains("quarantined")),
            "the damaged file is moved aside, not deleted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_disabled_sentence_names_the_switch_to_flip() {
        let sentence = disabled_sentence("grok");
        assert!(
            sentence.contains("grok") && sentence.contains("Providers"),
            "one plain sentence naming the provider and the place: {sentence}"
        );
    }
}
