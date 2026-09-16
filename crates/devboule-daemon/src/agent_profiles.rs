//! Agent profiles: the ordered list a creation resolves, and the standing
//! instructions that travel with it.
//!
//! The daemon is the only writer. The app sends `AgentProfilesSet` over the
//! named pipe; the creation path reads the store **at the moment it resolves a
//! profile and at the moment it applies the standing instructions**, so an edit
//! takes effect on the next creation rather than at the next session, and no
//! session carries a copy of the rules it was started under. The store is one
//! JSON document beside the journal (`agent-profiles.json`), written the way
//! `tool_policy.rs` writes its file: a create-new temp file, a current-user-only
//! DACL on Windows applied to that temp before its first byte is written, then a
//! rename over the target.
//! A crash leaves either the old list or the new one, never half a file, and a
//! document that decides what an agent may do is never briefly readable by
//! another user.
//!
//! Each device owns its own file. A profile is a local decision about what this
//! machine may start, so paired devices do not inherit or propagate one
//! another's profiles: `peer_policy.rs` refuses both requests to either role.
//!
//! **What this file failing means.** A document that cannot be read, parsed or
//! admitted is quarantined exactly as a corrupt tool-policy file is, and the
//! store then holds an **empty list and empty standing instructions** — never
//! the last good document. The failure of this file has to mean "no agents may
//! be created and no standing instructions", because the alternative is an
//! agent created from a profile nobody can read any more, or instructions
//! applied that the human has since deleted.
//!
//! **What this store does not validate, and why.** The catalog answers which
//! providers exist, and that answer is used below (`catalog_provider_id`). It
//! publishes no per-provider list of models or modes at this commit — for modes
//! `peer_policy.rs` says so in its own words ("ACP modes are defined by the
//! agent at runtime. This function cannot answer for them"), and there is no
//! feature table anywhere in the daemon. A membership test written here would
//! therefore be a second list, free to drift from the provider's, and its
//! failure mode would be refusing a profile that names a model the provider
//! really offers. `model`, `mode_id`, `thinking_option_id` and the feature keys
//! are stored verbatim and bounded by shape; the provider refuses an unknown
//! mode or model itself when a creation asks it to spawn
//! (`claude_client.rs`, `codex_view.rs`, `pi_client.rs`, `session.rs`).

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use devboule_protocol::{AgentProfile, AgentProfilesDocument};

/// The file inside the runtime directory. Named, not hashed, so a support
/// session can read it.
pub(crate) const PROFILES_FILE: &str = "agent-profiles.json";

/// The largest profile file this daemon will read (1 MiB).
///
/// Checked against the file's metadata before it is read, so an oversized file
/// is refused rather than allocated. A real document is a few kilobytes: this
/// cap only has to sit far above one — [`MAX_PROFILES`] rows of 2 KiB notes and
/// 8 KiB of standing instructions are far under it — and far below what would
/// turn a hand-written file into a denial of service at startup.
pub(crate) const MAX_PROFILES_FILE_BYTES: u64 = 1024 * 1024;

/// The most profiles one document may hold.
pub(crate) const MAX_PROFILES: usize = 64;

/// The longest profile name, in characters, after trimming.
pub(crate) const MAX_PROFILE_NAME_CHARS: usize = 60;

/// The longest profile note, in bytes: the "when to use" sentence.
pub(crate) const MAX_PROFILE_NOTE_BYTES: usize = 2 * 1024;

/// The longest standing-instructions text, in bytes.
pub(crate) const MAX_STANDING_INSTRUCTIONS_BYTES: usize = 8 * 1024;

/// The longest profile id, in bytes.
pub(crate) const MAX_PROFILE_ID_BYTES: usize = 128;

/// The longest icon token, in bytes.
pub(crate) const MAX_PROFILE_ICON_BYTES: usize = 64;

/// The longest provider, model, mode or thinking-option id, in bytes.
pub(crate) const MAX_PROFILE_FIELD_BYTES: usize = 128;

/// The most feature keys one profile may carry.
pub(crate) const MAX_PROFILE_FEATURES: usize = 32;

/// The longest feature key, in bytes.
pub(crate) const MAX_FEATURE_KEY_BYTES: usize = 64;

/// The largest one feature value may serialize to, in bytes.
pub(crate) const MAX_FEATURE_VALUE_BYTES: usize = 1024;

/// The most tool names one profile's overlay may remove.
pub(crate) const MAX_TOOL_OVERLAY_NAMES: usize = 256;

/// The longest tool name a profile's overlay may name, in bytes.
pub(crate) const MAX_TOOL_OVERLAY_NAME_BYTES: usize = 128;

/// Why a profile document was refused.
#[derive(Debug)]
pub(crate) enum ProfilesError {
    /// The document itself is over a cap, names a provider the catalog does not
    /// publish, or repeats an id. Nothing was written and nothing changed: the
    /// caller has to be told, because retrying the same document will fail the
    /// same way.
    InvalidRequest(String),
    /// The file could not be replaced. The store kept its previous document, so
    /// a restart still reads what the caller was told was in force.
    Io(io::Error),
}

impl From<io::Error> for ProfilesError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for ProfilesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(reason) => formatter.write_str(reason),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

/// The stored document, in memory, with the file as its durable copy.
///
/// One mutex covers both the document and the write: two `AgentProfilesSet`
/// frames arriving together must not interleave their read-modify-write of the
/// whole file (the last writer would then silently drop the other's profile).
pub(crate) struct AgentProfilesStore {
    path: PathBuf,
    document: Mutex<AgentProfilesDocument>,
}

