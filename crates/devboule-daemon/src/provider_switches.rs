//! Per-provider on/off switches: which providers the daemon may start.
//!
//! The daemon is the only writer. The app sends `ProviderSetEnabled` over the
//! named pipe; every spawn, probe and vocabulary decision reads the store at
//! the moment it decides (the read-cadence rule `delegation_store.rs` states
//! for its switch), so turning a provider off stops the next start rather
//! than the next restart. The store is one JSON document beside the journal
//! (`provider-switches.json`) naming only the choices a person made: `disabled`
//! lists the ids switched **off**, `enabled` the ids switched **on**. An id in
//! neither list has no choice yet and takes its default: on when the provider
//! is installed and is not an on-demand npx wrapper, off otherwise. So a first
//! run starts with the detected CLIs on and everything else off, and a CLI
//! installed later is on by default the next time the catalogue is read. This
//! is deliberately not a row on the tool-policy document: that row gates the
//! daemon's tools, this switch gates the provider itself, and one door must
//! never imply the other.
//!
//! A start with no saved choice is refused unless the provider is declared in
//! the providers file (as the loader accepts it) or installed on PATH; the
//! default is read at the moment of the call. See [`ProviderSwitchStore::default_for`].
//!
//! A document that cannot be read or parsed is quarantined like a corrupt
//! tool-policy file, and the store then reads every provider at its default.
//! Unlike delegation (whose failure must withhold autonomous answers), a
//! provider switch is availability config; the quarantine is logged, so
//! re-saving the switches in Settings is discoverable rather than silent.

use std::collections::HashMap;
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

/// The stored document: the canonical ids a person switched off and on,
/// each list sorted on write so the file is deterministic. `deny_unknown_fields`
/// refuses a document carrying anything else, the way the delegation switch
/// refuses a shape it does not hold.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SwitchesDocument {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    enabled: Vec<String>,
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

/// The default a provider with no choice takes: on when it is installed here,
/// never for an on-demand npx wrapper (its row says nothing is installed for
/// it, so it starts off until a person switches it on).
pub(crate) fn default_on(agent: &crate::provider_catalog::InstalledAgent) -> bool {
    agent.installed && agent.origin != crate::provider_catalog::ProviderOrigin::NpxWrapper
}

/// Whether a start may use `provider_id` right now: the person's saved choice
/// when there is one, else the default. Canonicalises the id the way the set
/// door does and reads the store at this moment.
pub(crate) fn is_on_now(switches: &ProviderSwitchStore, provider_id: &str) -> bool {
    let canonical = crate::provider_catalog::catalog_provider_id(provider_id)
        .unwrap_or_else(|| provider_id.to_string());
    switches
        .choice(&canonical)
        .unwrap_or_else(|| switches.default_for(&canonical))
}

/// Refuse a start the person has not turned on. A provider with no choice is
/// refused unless it is installed here, so a create or resume naming an npx
/// wrapper on a clean install gets the same one plain sentence the switch gives.
pub(crate) fn refuse_unless_enabled(
    switches: &ProviderSwitchStore,
    provider_id: &str,
) -> Result<(), WireError> {
    if is_on_now(switches, provider_id) {
        return Ok(());
    }
    let canonical = crate::provider_catalog::catalog_provider_id(provider_id)
        .unwrap_or_else(|| provider_id.to_string());
    Err(WireError::new(
        ErrorCode::InvalidRequest,
        disabled_sentence(&canonical),
    ))
}

/// The switch, in memory, with the file as its durable copy. Each id maps to
/// the choice a person made: `true` switched on, `false` switched off.
///
/// One mutex covers the set and the write, the same shape as the tool-policy
/// and delegation stores: two writes arriving together must not interleave
/// their read-and-replace of the file.
pub(crate) struct ProviderSwitchStore {
    path: PathBuf,
    inner: Mutex<HashMap<String, bool>>,
}