impl AgentProfilesStore {
    /// Read `runtime_dir/agent-profiles.json`.
    ///
    /// A missing file is the normal first run and is not an error. A file that
    /// cannot be read, parsed, or admitted — over a cap, naming a provider the
    /// catalog does not publish, holding one id twice, or not a profile
    /// document at all — also yields an empty document, but never silently and
    /// never destructively: the bad file is renamed aside first, best effort,
    /// and one line names it and the reason. A daemon that refused to start over
    /// a malformed settings file would lock the user out of the app that could
    /// fix it; a daemon that overwrote the file would erase the evidence of what
    /// it refused.
    ///
    /// Nothing is repaired on this path, and that is the deliberate difference
    /// from `ToolPolicyStore::load`: that store drops a row it cannot consult
    /// (a row for a provider with no MCP tools), because such a row could never
    /// have been reached. A profile has no equivalent — every row here is one a
    /// creation could have resolved — so a document that is wrong in any row is
    /// refused whole. Half a list of rules is not a smaller version of the
    /// rules; it is different rules, applied to an agent nobody chose.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(PROFILES_FILE);
        let document = match load_document(&path) {
            Ok(document) => document,
            Err(reason) => {
                match quarantine(&path) {
                    Some(kept) => eprintln!(
                        "agent profiles: {} is unusable ({reason}); moved to {} and starting with no profiles and no standing instructions",
                        path.display(),
                        kept.display()
                    ),
                    None => eprintln!(
                        "agent profiles: {} is unusable ({reason}); it could not be moved aside, starting with no profiles and no standing instructions",
                        path.display()
                    ),
                }
                AgentProfilesDocument::default()
            }
        };
        Self {
            path,
            document: Mutex::new(document),
        }
    }

    /// The whole document, read now.
    ///
    /// Deliberately a copy taken at the moment of use rather than a borrowed
    /// handle: the caller that resolves a profile must see the list as it stands
    /// when it resolves, not as it stood when something else last read it, which
    /// is the read cadence `tool_policy.rs` states for its own store ("the
    /// broker reads the store on every `tools/list` and on every `tools/call`,
    /// so a toggle takes effect on the next call rather than at the next
    /// session").
    pub(crate) fn document(&self) -> AgentProfilesDocument {
        self.document
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Replace the whole document and persist it before the in-memory copy
    /// moves. A failed write leaves both the file and the memory as they were,
    /// so the caller can report a real failure instead of a list the next
    /// restart would forget.
    ///
    /// The order of the profiles is the caller's and is preserved exactly;
    /// nothing here sorts, and nothing here keys a profile by its name.
    pub(crate) fn set(&self, document: AgentProfilesDocument) -> Result<(), ProfilesError> {
        let mut next = document;
        check_document(&mut next).map_err(ProfilesError::InvalidRequest)?;
        let mut held = self
            .document
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        write_document(&self.path, &next)?;
        *held = next;
        Ok(())
    }

    /// The file this store reads and writes, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// The document `path` holds as the store will keep it, or the reason the
/// store has to start empty.
fn load_document(path: &Path) -> Result<AgentProfilesDocument, String> {
    let mut document = read_document(path).map_err(|error| error.to_string())?;
    check_document(&mut document)?;
    Ok(document)
}

/// Parse the file, refusing one larger than the cap before it is read.
///
/// A missing file is an empty document, not an error: that is the first run.
///
/// An unknown field is a parse failure here, and the refusal names it: the wire
/// types carry `deny_unknown_fields`, so a hand-edited file cannot smuggle a
/// field the daemon would silently ignore — and neither can a frame.
fn read_document(path: &Path) -> io::Result<AgentProfilesDocument> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AgentProfilesDocument::default())
        }
        Err(error) => return Err(error),
    };
    if metadata.len() > MAX_PROFILES_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is {} bytes, over the {MAX_PROFILES_FILE_BYTES}-byte cap",
                path.display(),
                metadata.len()
            ),
        ));
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not an agent profile document: {error}",
                path.display()
            ),
        )
    })
}

/// The document as the store will hold it, or the reason it cannot be used.
///
/// Shared by `load` and `set`, so a document the store accepted from the app is
/// a document it would also accept from its file, and a document it refuses at
/// load is one it would have refused from the app with the same sentence.
///
/// Every sentence names what is wrong, and by value only where the value has
/// already been proven bounded — the rule `validate_display_name` states
/// (`messages.rs`): a string the caller sent arrives with nothing bounding it,
/// and echoing it would move a flood out of the frame and into an error the app
/// renders. A name is checked for length first and named by its position; a
/// provider, model, mode or tool name is checked for length first and only then
/// named back.
fn check_document(document: &mut AgentProfilesDocument) -> Result<(), String> {
    if document.profiles.len() > MAX_PROFILES {
        return Err(format!(
            "the document holds {} profiles, over the {MAX_PROFILES}-profile cap",
            document.profiles.len()
        ));
    }
    let standing = document.standing_instructions.len();
    if standing > MAX_STANDING_INSTRUCTIONS_BYTES {
        return Err(format!(
            "the standing instructions are {standing} bytes, over the {MAX_STANDING_INSTRUCTIONS_BYTES}-byte cap"
        ));
    }
    let mut ids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(document.profiles.len());
    // One snapshot for the whole document. The registry is swappable, so a
    // per-row read could validate row 1 against one catalogue and row 2
    // against another, accepting a pair no single catalogue ever published.
    let registry = crate::session::catalog_registry();
    for (index, profile) in document.profiles.iter_mut().enumerate() {
        check_profile(profile, index + 1, &mut ids, &registry)?;
    }
    Ok(())
}