impl ProviderSwitchStore {
    /// Read `runtime_dir/provider-switches.json`.
    ///
    /// A missing file is the normal first run: every provider has no choice
    /// and takes its default. A file that cannot be read or parsed is
    /// quarantined — moved aside first, best effort, never overwritten — and
    /// the store reads every provider at its default with one log line naming
    /// the file and the reason.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(SWITCHES_FILE);
        let choices = match read_switches(&path) {
            Ok(choices) => choices,
            Err(reason) => {
                match quarantine(&path) {
                    Some(kept) => eprintln!(
                        "provider-switches: {} is unusable ({reason}); moved to {} and starting with every provider at its default",
                        path.display(),
                        kept.display()
                    ),
                    None => eprintln!(
                        "provider-switches: {} is unusable ({reason}); it could not be moved aside, starting with every provider at its default",
                        path.display()
                    ),
                }
                HashMap::new()
            }
        };
        Self {
            path,
            inner: Mutex::new(choices),
        }
    }

    /// The person's saved choice for a canonical id; None when there is none.
    pub(crate) fn choice(&self, provider_id: &str) -> Option<bool> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(provider_id)
            .copied()
    }

    /// Whether `provider_id` — a canonical catalog spelling — is on: the
    /// person's choice when there is one, otherwise `installed_default`
    /// (installed and not an on-demand wrapper, decided by the caller).
    pub(crate) fn is_enabled(&self, provider_id: &str, installed_default: bool) -> bool {
        self.choice(provider_id).unwrap_or(installed_default)
    }

    /// [`Self::is_enabled`] for a catalog row, with the default decided once
    /// by [`default_on`].
    pub(crate) fn is_enabled_for(&self, agent: &crate::provider_catalog::InstalledAgent) -> bool {
        self.is_enabled(&agent.id, default_on(agent))
    }

    /// The default for a canonical id with no saved choice. A row the providers
    /// file declares is on (the person declared it). A CLI found on PATH is on
    /// when installed. Anything else — an npx wrapper, an id nothing holds — is
    /// off, the same rule `default_on` gives the catalog rows. Reads the file and
    /// the PATH now, so no shared registry snapshot can change the answer.
    pub(crate) fn default_for(&self, canonical: &str) -> bool {
        let runtime_dir = self.path.parent().unwrap_or(Path::new("."));
        if crate::user_providers::declared_row_ids(runtime_dir)
            .iter()
            .any(|id| id == canonical)
        {
            return true;
        }
        crate::provider_catalog::find_available(canonical).is_some_and(|agent| default_on(&agent))
    }

    /// Whether a person switched `provider_id` off. Gates ask this, not
    /// [`Self::is_enabled`]: an id with no choice is never refused here.
    pub(crate) fn is_off(&self, provider_id: &str) -> bool {
        self.choice(provider_id) == Some(false)
    }

    /// Switch one provider off or on and persist before the in-memory copy
    /// moves. A failed write leaves both as they were. Unknown ids are
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
        let mut choices = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = choices.clone();
        if next.len() >= crate::tool_policy::MAX_POLICY_ROWS && !next.contains_key(&canonical) {
            return Err(SwitchError::InvalidRequest(format!(
                "too many providers have a switch set; the limit is {}",
                crate::tool_policy::MAX_POLICY_ROWS
            )));
        }
        next.insert(canonical, enabled);
        write_switches(&self.path, &next)?;
        *choices = next;
        Ok(())
    }
}

/// Read the file, canonicalising built-in aliases while retaining ids that
/// are temporarily absent from the live catalog (including user rows).
///
/// A missing, 0-byte or whitespace-only file holds no choices (the first
/// run). A leading UTF-8 BOM is stripped, so a Notepad save loads as written.
/// When an id is named in both lists, the off choice wins.
fn read_switches(path: &Path) -> Result<HashMap<String, bool>, String> {
    let bytes =
        match crate::config_read::read_config_file(path, crate::tool_policy::MAX_POLICY_FILE_BYTES)
            .map_err(|error| error.to_string())?
        {
            // Absent and blank both hold no choices: a blank switches file
            // holds nothing to destroy.
            crate::config_read::ConfigFile::Absent | crate::config_read::ConfigFile::Blank => {
                return Ok(HashMap::new());
            }
            crate::config_read::ConfigFile::Present(bytes) => bytes,
        };
    let document: SwitchesDocument = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "{} is not a provider-switch document: {error}",
            path.display()
        )
    })?;
    let mut admitted: HashMap<String, bool> = HashMap::new();
    let mut discarded_id = false;
    for (ids, on) in [(document.enabled, true), (document.disabled, false)] {
        for id in ids {
            let id = id.trim();
            if id.is_empty() || id.len() > crate::tool_policy::MAX_POLICY_NAME_BYTES {
                discarded_id = true;
                continue;
            }
            let canonical =
                crate::provider_catalog::catalog_provider_id(id).unwrap_or_else(|| id.to_string());
            if !admitted.contains_key(&canonical)
                && admitted.len() >= crate::tool_policy::MAX_POLICY_ROWS
            {
                discarded_id = true;
                continue;
            }
            admitted.insert(canonical, on);
        }
    }
    if discarded_id {
        eprintln!("provider-switches: ignored invalid or excess provider ids");
    }
    Ok(admitted)
}