/// Check and canonicalise one profile in place.
///
/// `position` is the profile's 1-based place in the list, which is how a
/// refusal points at its row without echoing text that has not been bounded
/// yet. `ids` is every id already admitted, in list order: a document that
/// names one id twice is refused, and a document that names one **name** twice
/// is not — nothing here looks a profile up by name.
fn check_profile(
    profile: &mut AgentProfile,
    position: usize,
    ids: &mut std::collections::HashSet<String>,
    registry: &crate::session::ProviderRegistry,
) -> Result<(), String> {
    let name = profile.name.trim();
    if name.is_empty() {
        return Err(format!(
            "profile {position} has an empty name; a profile name is 1 to {MAX_PROFILE_NAME_CHARS} characters"
        ));
    }
    let name_length = name.chars().count();
    if name_length > MAX_PROFILE_NAME_CHARS {
        return Err(format!(
            "profile {position} is named with {name_length} characters, over the {MAX_PROFILE_NAME_CHARS}-character cap"
        ));
    }
    profile.name = name.to_string();

    // The id is the daemon's, assigned when the caller has none: a profile that
    // arrives without one is new. A profile that arrives with one keeps it, so
    // a rename cannot move a running child onto a different profile, and the
    // list may hold two profiles sharing a name.
    let id = profile.id.trim().to_string();
    if id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    } else {
        if id.len() > MAX_PROFILE_ID_BYTES {
            return Err(format!(
                "profile {position} has a {}-byte id, over the {MAX_PROFILE_ID_BYTES}-byte cap",
                id.len()
            ));
        }
        profile.id = id.clone();
    }
    if !ids.insert(profile.id.clone()) {
        return Err(format!(
            "the id '{}' is used by more than one profile",
            profile.id
        ));
    }

    let note_bytes = profile.note.len();
    if note_bytes > MAX_PROFILE_NOTE_BYTES {
        return Err(format!(
            "the note of profile {position} is {note_bytes} bytes, over the {MAX_PROFILE_NOTE_BYTES}-byte cap"
        ));
    }

    if let Some(icon) = profile.icon.as_deref() {
        let icon = icon.trim();
        if icon.is_empty() {
            return Err(format!(
                "profile {position} has an empty icon; omit the field to say it has none"
            ));
        }
        if icon.len() > MAX_PROFILE_ICON_BYTES {
            return Err(format!(
                "the icon of profile {position} is {} bytes, over the {MAX_PROFILE_ICON_BYTES}-byte cap",
                icon.len()
            ));
        }
        profile.icon = Some(icon.to_string());
    }

    // The provider is the one field the catalog answers, and it is resolved to
    // the catalog's own spelling: a session registered as `CLAUDE` and a profile
    // stored as `claude` are one provider. The predicate is the catalog's, not a
    // second list — the same shape `tool_policy.rs` uses for its own gate.
    let provider = profile.provider.trim();
    if provider.is_empty() {
        return Err(format!("profile {position} names no provider"));
    }
    if provider.len() > MAX_PROFILE_FIELD_BYTES {
        return Err(format!(
            "the provider of profile {position} is {} bytes, over the {MAX_PROFILE_FIELD_BYTES}-byte cap",
            provider.len()
        ));
    }
    let Some(canonical) = crate::provider_catalog::catalog_provider_id_for(registry, provider)
    else {
        return Err(format!(
            "'{provider}' is not a provider the catalog publishes"
        ));
    };
    profile.provider = canonical;

    check_required_field(&mut profile.model, "model", position)?;
    check_required_field(&mut profile.mode_id, "mode id", position)?;
    // `thinkingOptionId` is optional, and an empty one is refused for the reason
    // `validate_display_name` gives for a name: omitting the field is how a
    // caller says "none", so a caller that sent `""` meant to say something it
    // did not manage to say.
    if let Some(thinking) = profile.thinking_option_id.as_deref() {
        let thinking = thinking.trim();
        if thinking.is_empty() {
            return Err(format!(
                "profile {position} has an empty thinking option id; omit the field to say it has none"
            ));
        }
        if thinking.len() > MAX_PROFILE_FIELD_BYTES {
            return Err(format!(
                "the thinking option id of profile {position} is {} bytes, over the {MAX_PROFILE_FIELD_BYTES}-byte cap",
                thinking.len()
            ));
        }
        profile.thinking_option_id = Some(thinking.to_string());
    }

    if profile.features.len() > MAX_PROFILE_FEATURES {
        return Err(format!(
            "profile {position} carries {} features, over the {MAX_PROFILE_FEATURES}-feature cap",
            profile.features.len()
        ));
    }
    for (key, value) in &profile.features {
        if key.trim().is_empty() {
            return Err(format!(
                "profile {position} carries a feature with an empty key"
            ));
        }
        if key.len() > MAX_FEATURE_KEY_BYTES {
            return Err(format!(
                "a feature key of profile {position} is {} bytes, over the {MAX_FEATURE_KEY_BYTES}-byte cap",
                key.len()
            ));
        }
        let encoded = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        if encoded.len() > MAX_FEATURE_VALUE_BYTES {
            return Err(format!(
                "the value of feature '{key}' on profile {position} serializes to {} bytes, over the {MAX_FEATURE_VALUE_BYTES}-byte cap",
                encoded.len()
            ));
        }
    }

    // An overlay can only ever remove tools, and it removes them from the
    // broker's own closed table: a name outside it can never match a tool a
    // session is served, so a typo here would be an overlay that silently does
    // nothing. The table is the catalog's, not a second list.
    if profile.tool_overlay.len() > MAX_TOOL_OVERLAY_NAMES {
        return Err(format!(
            "the tool overlay of profile {position} names {} tools, over the {MAX_TOOL_OVERLAY_NAMES}-tool cap",
            profile.tool_overlay.len()
        ));
    }
    for slot in profile.tool_overlay.iter_mut() {
        let tool = slot.trim();
        if tool.is_empty() {
            return Err(format!(
                "the tool overlay of profile {position} names an empty tool"
            ));
        }
        if tool.len() > MAX_TOOL_OVERLAY_NAME_BYTES {
            return Err(format!(
                "a tool name in the overlay of profile {position} is {} bytes, over the {MAX_TOOL_OVERLAY_NAME_BYTES}-byte cap",
                tool.len()
            ));
        }
        if !crate::provider_catalog::MCP_BROKER_TOOLS
            .iter()
            .any(|(name, _)| *name == tool)
        {
            return Err(format!(
                "'{tool}' in the overlay of profile {position} is not a tool the broker serves"
            ));
        }
        *slot = tool.to_string();
    }

    Ok(())
}