fn write_switches(path: &Path, choices: &HashMap<String, bool>) -> io::Result<()> {
    let mut disabled: Vec<String> = choices
        .iter()
        .filter(|(_, on)| !**on)
        .map(|(id, _)| id.clone())
        .collect();
    let mut enabled: Vec<String> = choices
        .iter()
        .filter(|(_, on)| **on)
        .map(|(id, _)| id.clone())
        .collect();
    disabled.sort_unstable();
    enabled.sort_unstable();
    let bytes = serde_json::to_vec_pretty(&SwitchesDocument { disabled, enabled })
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

/// The providers every test fixture switches on: the ids the daemon's own
/// tests start, and the one whose binary cannot exist, so its spawn fails
/// where a test expects it to. A test must not depend on which CLIs the machine
/// has installed, so each fixture states the person's choice instead.
#[cfg(test)]
pub(crate) const TEST_FIXTURE_PROVIDERS: &[&str] = &[
    "claude",
    "codex",
    "pi",
    "devboule-acp-stub",
    "devboule-absent-probe",
];

#[cfg(test)]
impl ProviderSwitchStore {
    /// Mark the fixture providers on, in memory only, unless a choice is
    /// already held: a test that switched one off (or loaded a file that did)
    /// keeps that choice.
    pub(crate) fn switch_on_for_test_fixtures(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        for id in TEST_FIXTURE_PROVIDERS {
            inner.entry((*id).to_string()).or_insert(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        crate::test_dirs::test_temp_dir(&format!("devboule-switches-{tag}"))
    }

    #[test]
    fn a_first_run_leaves_installed_providers_on_and_the_rest_off() {
        let dir = temp_dir("first-run");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled("claude", true), "installed starts on");
        assert!(!store.is_enabled("grok", false), "not installed starts off");
        assert!(!store.is_off("claude"), "no choice is not a refusal");
        assert!(!store.is_off("grok"), "no choice is not a refusal");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_switch_round_trips_through_its_file_and_survives_a_reload() {
        let dir = temp_dir("round-trip");
        let store = ProviderSwitchStore::load(&dir);
        store.set("grok", false).expect("switch off");
        assert!(store.is_off("grok"));
        assert!(!store.is_off("claude"));
        // Aliases and case resolve to the catalog spelling on the way in.
        store.set("GROK", true).expect("switch back on");
        assert!(store.is_enabled("grok", false));
        assert!(!store.is_off("grok"));
        // A fresh load reads the durable copy, not the memory.
        store.set("claude", false).expect("switch Claude off");
        let reopened = ProviderSwitchStore::load(&dir);
        assert!(
            reopened.is_off("claude"),
            "reload reads the persisted off state"
        );
        store.set("qwen", false).expect("second switch off");
        let reread = ProviderSwitchStore::load(&dir);
        assert!(reread.is_off("qwen"));
        assert!(
            reread.is_enabled("grok", false),
            "an explicit on survives a reload even when the provider is not installed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_provider_is_refused_and_changes_nothing() {
        let dir = temp_dir("unknown");
        let store = ProviderSwitchStore::load(&dir);
        store.set("grok", false).expect("the store is nonempty");
        let error = store
            .set("not-a-provider", false)
            .expect_err("unknown ids cannot be switched");
        assert!(
            error.to_string().contains("not-a-provider"),
            "the refusal names the id: {error}"
        );
        assert!(store.is_enabled("not-a-provider", true));
        assert!(
            store.is_off("grok"),
            "refusing an unknown id preserves existing switches"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disabled_user_provider_survives_a_load_without_a_live_catalog_row() {
        let dir = temp_dir("user-row-not-live");
        std::fs::write(
            dir.join(SWITCHES_FILE),
            br#"{"disabled":["user-provider.example"]}"#,
        )
        .expect("seed a previously saved user provider switch");
        let store = ProviderSwitchStore::load(&dir);
        assert!(
            store.is_off("user-provider.example"),
            "loading without the user row must preserve its saved off state"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_invalid_id_does_not_reset_other_disabled_providers() {
        let dir = temp_dir("invalid-row-keeps-valid-switches");
        std::fs::create_dir_all(&dir).expect("create runtime directory");
        std::fs::write(
            dir.join(SWITCHES_FILE),
            br#"{"disabled":["grok","","claude"]}"#,
        )
        .expect("seed one malformed id beside valid switches");

        let store = ProviderSwitchStore::load(&dir);

        assert!(store.is_off("grok"), "the saved OFF state must survive");
        assert!(
            store.is_off("claude"),
            "valid rows after the bad id survive"
        );
        assert!(
            !store.is_off("qwen") && store.is_enabled("qwen", true),
            "providers absent from the file keep their default"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_on_choice_in_the_file_is_honoured() {
        let dir = temp_dir("explicit-on");
        std::fs::create_dir_all(&dir).expect("create runtime directory");
        std::fs::write(dir.join(SWITCHES_FILE), br#"{"enabled":["grok"]}"#).expect("seed");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled("grok", false));
        assert!(!store.is_off("grok"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bom_prefixed_switches_file_keeps_its_off_state() {
        let dir = temp_dir("bom");
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend(br#"{"disabled":["grok"]}"#);
        std::fs::write(dir.join(SWITCHES_FILE), &bytes).expect("seed");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_off("grok"), "the off state survives the BOM");
        assert!(!store.is_off("claude"));
        assert!(dir.join(SWITCHES_FILE).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_or_whitespace_switches_file_holds_no_choices_and_is_left() {
        for (tag, bytes) in [
            ("empty", b"".as_slice()),
            ("whitespace", b"  \n ".as_slice()),
            ("bom-only", b"\xef\xbb\xbf".as_slice()),
        ] {
            let dir = temp_dir(tag);
            std::fs::write(dir.join(SWITCHES_FILE), bytes).expect("seed");
            let store = ProviderSwitchStore::load(&dir);
            assert!(!store.is_off("grok"), "{tag} is a first run");
            assert!(store.is_enabled("claude", true));
            assert!(dir.join(SWITCHES_FILE).is_file(), "{tag} is left alone");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_damaged_file_quarantines_into_every_provider_at_its_default() {
        let dir = temp_dir("damaged");
        std::fs::write(dir.join(SWITCHES_FILE), b"{nope").expect("seed damage");
        let store = ProviderSwitchStore::load(&dir);
        assert!(!store.is_off("grok"));
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

    fn catalog_row(
        installed: bool,
        origin: crate::provider_catalog::ProviderOrigin,
    ) -> crate::provider_catalog::InstalledAgent {
        crate::provider_catalog::InstalledAgent {
            id: "grok".to_string(),
            aliases: &[],
            installed,
            executable: PathBuf::from("grok"),
            prefix_args: Vec::new(),
            acp_command: None,
            stream_json_command: None,
            rpc_command: None,
            app_server_command: None,
            authentication: crate::provider_catalog::AuthenticationStatus::Unknown,
            origin,
            launch_args: None,
            pickable: None,
            installed_version: None,
            latest_version: None,
            install_channel: crate::provider_catalog::InstallChannel::Native,
            npm_package: None,
            tools: Vec::new(),
            spawn_path_env: None,
            launch_directory: None,
        }
    }

    #[test]
    fn the_default_is_on_for_an_installed_provider_and_off_for_the_rest() {
        use crate::provider_catalog::ProviderOrigin;
        assert!(default_on(&catalog_row(true, ProviderOrigin::UserBinary)));
        assert!(!default_on(&catalog_row(false, ProviderOrigin::UserBinary)));
        assert!(
            !default_on(&catalog_row(true, ProviderOrigin::NpxWrapper)),
            "an on-demand wrapper says nothing is installed for it, so it starts off"
        );
    }

    #[test]
    fn the_catalog_row_reads_its_default_when_no_choice_is_saved() {
        use crate::provider_catalog::ProviderOrigin;
        let dir = temp_dir("row-default");
        let store = ProviderSwitchStore::load(&dir);
        assert!(store.is_enabled_for(&catalog_row(true, ProviderOrigin::UserBinary)));
        assert!(!store.is_enabled_for(&catalog_row(false, ProviderOrigin::UserBinary)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_the_providers_file_declares_is_on_without_a_choice() {
        let dir = temp_dir("declared-row");
        std::fs::create_dir_all(&dir).expect("create runtime directory");
        std::fs::write(
            dir.join(crate::user_providers::PROVIDERS_FILE),
            br#"{"route-declared-fixture": {"extends": "acp", "command": ["/bin/x"]}}"#,
        )
        .expect("declare a row");
        let store = ProviderSwitchStore::load(&dir);
        assert!(
            store.default_for("route-declared-fixture"),
            "a person declared the row, so its default is on"
        );
        assert!(is_on_now(&store, "route-declared-fixture"));
        refuse_unless_enabled(&store, "route-declared-fixture")
            .expect("a declared row passes the gate");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_from_a_document_the_loader_rejects_are_not_declared() {
        // One bad row fails the whole document (the loader applies nothing from
        // it), so the good row beside it is not declared either.
        let dir = temp_dir("declared-invalid");
        std::fs::create_dir_all(&dir).expect("create runtime directory");
        std::fs::write(
            dir.join(crate::user_providers::PROVIDERS_FILE),
            br#"{"route-declared-fixture": {"extends": "acp", "command": ["/bin/x"]}, "bad id!": {"extends": "acp", "command": ["/bin/x"]}}"#,
        )
        .expect("declare two rows, one invalid");
        let store = ProviderSwitchStore::load(&dir);
        assert!(
            !store.default_for("route-declared-fixture"),
            "a document the loader refuses declares nothing"
        );
        assert!(!store.default_for("bad id!"));
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