/// Length-check a required id field (`model`, `modeId`) and store its trimmed
/// value. Named by position until its length is known, then by nothing else:
/// these are the provider's vocabulary, and this store does not hold a second
/// copy of it to check membership against (see the module note).
fn check_required_field(value: &mut String, label: &str, position: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("profile {position} names no {label}"));
    }
    if trimmed.len() > MAX_PROFILE_FIELD_BYTES {
        return Err(format!(
            "the {label} of profile {position} is {} bytes, over the {MAX_PROFILE_FIELD_BYTES}-byte cap",
            trimmed.len()
        ));
    }
    *value = trimmed.to_string();
    Ok(())
}

/// The most quarantine destinations one failed load tries before it gives up
/// and leaves the file where it is.
const MAX_QUARANTINE_ATTEMPTS: u64 = 32;

/// Distinguishes the destinations of two quarantines inside one process, so two
/// files refused in the same millisecond cannot be offered the same name.
static QUARANTINE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Move a file the store refused aside, so the next write cannot erase it.
///
/// Best effort by design: this runs while the daemon is already reporting a
/// load failure, and a path that cannot be renamed is left exactly where it is
/// — the caller's line then says so, rather than claiming a move that did not
/// happen.
fn quarantine(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    quarantine_at(path, now_millis())
}

/// [`quarantine`] with the clock supplied.
///
/// The name is `agent-profiles.json.corrupt-<unix millis>-<8 hex nonce>`, and
/// the destination has to be free before it is used: `rename` replaces an
/// existing destination on Windows as well as on Unix, so a name that is
/// already taken would erase the evidence an earlier quarantine kept.
fn quarantine_at(path: &Path, millis: u128) -> Option<PathBuf> {
    for _ in 0..MAX_QUARANTINE_ATTEMPTS {
        let nonce = QUARANTINE_NONCE.fetch_add(1, Ordering::Relaxed);
        let kept = quarantine_name(path, millis, nonce);
        if kept.exists() {
            continue;
        }
        return std::fs::rename(path, &kept).ok().map(|()| kept);
    }
    None
}

/// The name one quarantine attempt keeps `path` under: `path`'s own directory,
/// the millisecond it was called at, and this process's nonce for that attempt.
/// One function, so the name a test occupies is the name the writer picks.
fn quarantine_name(path: &Path, millis: u128, nonce: u64) -> PathBuf {
    path.with_file_name(format!("{PROFILES_FILE}.corrupt-{millis}-{nonce:08x}"))
}

/// Unix time in milliseconds, or 0 before the epoch: the timestamp in a
/// quarantine name, not a duration anything measures.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// Serialize the whole document and replace the file with it.
///
/// The write is the tool policy's write (`tool_policy::write_policies`): a temp
/// file created with `create_new`, its DACL replaced with a current-user-only
/// one on Windows *before the first byte is written*, then `sync_all` and a
/// rename over the target. The order is the point: a profile that decides how an
/// agent runs is never on disk under a DACL weaker than the one it will carry.
fn write_document(path: &Path, document: &AgentProfilesDocument) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(document).map_err(io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "agent profile file has no parent directory",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension("tmp");
    let result = (|| {
        // The temp name is this writer's own. `create_new` below refuses to
        // follow a file already at it, and a run that died between the create
        // and the rename would otherwise leave a temp that no later write can
        // ever get past. Removing it removes the name, not whatever a symlink
        // at it points at.
        let _ = std::fs::remove_file(&temp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        // The document decides what a created agent may do, so the temp file
        // carries the same current-user-only DACL as the tool policy file —
        // applied here, after the create and before the first `write_all`.
        // `security.rs` owns that call, so neither writer can drift on what
        // "protected" means. Off Windows there is no DACL to set.
        #[cfg(windows)]
        crate::security::apply_current_user_dacl(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// The whole document is validated against **one** registry snapshot.
    /// The registry is swappable, so a per-row read could accept row 1
    /// against one catalogue and row 2 against another — a pair no single
    /// catalogue ever published, which is exactly the hole the ordering rule
    /// exists to close. Proven on a **local** registry the global cell never
    /// sees: the row below is live in the snapshot handed to `check_profile`
    /// and absent from the process-wide one, so a call that reached for the
    /// global catalogue instead would refuse it.
    #[test]
    fn a_document_is_validated_against_one_snapshot() {
        let rows = crate::user_providers::parse_providers_document(
            br#"{"snapshot-agent": {"extends": "acp", "command": ["/bin/snap"]}}"#,
            &crate::session::native_family_ids(),
        )
        .expect("a valid row");
        let local = crate::session::ProviderRegistry::catalog_default().with_user_rows(rows);

        assert!(
            crate::provider_catalog::catalog_provider_id("snapshot-agent").is_none(),
            "the row is NOT in the live catalogue: that is what makes this test mean something"
        );

        let mut named = profile("p-1", "On a user row");
        named.provider = "snapshot-agent".to_string();
        let mut ids = std::collections::HashSet::new();
        check_profile(&mut named, 1, &mut ids, &local)
            .expect("the snapshot handed in is the one consulted");
        assert_eq!(named.provider, "snapshot-agent");
    }

    fn profile(id: &str, name: &str) -> AgentProfile {
        AgentProfile {
            id: id.to_string(),
            name: name.to_string(),
            icon: None,
            note: String::new(),
            provider: "claude".to_string(),
            model: "claude-opus-4-6".to_string(),
            mode_id: "default".to_string(),
            thinking_option_id: None,
            features: serde_json::Map::new(),
            tool_overlay: Vec::new(),
            enabled_for_agents: false,
        }
    }

    fn document(profiles: Vec<AgentProfile>) -> AgentProfilesDocument {
        AgentProfilesDocument {
            profiles,
            standing_instructions: String::new(),
        }
    }

    fn names(document: &AgentProfilesDocument) -> Vec<&str> {
        document
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect()
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule-agent-profiles-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Every `<PROFILES_FILE>.corrupt-*` sibling in `dir`, sorted.
    fn quarantined(dir: &Path) -> Vec<PathBuf> {
        let prefix = format!("{PROFILES_FILE}.corrupt-");
        let mut kept = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("dir entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect::<Vec<_>>();
        kept.sort();
        kept
    }

    fn seed(dir: &Path, document: serde_json::Value) {
        std::fs::write(
            dir.join(PROFILES_FILE),
            serde_json::to_vec_pretty(&document).expect("json"),
        )
        .expect("seed");
    }

    #[test]
    fn the_store_round_trips_through_its_file_preserving_the_humans_order() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        assert!(
            store.document().profiles.is_empty(),
            "a missing file is a first run"
        );
        assert!(store.document().standing_instructions.is_empty());

        // Deliberately not in name or provider order: the order is the human's,
        // and the store has no opinion of its own.
        let mut sent = document(vec![
            profile("p-z", "Zebra"),
            profile("p-a", "Aardvark"),
            profile("p-m", "Mule"),
        ]);
        sent.standing_instructions = "Report your result in your final message.".to_string();
        store.set(sent.clone()).expect("set");

        let reopened = AgentProfilesStore::load(&dir).document();
        assert_eq!(names(&reopened), ["Zebra", "Aardvark", "Mule"]);
        assert_eq!(
            reopened, sent,
            "the whole document round-trips, nothing sorted"
        );

        // The file carries the same order, so a store that sorted in memory
        // would still be caught by reading the document a human edits.
        let text = std::fs::read_to_string(dir.join(PROFILES_FILE)).expect("profiles file");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
        let written: Vec<&str> = parsed["profiles"]
            .as_array()
            .expect("profiles array")
            .iter()
            .map(|row| row["name"].as_str().expect("name"))
            .collect();
        assert_eq!(written, ["Zebra", "Aardvark", "Mule"]);
        assert_eq!(parsed["standingInstructions"], sent.standing_instructions);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reason `id` exists at all: a human renames a profile, and a child
    /// already running must still be able to say what it was started from.
    #[test]
    fn renaming_a_profile_keeps_its_id() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        store
            .set(document(vec![profile("p-1", "Reviewer")]))
            .expect("first save");

        let mut renamed = profile("p-1", "Second opinion");
        renamed.note = "Use when the first answer has to be checked.".to_string();
        store.set(document(vec![renamed])).expect("rename");

        let saved = store.document();
        assert_eq!(saved.profiles.len(), 1, "a rename is not an add");
        assert_eq!(saved.profiles[0].id, "p-1", "the id survives the rename");
        assert_eq!(saved.profiles[0].name, "Second opinion");
        let text = std::fs::read_to_string(dir.join(PROFILES_FILE)).expect("profiles file");
        assert!(text.contains("\"p-1\""), "{text}");
        assert!(!text.contains("Reviewer"), "the old name is gone: {text}");
        // A child that recorded `p-1` still names the profile it was started
        // from, which is the whole point of keying on the id.
        assert_eq!(
            AgentProfilesStore::load(&dir).document().profiles[0].id,
            "p-1"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two profiles may share a name: nothing shadows anything, because nothing
    /// here looks a profile up by name. Two profiles may **not** share an id,
    /// and that refusal names the id.
    #[test]
    fn two_profiles_may_share_a_name_and_may_not_share_an_id() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        store
            .set(document(vec![
                profile("p-1", "Reviewer"),
                profile("p-2", "Reviewer"),
            ]))
            .expect("a shared name is not a collision");
        let saved = store.document();
        assert_eq!(names(&saved), ["Reviewer", "Reviewer"]);
        assert_eq!(saved.profiles[0].id, "p-1");
        assert_eq!(saved.profiles[1].id, "p-2");

        let error = store
            .set(document(vec![
                profile("p-1", "Reviewer"),
                profile("p-1", "Different name"),
            ]))
            .expect_err("one id twice must be refused");
        assert!(error.to_string().contains("p-1"), "{error}");
        // The refused document did not reach the memory or the file.
        assert_eq!(names(&store.document()), ["Reviewer", "Reviewer"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The daemon mints the id when the caller has none, and the id it minted
    /// is what the next read returns — so a profile created by the Settings
    /// form has an identity before it has ever been renamed.
    #[test]
    fn a_profile_that_arrives_without_an_id_is_given_one() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        store
            .set(document(vec![profile("", "Fresh")]))
            .expect("set");
        let minted = store.document().profiles[0].id.clone();
        assert!(!minted.is_empty(), "the daemon mints an id");
        assert_ne!(minted, "Fresh", "never the name");
        assert!(minted.len() <= MAX_PROFILE_ID_BYTES);
        assert_eq!(
            AgentProfilesStore::load(&dir).document().profiles[0].id,
            minted,
            "the minted id is what the file and the next read hold"
        );

        // A second profile minted in the same save gets its own id.
        store
            .set(document(vec![profile("", "One"), profile("", "Two")]))
            .expect("two fresh profiles");
        let saved = store.document();
        assert_ne!(saved.profiles[0].id, saved.profiles[1].id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The one membership the catalog really answers, and the store uses the
    /// catalog's own predicate for it — `catalog_provider_id`, the same door the
    /// preset cells resolve through, which resolves aliases and the catalog's
    /// own spelling.
    #[test]
    fn a_profile_naming_no_catalog_provider_is_refused_with_the_value() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        let mut unknown = profile("p-1", "Ghost");
        unknown.provider = "does-not-exist".to_string();
        let error = store
            .set(document(vec![unknown]))
            .expect_err("an unknown provider must be refused");
        assert!(error.to_string().contains("does-not-exist"), "{error}");
        assert!(
            crate::provider_catalog::catalog_provider_id("does-not-exist").is_none(),
            "the predicate under test is the catalog's, not a second list"
        );
        assert!(store.document().profiles.is_empty());
        assert!(
            !dir.join(PROFILES_FILE).exists(),
            "a refused set must not write a profile file"
        );

        // The catalog's own spelling is what is stored, so `CLAUDE` and
        // `claude-code` are one provider and a later lookup cannot miss the row
        // it admitted.
        let mut alias = profile("p-2", "By an alias");
        alias.provider = "claude-code".to_string();
        store.set(document(vec![alias])).expect("an alias resolves");
        assert_eq!(store.document().profiles[0].provider, "claude");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pass 2e step 3: a profile naming a live user provider is accepted and
    /// canonicalised through the same door as a built-in — the lookup now
    /// walks the registry snapshot's published ids, so what it accepts is
    /// exactly what can spawn. The row goes live through the real seam with
    /// the rows lock held across the test, and adds only an id no other test
    /// names; every built-in answer is unchanged while it is live.
    #[test]
    fn a_profile_naming_a_live_user_provider_is_accepted_like_a_builtin() {
        let rows = crate::user_providers::parse_providers_document(
            br#"{"profile-row-agent": {"extends": "acp", "command": ["/bin/prow"]}}"#,
            &crate::session::native_family_ids(),
        )
        .expect("a valid row");
        let gate = crate::user_providers::lock_rows_state();
        crate::session::apply_user_rows(rows);

        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        let mut named = profile("p-1", "On a user row");
        // Case differs from the row's own spelling: the catalog's walk is
        // case-insensitive, and what is stored is the row's own spelling.
        named.provider = "Profile-Row-Agent".to_string();
        store
            .set(document(vec![named]))
            .expect("a live user provider is a provider like a built-in");
        assert_eq!(store.document().profiles[0].provider, "profile-row-agent");

        // Back the rows out while still holding the lock, so the live
        // snapshot the rest of the suite sees is the builtins-only one.
        crate::session::apply_user_rows(std::collections::BTreeMap::new());
        drop(gate);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A name is 1..60 characters after trimming. The refusal names the row and
    /// the cap, never the name: it is a string the caller sent with nothing
    /// bounding it, and echoing it would move a flood into an error the app
    /// renders.
    #[test]
    fn a_name_outside_one_to_sixty_characters_is_refused() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        let error = store
            .set(document(vec![profile("p-1", "   ")]))
            .expect_err("an empty name must be refused");
        assert!(error.to_string().contains("profile 1"), "{error}");
        assert!(error.to_string().contains("empty name"), "{error}");

        let long = "n".repeat(MAX_PROFILE_NAME_CHARS + 1);
        let error = store
            .set(document(vec![profile("p-1", &long)]))
            .expect_err("a 61-character name must be refused");
        assert!(error.to_string().contains("61"), "{error}");
        assert!(error.to_string().contains("60"), "{error}");
        assert!(
            !error.to_string().contains(&long),
            "the refusal must not echo an unbounded name: {error}"
        );

        // The trimmed name is what is stored, and 60 characters is admitted.
        store
            .set(document(vec![profile("p-1", "  Sixty  ")]))
            .expect("a name is judged after trimming");
        assert_eq!(store.document().profiles[0].name, "Sixty");
        store
            .set(document(vec![profile(
                "p-1",
                &"n".repeat(MAX_PROFILE_NAME_CHARS),
            )]))
            .expect("exactly the cap is admitted");
        assert_eq!(
            store.document().profiles[0].name.len(),
            MAX_PROFILE_NAME_CHARS
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Over a cap is refused with the size named, never truncated: half a rule
    /// is a different rule.
    #[test]
    fn an_over_cap_note_or_standing_instructions_is_refused_with_its_size() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        assert_eq!(MAX_PROFILE_NOTE_BYTES, 2 * 1024);
        assert_eq!(MAX_STANDING_INSTRUCTIONS_BYTES, 8 * 1024);

        let mut long_note = profile("p-1", "Long");
        long_note.note = "n".repeat(MAX_PROFILE_NOTE_BYTES + 1);
        let error = store
            .set(document(vec![long_note]))
            .expect_err("a note over the cap must be refused");
        assert!(error.to_string().contains("2049"), "{error}");
        assert!(error.to_string().contains("2048"), "{error}");

        let mut long_standing = document(vec![profile("p-1", "Fine")]);
        long_standing.standing_instructions = "s".repeat(MAX_STANDING_INSTRUCTIONS_BYTES + 1);
        let error = store
            .set(long_standing)
            .expect_err("standing instructions over the cap must be refused");
        assert!(error.to_string().contains("8193"), "{error}");
        assert!(error.to_string().contains("8192"), "{error}");

        // Neither refusal wrote anything, and the document is still the empty
        // one: nothing was truncated to fit.
        assert!(store.document().profiles.is_empty());
        assert!(store.document().standing_instructions.is_empty());
        assert!(!dir.join(PROFILES_FILE).exists());

        // Exactly the cap is admitted, both halves, so the refusal above is
        // about the size and not about the door.
        let mut exact = document(vec![profile("p-1", "Exact")]);
        exact.profiles[0].note = "n".repeat(MAX_PROFILE_NOTE_BYTES);
        exact.standing_instructions = "s".repeat(MAX_STANDING_INSTRUCTIONS_BYTES);
        store.set(exact.clone()).expect("exactly the caps");
        assert_eq!(store.document(), exact);
        assert_eq!(
            AgentProfilesStore::load(&dir).document(),
            exact,
            "the largest admitted document round-trips"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The list cap, and an overlay that names a tool the broker does not serve
    /// — a name that could never match, so a typo would be an overlay that
    /// silently does nothing.
    #[test]
    fn an_over_cap_list_and_an_unknown_overlay_tool_are_refused() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        assert_eq!(MAX_PROFILES, 64);
        let too_many = (0..=MAX_PROFILES)
            .map(|index| profile(&format!("p-{index}"), &format!("Profile {index}")))
            .collect::<Vec<_>>();
        let error = store
            .set(document(too_many))
            .expect_err("a 65th profile must be refused");
        assert!(error.to_string().contains("65"), "{error}");
        assert!(error.to_string().contains("64"), "{error}");
        assert!(!dir.join(PROFILES_FILE).exists());

        let mut overlay = profile("p-1", "Design child");
        overlay.tool_overlay = vec!["devboule_create_agent".to_string()];
        store.set(document(vec![overlay])).expect("a served tool");
        assert_eq!(
            store.document().profiles[0].tool_overlay,
            ["devboule_create_agent"]
        );

        let mut typo = profile("p-1", "Design child");
        typo.tool_overlay = vec!["devboule_create_agentt".to_string()];
        let error = store
            .set(document(vec![typo]))
            .expect_err("a tool the broker does not serve must be refused");
        assert!(
            error.to_string().contains("devboule_create_agentt"),
            "{error}"
        );
        assert!(
            crate::provider_catalog::MCP_BROKER_TOOLS
                .iter()
                .all(|(name, _)| *name != "devboule_create_agentt"),
            "the predicate under test is the catalog's own tool table"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The feature map is bounded by shape, not by membership: this daemon
    /// publishes no feature table, so a key it does not know is stored (see the
    /// module note) while a key or value over its cap is refused.
    #[test]
    fn a_feature_key_or_value_over_its_cap_is_refused() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        let mut wide = profile("p-1", "Wide");
        for index in 0..=MAX_PROFILE_FEATURES {
            wide.features
                .insert(format!("key{index}"), serde_json::Value::Bool(true));
        }
        let error = store
            .set(document(vec![wide]))
            .expect_err("over the feature cap");
        assert!(error.to_string().contains("33"), "{error}");
        assert!(error.to_string().contains("32"), "{error}");

        let mut long_key = profile("p-1", "Long key");
        long_key.features.insert(
            "k".repeat(MAX_FEATURE_KEY_BYTES + 1),
            serde_json::Value::Bool(true),
        );
        let error = store
            .set(document(vec![long_key]))
            .expect_err("over the key cap");
        assert!(error.to_string().contains("65"), "{error}");

        let mut long_value = profile("p-1", "Long value");
        long_value.features.insert(
            "autoAccept".to_string(),
            serde_json::Value::String("v".repeat(MAX_FEATURE_VALUE_BYTES)),
        );
        let error = store
            .set(document(vec![long_value]))
            .expect_err("over the value cap");
        assert!(error.to_string().contains("1026"), "{error}");
        assert!(error.to_string().contains("autoAccept"), "{error}");

        // A key this daemon has never heard of is stored as written: there is no
        // feature table to check it against, and inventing one would refuse a
        // feature the provider really has.
        let mut unknown_but_shaped = profile("p-1", "Unknown feature");
        unknown_but_shaped
            .features
            .insert("someFutureFeature".to_string(), serde_json::json!(3));
        store
            .set(document(vec![unknown_but_shaped]))
            .expect("a bounded feature key is stored");
        assert_eq!(
            store.document().profiles[0].features["someFutureFeature"],
            serde_json::json!(3)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_path_is_inside_the_runtime_dir() {
        let dir = temp_dir();
        let store = AgentProfilesStore::load(&dir);
        assert_eq!(store.path(), dir.join(PROFILES_FILE).as_path());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The load-bearing failure mode: a file this store cannot read means **no
    /// profiles and no standing instructions**, never yesterday's rules. The
    /// file here held a full document before it was corrupted, so a store that
    /// kept the last good copy would still answer with a profile and with
    /// standing instructions.
    #[test]
    fn a_corrupt_file_is_quarantined_and_reads_as_no_profiles_and_no_standing_instructions() {
        let dir = temp_dir();
        let good = {
            let mut document = document(vec![profile("p-1", "Yesterday")]);
            document.standing_instructions = "Yesterday's rules.".to_string();
            document
        };
        seed(
            &dir,
            serde_json::to_value(&good).expect("the good document"),
        );

        // A daemon restarted on the good file reads it, so the corruption below
        // is what changes the answer.
        assert_eq!(AgentProfilesStore::load(&dir).document(), good);

        std::fs::write(dir.join(PROFILES_FILE), b"{ this is not json").expect("corrupt");

        let store = AgentProfilesStore::load(&dir);
        assert!(
            store.document().profiles.is_empty(),
            "a corrupt file means no profiles may be created: {:?}",
            store.document().profiles
        );
        assert!(
            store.document().standing_instructions.is_empty(),
            "and no standing instructions: {:?}",
            store.document().standing_instructions
        );

        // The load failed loudly and the bytes are still on disk under the name
        // the `eprintln!` names: a daemon that started empty is not a daemon
        // that threw the file away.
        assert!(!dir.join(PROFILES_FILE).exists());
        let kept = quarantined(&dir);
        assert_eq!(kept.len(), 1, "one quarantine file, got {kept:?}");
        assert!(
            kept[0]
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("agent-profiles.json.corrupt-")),
            "the quarantine name names the file it keeps: {kept:?}"
        );
        assert_eq!(
            std::fs::read(&kept[0]).expect("quarantined bytes"),
            b"{ this is not json"
        );

        // A store that could not be moved aside still answers empty, and says so
        // rather than claiming a move that did not happen.
        let stuck = temp_dir();
        std::fs::create_dir(stuck.join(PROFILES_FILE)).expect("squat the file name");
        let store = AgentProfilesStore::load(&stuck);
        assert!(store.document().profiles.is_empty());
        assert!(store.document().standing_instructions.is_empty());
        assert!(
            stuck.join(PROFILES_FILE).is_dir(),
            "the file stays where it is"
        );
        assert!(quarantined(&stuck).is_empty(), "nothing was quarantined");

        // And a successful set repairs the live file.
        let store = AgentProfilesStore::load(&dir);
        store
            .set(document(vec![profile("p-2", "Today")]))
            .expect("set");
        assert_eq!(names(&AgentProfilesStore::load(&dir).document()), ["Today"]);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&stuck);
    }

    /// The brief's own audit target: a profile file edited by hand to name
    /// something the daemon cannot use is quarantined and refused whole, never
    /// partially repaired and never guessed at.
    #[test]
    fn a_hand_edited_document_the_store_cannot_admit_is_quarantined() {
        // A provider the catalog does not publish.
        let dir = temp_dir();
        let mut unresolvable =
            serde_json::to_value(document(vec![profile("p-1", "Ghost")])).expect("the document");
        unresolvable["profiles"][0]["provider"] = serde_json::json!("not-a-provider");
        seed(&dir, unresolvable);
        let store = AgentProfilesStore::load(&dir);
        assert!(store.document().profiles.is_empty());
        assert_eq!(quarantined(&dir).len(), 1, "refused whole, not repaired");
        assert!(!dir.join(PROFILES_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);

        // An unknown field: the wire types deny them, so a document carrying one
        // is not a profile document at all.
        let dir = temp_dir();
        let mut with_unknown =
            serde_json::to_value(document(vec![profile("p-1", "Extra")])).expect("the document");
        with_unknown["profiles"][0]["mode"] = serde_json::json!("default");
        seed(&dir, with_unknown);
        let store = AgentProfilesStore::load(&dir);
        assert!(store.document().profiles.is_empty());
        assert_eq!(quarantined(&dir).len(), 1, "an unknown field is unreadable");
        let _ = std::fs::remove_dir_all(&dir);

        // One id twice in one document: a list in which two rows claim one
        // identity cannot be used as written.
        let dir = temp_dir();
        let doubled = serde_json::json!({
            "profiles": [
                { "id": "p-1", "name": "First", "provider": "claude",
                  "model": "m", "modeId": "default", "enabledForAgents": false },
                { "id": "p-1", "name": "Second", "provider": "pi",
                  "model": "m", "modeId": "ask", "enabledForAgents": false }
            ],
            "standingInstructions": ""
        });
        seed(&dir, doubled);
        let store = AgentProfilesStore::load(&dir);
        assert!(store.document().profiles.is_empty());
        assert_eq!(quarantined(&dir).len(), 1);
        assert!(
            crate::provider_catalog::catalog_provider_id("pi").is_some(),
            "the rows themselves are fine: it is the repeated id that is not"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_byte_cap_is_quarantined_before_it_is_read() {
        let dir = temp_dir();
        assert_eq!(MAX_PROFILES_FILE_BYTES, 1024 * 1024);
        // One byte over the cap, and not JSON either: the refusal has to come
        // from the metadata, so the daemon can never be made to allocate what
        // the file claims to hold.
        let oversized = vec![b' '; MAX_PROFILES_FILE_BYTES as usize + 1];
        std::fs::write(dir.join(PROFILES_FILE), &oversized).expect("seed");
        let store = AgentProfilesStore::load(&dir);
        assert!(store.document().profiles.is_empty());
        assert!(store.document().standing_instructions.is_empty());
        let kept = quarantined(&dir);
        assert_eq!(kept.len(), 1, "one quarantine file, got {kept:?}");
        assert_eq!(
            std::fs::metadata(&kept[0]).expect("metadata").len(),
            MAX_PROFILES_FILE_BYTES + 1,
            "the oversized file is moved, not truncated"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file's DACL and the temp-then-rename staging, asserted the way the
    /// tool policy's own test asserts them (`tool_policy.rs`,
    /// `the_policy_file_dacl_names_only_the_current_user`): the stale temp from a
    /// run that died mid-write is removed rather than written through, the
    /// target holds this write, and it carries the current-user-only DACL.
    #[cfg(windows)]
    #[test]
    fn the_profiles_file_dacl_names_only_the_current_user() {
        let dir = temp_dir();
        let stale = dir.join("agent-profiles.tmp");
        std::fs::write(&stale, b"stale temp from a run that died mid-write").expect("seed stale");
        let store = AgentProfilesStore::load(&dir);
        store
            .set(document(vec![profile("p-1", "Today")]))
            .expect("set");
        assert!(
            !stale.exists(),
            "the stale temp is removed, not written through"
        );
        let text = std::fs::read_to_string(dir.join(PROFILES_FILE)).expect("profiles file");
        let document: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            document["profiles"][0]["name"], "Today",
            "the target holds this write, not the stale temp's bytes"
        );
        assert!(
            !dir.join("agent-profiles.bak").exists(),
            "this writer never keeps a .bak"
        );
        let sddl =
            crate::security::dacl_sddl_for_path(&dir.join(PROFILES_FILE)).expect("profiles DACL");
        let sid = crate::security::current_user_sid().expect("sid");
        assert!(
            crate::security::dacl_is_current_user_only(&sddl, &sid),
            "the profile file DACL must name only the current user: {sddl}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
