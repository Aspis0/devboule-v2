//! Per-provider tool policy: which of the daemon's MCP broker tools a
//! provider's sessions are served.
//!
//! The daemon is the only writer. The app sends `ToolPolicySet` over the
//! named pipe; the broker reads the store on every `tools/list` and on every
//! `tools/call`, so a toggle takes effect on the next call rather than at the
//! next session. The store is one JSON file beside the journal
//! (`tool-policies.json`), written the way the MCP config is: a create-new temp
//! file, a current-user-only DACL on Windows applied to that temp before its
//! first byte is written, then a rename over the target.
//! A crash leaves either the old policy or the new one, never half a file, and
//! a policy that decides what an agent may call is never briefly readable by
//! another user.
//!
//! Each device owns its own file. A tool toggle is a local decision about
//! what this machine hands to an agent, so paired devices do not inherit or
//! propagate one another's policies.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use devboule_protocol::ToolPolicyEntry;

/// The file inside the runtime directory. Named, not hashed, so a support
/// session can read it.
pub(crate) const POLICY_FILE: &str = "tool-policies.json";

/// The largest policy file this daemon will read (1 MiB).
///
/// Checked against the file's metadata before it is read, so an oversized file
/// is refused rather than allocated. A real policy is a few hundred bytes:
/// this cap only has to sit far above one and far below what would turn a
/// hand-written file into a denial of service at startup.
pub(crate) const MAX_POLICY_FILE_BYTES: u64 = 1024 * 1024;

/// The most provider rows one policy document may hold.
pub(crate) const MAX_POLICY_ROWS: usize = 64;

/// The most disabled-tool names one provider row may hold.
pub(crate) const MAX_DISABLED_TOOLS: usize = 256;

/// The longest provider id or tool name, in bytes.
pub(crate) const MAX_POLICY_NAME_BYTES: usize = 128;

/// Why a policy write was refused.
#[derive(Debug)]
pub(crate) enum PolicyError {
    /// The request itself is over a cap, or names a provider the daemon
    /// publishes no MCP tools for. Nothing was written and nothing changed:
    /// the caller has to be told, because retrying the same request will fail
    /// the same way.
    InvalidRequest(String),
    /// The file could not be replaced. The store kept its previous contents,
    /// so a restart still reads the policy the caller was told was in force.
    Io(io::Error),
}

impl From<io::Error> for PolicyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(reason) => formatter.write_str(reason),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

/// May `name` be served under `policy`?
///
/// Three answers, in this order:
///
/// - [`crate::provider_catalog::MCP_ROSTER_TOOL`] is always on. It reads only
///   the caller's own roster (the bearer is the identity, never a tool
///   argument), so an agent that cannot call it cannot be steered, and a
///   stored policy that names it in `disabled_tools` cannot brick a session.
/// - No policy at all — a provider nobody has negotiated, or a session
///   registered before its provider id was known — is enabled. The default is
///   the pre-policy behaviour, so an absent file changes nothing.
/// - `Some(false)` disables everything else; otherwise the per-tool deny list
///   decides.
pub(crate) fn is_tool_enabled(policy: Option<&ToolPolicyEntry>, name: &str) -> bool {
    // Two always-on names, for the same reason: the roster is the only way a
    // session can see who it may talk to, and the profile list is the only way
    // it can say what to run (`devboule_create_agent` names a profile and
    // nothing else). A policy that took either away would leave an agent that
    // cannot do its job and cannot say why.
    if name == crate::provider_catalog::MCP_ROSTER_TOOL
        || name == crate::provider_catalog::MCP_LIST_PROFILES_TOOL
    {
        return true;
    }
    let Some(policy) = policy else {
        return true;
    };
    match policy.enabled {
        Some(false) => false,
        _ => !policy
            .disabled_tools
            .iter()
            .any(|disabled| disabled == name),
    }
}

/// The stored policies, in memory, with the file as their durable copy.
///
/// One mutex covers both the map and the write: two `ToolPolicySet` frames
/// arriving together must not interleave their read-modify-write of the
/// whole file (the last writer would then silently drop the other's
/// provider).
pub(crate) struct ToolPolicyStore {
    path: PathBuf,
    policies: Mutex<HashMap<String, ToolPolicyEntry>>,
    /// True when the file existed but could not be parsed. The file is kept
    /// where it is and every restrictable tool is denied until the file is
    /// fixed or removed and the daemon restarts; see `get`.
    failed_closed: AtomicBool,
    /// Why the load failed, for the Status surface. `None` on a clean load.
    load_error: Mutex<Option<String>>,
}

impl ToolPolicyStore {
    /// Read `runtime_dir/tool-policies.json`.
    ///
    /// A missing file is the normal first run and is not an error: the store
    /// starts empty and every tool is enabled (the pre-policy behaviour
    /// `is_tool_enabled` states for `None`). A BOM-prefixed file is stripped
    /// before parsing, so a Notepad save loads as written.
    ///
    /// A file that exists but holds no JSON (0 bytes, whitespace, a lone BOM)
    /// is NOT a first run here: this store decides access, so a blank file is
    /// an unknown policy and fails closed like any other unreadable file.
    ///
    /// A file that exists but cannot be read, parsed, or admitted — blank,
    /// over a cap, or not a policy document at all — fails CLOSED, not open:
    /// the file is kept where it is (never quarantined aside), the store holds
    /// no rows, and `get` answers every known provider with a deny-all row.
    /// `set` refuses while failed-closed (writing would rebuild the document
    /// from the empty map and destroy the other providers' rows), so the
    /// repair is by hand or removal, then restart. The reason is kept in
    /// `load_error` for the Status surface and named in one line. A daemon
    /// that re-enabled every restriction over a malformed file would silently
    /// drop the user's access controls in one restart.
    ///
    /// A row no session can ever be gated by — a key that disagrees with its own
    /// `providerId`, or a provider id the catalog publishes no MCP tools for —
    /// is dropped instead of quarantining the document, and one line reports how
    /// many of each. Dropping is safe in both cases for the same reason: the row
    /// could not have been consulted the way it was written. A key that names a
    /// provider the catalog knows is admitted under the catalog's own spelling
    /// of that id, so `CLAUDE` and `claude` are one provider and one row.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(POLICY_FILE);
        let (policies, failed_closed, load_error) = match load_policies(&path) {
            Ok((policies, dropped)) => {
                if dropped.mismatched > 0 || dropped.unknown_provider > 0 {
                    eprintln!(
                        "tool policy: {} dropped {} mismatched row(s) (key and providerId did not name one provider, or two keys named one) and {} row(s) for provider ids with no MCP tools; starting without them",
                        path.display(),
                        dropped.mismatched,
                        dropped.unknown_provider
                    );
                }
                (policies, false, None)
            }
            Err(reason) => {
                eprintln!(
                    "tool policy: {} is unusable ({reason}); keeping the file and starting with every broker tool denied until it is fixed or reset",
                    path.display(),
                );
                (HashMap::new(), true, Some(reason))
            }
        };
        Self {
            path,
            policies: Mutex::new(policies),
            failed_closed: AtomicBool::new(failed_closed),
            load_error: Mutex::new(load_error),
        }
    }

    /// This provider's policy, or `None` when it has none.
    ///
    /// The id is resolved to the catalog's own spelling before the lookup, the
    /// same resolution `set` and `load` apply, so the store's keys and the ids a
    /// session carries are one name per provider: a session registered as
    /// `CLAUDE` reads the row a caller stored as `claude`.
    ///
    /// When the file failed to load the store holds no rows and answers every
    /// known provider with a deny-all row (`enabled: Some(false)`), so the
    /// broker's `is_tool_enabled` denies every restrictable tool while the two
    /// always-on names stay on. `None` (no provider id) still reads as no
    /// policy: it matches no row by construction, so no restriction is lost by
    /// serving it. Unknown ids likewise read as no policy: `set` refuses them
    /// and `load` drops them, so none could have been restricted.
    pub(crate) fn get(&self, provider_id: Option<&str>) -> Option<ToolPolicyEntry> {
        let provider_id = crate::provider_catalog::mcp_catalog_id(provider_id?)?;
        if self.failed_closed.load(Ordering::Relaxed) {
            return Some(ToolPolicyEntry {
                provider_id: provider_id.to_string(),
                enabled: Some(false),
                disabled_tools: Vec::new(),
            });
        }
        self.policies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(provider_id)
            .cloned()
    }

    /// Whether the store is serving deny-all after a failed load is read
    /// through `load_error().is_some()`: one inhabited field, not two that
    /// can disagree.
    ///
    /// Why the file failed to load, for the Status surface (`Status` carries
    /// it as `toolPolicyError`). `None` on a clean load.
    pub(crate) fn load_error(&self) -> Option<String> {
        self.load_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Every stored policy, ordered by provider id. `HashMap` iteration order
    /// is not stable, and the app compares this list, so it is sorted here
    /// rather than left to the caller.
    pub(crate) fn entries(&self) -> Vec<ToolPolicyEntry> {
        let mut entries = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));
        entries
    }

    /// Replace one provider's policy and persist the whole store before the
    /// in-memory copy moves. A failed write leaves both the file and the
    /// memory as they were, so the caller can report a real failure instead
    /// of a policy the next restart would forget.
    ///
    /// While failed-closed every write is refused before anything is read:
    /// `set` is a read-modify-write of the whole store and the store holds no
    /// rows, so persisting would replace the user's file with a partial
    /// document and lift the deny-all over destroyed rows. The file stays
    /// untouched; fix it or remove it, then restart.
    ///
    /// A request over a cap, or one naming a provider the daemon publishes no
    /// MCP tools for, is refused before anything is written: the first is a
    /// caller bug the daemon will not store, the second could never be
    /// consulted, because the broker looks a session's own catalog id up.
    ///
    /// An admitted id is stored under the catalog's own spelling of it, which
    /// C-1 made a rule rather than a nicety: the predicate below matches a
    /// provider id case-insensitively while every lookup is an exact key, so a
    /// row stored as the caller spelled it (`CLAUDE`) was admitted and then
    /// unreachable — `CLAUDE` and `claude` were two providers. `set`, `load` and
    /// `get` resolve through the same catalog, so one provider has one name.
    pub(crate) fn set(
        &self,
        provider_id: &str,
        enabled: Option<bool>,
        disabled_tools: Vec<String>,
    ) -> Result<(), PolicyError> {
        if self.failed_closed.load(Ordering::Relaxed) {
            return Err(PolicyError::InvalidRequest(
                "the tool policy file is unreadable; fix it or remove it, then restart".to_string(),
            ));
        }
        check_row(provider_id, &disabled_tools).map_err(PolicyError::InvalidRequest)?;
        let Some(canonical) = crate::provider_catalog::mcp_catalog_id(provider_id) else {
            return Err(PolicyError::InvalidRequest(format!(
                "'{provider_id}' is not a provider the daemon publishes MCP tools for"
            )));
        };
        let entry = ToolPolicyEntry {
            provider_id: canonical.to_string(),
            enabled,
            disabled_tools,
        };
        let mut policies = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !policies.contains_key(canonical) && policies.len() >= MAX_POLICY_ROWS {
            return Err(PolicyError::InvalidRequest(format!(
                "the store already holds its cap of {MAX_POLICY_ROWS} providers, so '{provider_id}' cannot be added"
            )));
        }
        let mut next: HashMap<String, ToolPolicyEntry> = (*policies).clone();
        next.insert(canonical.to_string(), entry);
        write_policies(&self.path, &next)?;
        *policies = next;
        Ok(())
    }

    /// The file this store reads and writes, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// The document `path` holds as the store will keep it, or the reason the
/// store has to start empty, plus what had to be dropped for it to be usable.
fn load_policies(path: &Path) -> Result<(HashMap<String, ToolPolicyEntry>, Dropped), String> {
    let document = read_policies(path).map_err(|error| error.to_string())?;
    admit(document)
}

/// What a loaded document had to give up, by reason.
///
/// Both counts are reported in one line by [`ToolPolicyStore::load`]; they are
/// separate numbers because they mean different things to whoever wrote the
/// file: a key that disagrees with its own `providerId` is a mistake in the
/// document, an id with no broker tools is a name nothing can use.
#[derive(Default)]
struct Dropped {
    /// Rows whose outer key and `providerId` disagreed, and second keys for a
    /// provider already admitted — a document that named one provider twice.
    mismatched: usize,
    /// Rows for a provider id the catalog publishes no MCP tools for.
    unknown_provider: usize,
}

/// Parse the file, refusing one larger than the cap before it is read.
///
/// A missing file is an empty document, not an error: that is the first run.
/// A file that exists but holds no JSON is NOT a first run here: this store
/// decides access, so a blank file is an unknown policy and fails closed. A
/// leading UTF-8 BOM is stripped before parsing, so a Notepad save loads as
/// written.
fn read_policies(path: &Path) -> io::Result<HashMap<String, ToolPolicyEntry>> {
    match crate::config_read::read_config_file(path, MAX_POLICY_FILE_BYTES)? {
        crate::config_read::ConfigFile::Absent => Ok(HashMap::new()),
        crate::config_read::ConfigFile::Blank => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} exists but holds no policy; a blank file is not a first run for access config",
                path.display()
            ),
        )),
        crate::config_read::ConfigFile::Present(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a tool policy document: {error}", path.display()),
                )
            })
        }
    }
}

/// The document as the store will hold it, or the reason it cannot be used,
/// plus what was dropped.
///
/// Caps are checked on the document as it was written, then a row is dropped
/// and counted when it cannot be consulted as written: its outer key does not
/// name the same provider as its own `providerId` — both sides resolved to the
/// catalog's own spelling first, so `CLAUDE` and `claude` are one provider and
/// not a mismatch — or its provider id is one the catalog publishes no MCP tools
/// for (the predicate `set` enforces too). Dropping is the right repair for both
/// rather than a quarantine: neither row could have been reached by the lookup
/// the broker performs — which is keyed by the session's own catalog id — so
/// removing them cannot invent a policy, and trusting the outer key instead
/// would hand one provider another provider's deny list.
///
/// Two keys that resolve to one provider are one row too: the first key in order
/// is admitted and the second is counted as mismatched. The iteration is sorted
/// for exactly that reason, so which row a document yields is a property of the
/// document and not of `HashMap`'s order.
fn admit(
    document: HashMap<String, ToolPolicyEntry>,
) -> Result<(HashMap<String, ToolPolicyEntry>, Dropped), String> {
    if document.len() > MAX_POLICY_ROWS {
        return Err(format!(
            "it holds {} provider rows, over the {MAX_POLICY_ROWS}-row cap",
            document.len()
        ));
    }
    let mut admitted = HashMap::with_capacity(document.len());
    let mut dropped = Dropped::default();
    let mut rows = document.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    for (provider_id, mut entry) in rows {
        let Some(canonical) = crate::provider_catalog::mcp_catalog_id(&provider_id) else {
            dropped.unknown_provider += 1;
            continue;
        };
        if crate::provider_catalog::mcp_catalog_id(&entry.provider_id) != Some(canonical)
            || admitted.contains_key(canonical)
        {
            dropped.mismatched += 1;
            continue;
        }
        check_row(canonical, &entry.disabled_tools)?;
        // An over-cap row fails the whole document rather than being dropped
        // like a mismatched one, and the asymmetry is deliberate: a dropped
        // row reads as no policy (enabled), so dropping a row that could have
        // restricted tools would silently re-enable them — the fail-open this
        // store exists to close. Mismatched and unknown rows could never be
        // consulted, so dropping them loses nothing; an over-cap row could.
        // The row keeps the catalog's spelling on both sides: the key is what
        // the broker looks the session's own id up with, and `providerId` is
        // what the app renders and what the next write persists.
        entry.provider_id = canonical.to_string();
        admitted.insert(canonical.to_string(), entry);
    }
    Ok((admitted, dropped))
}

/// Check one row against the caps a stored row must respect.
///
/// Shared by `load` and `set`, so a row the store accepted from the app is a
/// row it would also accept from its file, and a row it refuses at load is one
/// it would have refused from the app with the same sentence.
fn check_row(provider_id: &str, disabled_tools: &[String]) -> Result<(), String> {
    if provider_id.len() > MAX_POLICY_NAME_BYTES {
        return Err(format!(
            "the provider id is {} bytes, over the {MAX_POLICY_NAME_BYTES}-byte cap",
            provider_id.len()
        ));
    }
    if disabled_tools.len() > MAX_DISABLED_TOOLS {
        return Err(format!(
            "the row disables {} tools, over the {MAX_DISABLED_TOOLS}-tool cap",
            disabled_tools.len()
        ));
    }
    if let Some(long) = disabled_tools
        .iter()
        .find(|name| name.len() > MAX_POLICY_NAME_BYTES)
    {
        return Err(format!(
            "a disabled tool name is {} bytes, over the {MAX_POLICY_NAME_BYTES}-byte cap",
            long.len()
        ));
    }
    Ok(())
}

/// Serialize the whole store and replace the file with it.
///
/// One writer (P2): the bytes go through the shared protected-write primitive
/// in `crate::atomic` — the same create_new, owner-only mode, Windows DACL
/// before the first byte, sync and rename the MCP carriers use — so the order
/// a policy file carries cannot drift from the order a broker secret carries.
/// The order is the point: a policy that decides which tools an agent may call
/// is never on disk under a DACL weaker than the one it will carry.
fn write_policies(path: &Path, policies: &HashMap<String, ToolPolicyEntry>) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(policies).map_err(io::Error::other)?;
    crate::atomic::write_protected_bytes(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(provider_id: &str, enabled: Option<bool>, disabled: &[&str]) -> ToolPolicyEntry {
        ToolPolicyEntry {
            provider_id: provider_id.to_string(),
            enabled,
            disabled_tools: disabled.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    fn temp_dir() -> PathBuf {
        crate::test_dirs::test_temp_dir("devboule-tool-policy")
    }

    /// No `<POLICY_FILE>.corrupt-*` sibling may exist: this store never
    /// quarantines. A failed load keeps the file where it is.
    fn no_corrupt_siblings(dir: &Path) {
        let prefix = format!("{POLICY_FILE}.corrupt-");
        let mut kept = Vec::new();
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
            {
                kept.push(path);
            }
        }
        assert!(kept.is_empty(), "nothing is quarantined, got {kept:?}");
    }

    #[test]
    fn an_absent_or_enabled_policy_serves_every_tool() {
        assert!(is_tool_enabled(None, "devboule_list_agents"));
        assert!(is_tool_enabled(None, "some_future_tool"));
        assert!(is_tool_enabled(
            Some(&entry("claude", None, &[])),
            "some_future_tool"
        ));
        assert!(is_tool_enabled(
            Some(&entry("claude", Some(true), &[])),
            "some_future_tool"
        ));
    }

    #[test]
    fn disabled_globally_or_by_name_is_refused() {
        assert!(!is_tool_enabled(
            Some(&entry("claude", Some(false), &[])),
            "some_future_tool"
        ));
        let selective = entry("claude", Some(true), &["some_future_tool"]);
        assert!(!is_tool_enabled(Some(&selective), "some_future_tool"));
        assert!(is_tool_enabled(Some(&selective), "another_tool"));
    }

    #[test]
    fn the_roster_tool_is_always_on_even_when_a_policy_names_it() {
        let hostile = entry(
            "claude",
            Some(false),
            &[crate::provider_catalog::MCP_ROSTER_TOOL],
        );
        assert!(is_tool_enabled(
            Some(&hostile),
            crate::provider_catalog::MCP_ROSTER_TOOL
        ));
        // The same name is on the broker's catalog, so the two cannot drift.
        assert!(crate::provider_catalog::MCP_BROKER_TOOLS
            .iter()
            .any(|(name, _)| *name == crate::provider_catalog::MCP_ROSTER_TOOL));
    }

    #[test]
    fn the_store_round_trips_through_its_file() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty(), "a missing file is not an error");
        assert!(store.get(Some("claude")).is_none());

        store
            .set("claude", Some(false), vec!["some_tool".to_string()])
            .expect("set claude");
        store.set("grok", None, Vec::new()).expect("set grok");
        assert_eq!(store.entries().len(), 2);
        let listed = store
            .entries()
            .into_iter()
            .map(|policy| policy.provider_id)
            .collect::<Vec<_>>();
        assert_eq!(listed, ["claude", "grok"], "entries are ordered by id");

        let reopened = ToolPolicyStore::load(&dir);
        assert_eq!(
            reopened.get(Some("claude")),
            Some(entry("claude", Some(false), &["some_tool"]))
        );
        assert_eq!(reopened.get(Some("grok")), Some(entry("grok", None, &[])));
        assert!(reopened.get(None).is_none(), "no provider, no policy");

        // The wire names are the file names: a hand-edited file must be the
        // same document the app sends.
        let text = std::fs::read_to_string(dir.join(POLICY_FILE)).expect("policy file");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(parsed["claude"]["enabled"], false);
        assert_eq!(parsed["claude"]["disabledTools"][0], "some_tool");
        assert_eq!(parsed["claude"]["providerId"], "claude");
        assert_eq!(parsed["grok"]["enabled"], serde_json::Value::Null);
        assert!(parsed["grok"].get("disabledTools").is_none());
        // The write stages through a sibling temp file and removes it; no
        // `.bak` exists because this writer never keeps one.
        assert!(!dir.join("tool-policies.tmp").exists());
        assert!(!dir.join("tool-policies.bak").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setting_the_same_provider_twice_replaces_it() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        store.set("claude", Some(false), Vec::new()).expect("first");
        store
            .set("claude", Some(true), vec!["some_tool".to_string()])
            .expect("second");
        assert_eq!(store.entries().len(), 1);
        assert_eq!(
            store.get(Some("claude")),
            Some(entry("claude", Some(true), &["some_tool"]))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_fails_closed_and_is_kept() {
        let dir = temp_dir();
        std::fs::write(dir.join(POLICY_FILE), b"{ this is not json").expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert!(store.load_error().is_some(), "a corrupt file denies");
        assert!(
            store.load_error().is_some(),
            "the reason is kept for the Status surface"
        );

        // The file is kept where it is: a daemon that started denied is not
        // a daemon that threw the evidence away.
        assert_eq!(
            std::fs::read(dir.join(POLICY_FILE)).expect("kept bytes"),
            b"{ this is not json"
        );
        no_corrupt_siblings(&dir);

        // A restricted tool is denied, while the two always-on names stay on.
        let policy = store.get(Some("claude")).expect("deny-all row");
        assert!(!is_tool_enabled(Some(&policy), "some_future_tool"));
        assert!(is_tool_enabled(
            Some(&policy),
            crate::provider_catalog::MCP_ROSTER_TOOL
        ));
        assert!(is_tool_enabled(
            Some(&policy),
            crate::provider_catalog::MCP_LIST_PROFILES_TOOL
        ));
        // No provider id matches no row, so nothing is lost by serving it.
        assert!(store.get(None).is_none());

        // While failed-closed no write may replace the user's file: a `set`
        // is a read-modify-write of the whole store, and the store holds no
        // rows, so writing would destroy every other provider's policy and
        // lift the deny-all. The repair is by hand or removal, then restart.
        let before = std::fs::read(dir.join(POLICY_FILE)).expect("kept bytes");
        let error = store
            .set("claude", Some(false), Vec::new())
            .expect_err("a set while failed-closed must be refused");
        assert!(
            error.to_string().contains("unreadable"),
            "the refusal names the unreadable file: {error}"
        );
        assert_eq!(
            std::fs::read(dir.join(POLICY_FILE)).expect("kept bytes"),
            before,
            "the refused write leaves the file byte-identical"
        );
        assert!(
            store.load_error().is_some(),
            "still denied after the refusal"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_while_failed_closed_keeps_a_truncated_documents_rows() {
        let dir = temp_dir();
        // Truncated mid-row: recoverable-looking rows plus a syntax error.
        // Nothing in here parses, so no row may be treated as known.
        let truncated = br#"{"claude": {"providerId": "claude", "enabled": false, "disabledTools": ["some_tool"]}, "codex": {"providerId": "cod"#;
        std::fs::write(dir.join(POLICY_FILE), truncated).expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.load_error().is_some());

        let before = std::fs::read(dir.join(POLICY_FILE)).expect("bytes");
        store
            .set("claude", Some(true), Vec::new())
            .expect_err("the truncated file must not be rewritten from an empty map");
        assert_eq!(
            std::fs::read(dir.join(POLICY_FILE)).expect("bytes"),
            before,
            "codex's row survives the refused write byte for byte"
        );
        // And codex stays denied: the refusal lifted nothing.
        let policy = store.get(Some("codex")).expect("deny-all row");
        assert!(!is_tool_enabled(Some(&policy), "some_future_tool"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_byte_cap_fails_closed_before_it_is_read() {
        let dir = temp_dir();
        assert_eq!(MAX_POLICY_FILE_BYTES, 1024 * 1024);
        // One byte over the cap, and not JSON either: the refusal has to come
        // from the metadata, so the daemon can never be made to allocate what
        // the file claims to hold.
        let oversized = vec![b' '; MAX_POLICY_FILE_BYTES as usize + 1];
        std::fs::write(dir.join(POLICY_FILE), &oversized).expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert!(store.load_error().is_some(), "over the cap denies");
        assert_eq!(
            std::fs::metadata(dir.join(POLICY_FILE))
                .expect("metadata")
                .len(),
            MAX_POLICY_FILE_BYTES + 1,
            "the oversized file is kept, not truncated"
        );
        no_corrupt_siblings(&dir);

        // A document under the cap is read normally, so the refusal above is
        // about the size of the file and not about the quarantine path itself.
        let mut document: HashMap<String, ToolPolicyEntry> = HashMap::new();
        document.insert("claude".to_string(), entry("claude", Some(false), &[]));
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec(&document).expect("json"),
        )
        .expect("seed small");
        assert_eq!(ToolPolicyStore::load(&dir).entries().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_row_cap_fails_closed() {
        let dir = temp_dir();
        let mut seeded: HashMap<String, ToolPolicyEntry> = HashMap::new();
        for index in 0..=MAX_POLICY_ROWS {
            let provider_id = format!("provider-{index:02}");
            seeded.insert(provider_id.clone(), entry(&provider_id, Some(true), &[]));
        }
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec_pretty(&seeded).expect("json"),
        )
        .expect("seed");

        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert!(store.load_error().is_some(), "over the row cap denies");
        assert!(dir.join(POLICY_FILE).is_file(), "the file is kept");
        no_corrupt_siblings(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_over_a_cap_or_naming_no_mcp_provider_is_refused_without_writing() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        assert_eq!(MAX_POLICY_ROWS, 64);
        assert_eq!(MAX_DISABLED_TOOLS, 256);
        assert_eq!(MAX_POLICY_NAME_BYTES, 128);

        let error = store
            .set(
                "claude",
                Some(true),
                vec!["some_tool".to_string(); MAX_DISABLED_TOOLS + 1],
            )
            .expect_err("a row over the tool cap must be refused");
        assert!(error.to_string().contains("256"), "{error}");

        let error = store
            .set(
                "claude",
                Some(true),
                vec!["t".repeat(MAX_POLICY_NAME_BYTES + 1)],
            )
            .expect_err("a tool name over the byte cap must be refused");
        assert!(error.to_string().contains("128"), "{error}");

        let error = store
            .set(
                &"p".repeat(MAX_POLICY_NAME_BYTES + 1),
                Some(true),
                Vec::new(),
            )
            .expect_err("a provider id over the byte cap must be refused");
        assert!(error.to_string().contains("128"), "{error}");

        // C-1: the gate is keyed by catalog id, so nothing else can be stored.
        let error = store
            .set("does-not-exist", Some(true), Vec::new())
            .expect_err("an id with no tools to gate must be refused");
        assert!(error.to_string().contains("does-not-exist"), "{error}");
        assert!(
            crate::provider_catalog::mcp_tools_for("does-not-exist").is_empty(),
            "the predicate under test is the catalog's, not a second list"
        );

        // None of the refusals reached the file or the memory.
        assert!(store.entries().is_empty());
        assert!(
            !dir.join(POLICY_FILE).exists(),
            "a refused set must not write a policy file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_row_cap_is_enforced_when_the_store_already_holds_its_maximum() {
        let dir = temp_dir();
        // Built directly rather than loaded: with the catalog predicate enforced
        // on both paths, neither a file nor a request can fill the store past
        // the four providers the catalog publishes tools for. The cap is a guard
        // for a catalog that grows, so the state it guards against is
        // constructed here.
        let mut policies: HashMap<String, ToolPolicyEntry> = HashMap::new();
        for index in 0..MAX_POLICY_ROWS - 1 {
            let provider_id = format!("provider-{index:02}");
            policies.insert(provider_id.clone(), entry(&provider_id, Some(true), &[]));
        }
        policies.insert("claude".to_string(), entry("claude", Some(false), &[]));
        let store = ToolPolicyStore {
            path: dir.join(POLICY_FILE),
            policies: Mutex::new(policies),
            failed_closed: AtomicBool::new(false),
            load_error: Mutex::new(None),
        };
        assert_eq!(store.entries().len(), MAX_POLICY_ROWS);

        // Another provider would be the 65th row and is refused before any
        // write, so neither the store nor the disk moves.
        let error = store
            .set("grok", Some(true), Vec::new())
            .expect_err("a 65th row must be refused");
        assert!(error.to_string().contains("64"), "{error}");
        assert_eq!(store.entries().len(), MAX_POLICY_ROWS);
        assert!(
            !dir.join(POLICY_FILE).exists(),
            "a refused set must not write"
        );

        // A provider the store already holds is replaced, not added to: the cap
        // counts rows, not writes.
        store
            .set("claude", Some(true), vec!["some_tool".to_string()])
            .expect("replacing a stored row is not an add");
        assert_eq!(store.entries().len(), MAX_POLICY_ROWS);
        assert_eq!(
            store.get(Some("claude")),
            Some(entry("claude", Some(true), &["some_tool"]))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_for_a_provider_with_no_mcp_tools_is_dropped_at_load() {
        let dir = temp_dir();
        let document = serde_json::json!({
            "claude-acp": {
                "providerId": "claude-acp",
                "enabled": false,
                "disabledTools": [],
            },
            "claude": {
                "providerId": "claude",
                "enabled": false,
                "disabledTools": ["some_tool"],
            },
        });
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec_pretty(&document).expect("json"),
        )
        .expect("seed");

        let store = ToolPolicyStore::load(&dir);
        // A registry wrapper id has no broker tools, so the broker — which looks
        // a session's own catalog id up — can never consult a row under it. The
        // row goes, the rest of the document stays, and the file is not
        // quarantined: nothing here is corruption.
        assert!(store.get(Some("claude-acp")).is_none());
        assert_eq!(
            store.get(Some("claude")),
            Some(entry("claude", Some(false), &["some_tool"]))
        );
        assert_eq!(store.entries().len(), 1);
        assert!(dir.join(POLICY_FILE).is_file());
        no_corrupt_siblings(&dir);
        assert!(
            crate::provider_catalog::mcp_catalog_id("claude-acp").is_none(),
            "the predicate under test is the catalog's, not a second list"
        );

        // The next write persists the admitted document, so a dropped row does
        // not come back through the file.
        store.set("grok", Some(true), Vec::new()).expect("set");
        let reopened = ToolPolicyStore::load(&dir);
        assert_eq!(reopened.entries().len(), 2);
        assert!(reopened.get(Some("claude-acp")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_row_whose_key_and_provider_id_disagree_is_dropped_at_load() {
        let dir = temp_dir();
        let document = serde_json::json!({
            "claude": {
                "providerId": "grok",
                "enabled": false,
                "disabledTools": ["some_tool"],
            },
            "grok": { "providerId": "grok", "enabled": true, "disabledTools": [] },
        });
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec_pretty(&document).expect("json"),
        )
        .expect("seed");

        let store = ToolPolicyStore::load(&dir);
        // The mismatched row is dropped rather than re-keyed either way:
        // `get("claude")` must not hand claude grok's deny list.
        assert!(store.get(Some("claude")).is_none());
        assert_eq!(
            store.get(Some("grok")),
            Some(entry("grok", Some(true), &[]))
        );
        assert_eq!(store.entries().len(), 1);
        // A mismatch is repaired, not treated as corruption.
        assert!(dir.join(POLICY_FILE).is_file());
        no_corrupt_siblings(&dir);

        // The next write persists the repaired document.
        store.set("claude", Some(false), Vec::new()).expect("set");
        let reopened = ToolPolicyStore::load(&dir);
        assert_eq!(
            reopened.get(Some("claude")),
            Some(entry("claude", Some(false), &[]))
        );
        assert_eq!(reopened.entries().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C-1: the predicate admits a provider id case-insensitively while the
    /// lookup is an exact key, so a row stored as the caller spelled it was
    /// admitted and then unreachable. `CLAUDE` is admitted *as* `claude`.
    #[test]
    fn an_uppercase_set_is_stored_under_the_catalog_id() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        store
            .set("CLAUDE", Some(false), vec!["some_tool".to_string()])
            .expect("set CLAUDE");
        let expected = entry("claude", Some(false), &["some_tool"]);
        assert_eq!(store.entries(), vec![expected.clone()]);

        // The file carries the canonical key and the canonical providerId, so a
        // reopened store — or the app reading the document — sees one provider.
        let text = std::fs::read_to_string(dir.join(POLICY_FILE)).expect("policy file");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert!(parsed.get("CLAUDE").is_none(), "{parsed}");
        assert_eq!(parsed["claude"]["providerId"], "claude");
        assert_eq!(parsed["claude"]["enabled"], false);
        assert_eq!(
            ToolPolicyStore::load(&dir).get(Some("claude")),
            Some(expected)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C-1 from the session's side: the row a caller set as `CLAUDE` is the row
    /// a session registered as `claude` is gated by. The broker looks the
    /// session's own catalog id up, and that is the lowercase one.
    #[test]
    fn a_session_registered_as_claude_sees_the_policy_set_as_uppercase() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        store
            .set("CLAUDE", Some(true), vec!["some_future_tool".to_string()])
            .expect("set under CLAUDE");
        let policy = store
            .get(Some("claude"))
            .expect("the row is stored under claude");
        assert_eq!(policy.provider_id, "claude");
        assert!(
            !is_tool_enabled(Some(&policy), "some_future_tool"),
            "the deny list the caller set as CLAUDE has to reach the session"
        );
        assert!(is_tool_enabled(Some(&policy), "another_tool"));
        // The lookup resolves the id the same way, whichever spelling asks.
        assert_eq!(store.get(Some("CLAUDE")), Some(policy.clone()));
        assert_eq!(
            ToolPolicyStore::load(&dir).get(Some("claude")),
            Some(policy)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same fold on the load path: a hand-edited document that spells the
    /// provider in caps is the row for the catalog's id, not a row nothing can
    /// reach. Both sides of the row are canonical after the load.
    #[test]
    fn an_uppercase_key_in_the_file_is_admitted_under_the_catalog_id() {
        let dir = temp_dir();
        let document = serde_json::json!({
            "CLAUDE": {
                "providerId": "CLAUDE",
                "enabled": false,
                "disabledTools": ["some_tool"],
            },
            "grok": { "providerId": "GROK", "enabled": true, "disabledTools": [] },
        });
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec_pretty(&document).expect("json"),
        )
        .expect("seed");

        let store = ToolPolicyStore::load(&dir);
        assert_eq!(
            store.get(Some("claude")),
            Some(entry("claude", Some(false), &["some_tool"]))
        );
        assert_eq!(
            store.get(Some("grok")),
            Some(entry("grok", Some(true), &[]))
        );
        assert_eq!(store.entries().len(), 2);
        // A spelling is not corruption: the document is usable as written.
        assert!(dir.join(POLICY_FILE).is_file());
        no_corrupt_siblings(&dir);

        // And the next write persists the canonical spelling on both sides.
        store.set("gemini", Some(true), Vec::new()).expect("set");
        let text = std::fs::read_to_string(dir.join(POLICY_FILE)).expect("policy file");
        assert!(text.contains("\"claude\""), "{text}");
        assert!(!text.contains("CLAUDE"), "{text}");
        assert!(!text.contains("GROK"), "{text}");
        assert_eq!(ToolPolicyStore::load(&dir).entries().len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two keys that fold to one provider are one row, not two: the document is
    /// read in sorted key order, so the row that survives is a property of the
    /// document rather than of the hash map's iteration order.
    #[test]
    fn two_keys_for_one_provider_leave_one_row() {
        let dir = temp_dir();
        let document = serde_json::json!({
            "CLAUDE": {
                "providerId": "claude",
                "enabled": false,
                "disabledTools": ["some_tool"],
            },
            "claude": { "providerId": "claude", "enabled": true, "disabledTools": [] },
        });
        std::fs::write(
            dir.join(POLICY_FILE),
            serde_json::to_vec_pretty(&document).expect("json"),
        )
        .expect("seed");

        let store = ToolPolicyStore::load(&dir);
        assert_eq!(store.entries().len(), 1, "one provider, one row");
        assert_eq!(
            store.get(Some("claude")),
            Some(entry("claude", Some(false), &["some_tool"])),
            "the first key in order is the one admitted"
        );
        // One row for one provider is not corruption.
        assert!(dir.join(POLICY_FILE).is_file());
        no_corrupt_siblings(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory at the policy name is a file the store cannot read as a
    /// policy. It fails closed and stays where it is.
    #[test]
    fn an_unreadable_policy_file_fails_closed_and_is_left() {
        let dir = temp_dir();
        std::fs::create_dir(dir.join(POLICY_FILE)).expect("squat the policy name");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert!(store.load_error().is_some());
        assert!(dir.join(POLICY_FILE).is_dir(), "the file stays where it is");
        no_corrupt_siblings(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bom_prefixed_policy_loads_with_its_row() {
        let dir = temp_dir();
        let mut seeded: HashMap<String, ToolPolicyEntry> = HashMap::new();
        seeded.insert(
            "claude".to_string(),
            entry("claude", Some(true), &["some_tool"]),
        );
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend(serde_json::to_vec(&seeded).expect("json"));
        std::fs::write(dir.join(POLICY_FILE), &bytes).expect("seed");

        let store = ToolPolicyStore::load(&dir);
        assert_eq!(store.load_error(), None, "a BOM is not damage");
        let policy = store.get(Some("claude")).expect("the row survives");
        assert!(!is_tool_enabled(Some(&policy), "some_tool"));
        assert!(is_tool_enabled(Some(&policy), "another_tool"));
        assert!(dir.join(POLICY_FILE).is_file());
        no_corrupt_siblings(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_but_blank_policy_file_fails_closed() {
        // A first run has NO file. A file that exists but holds no policy —
        // 0 bytes, whitespace, or a lone BOM, the shape a truncated copy or
        // a sync tool leaves — is an unknown policy, and unknown denies.
        // Blank-as-absent stays for the non-security stores; this store
        // decides access, so blank is damage here.
        for (tag, bytes) in [
            ("empty", b"".as_slice()),
            ("whitespace", b"  \r\n\t ".as_slice()),
            ("bom-only", b"\xef\xbb\xbf".as_slice()),
        ] {
            let dir = crate::test_dirs::test_temp_dir(&format!("devboule-tool-policy-{tag}"));
            std::fs::write(dir.join(POLICY_FILE), bytes).expect("seed");
            let store = ToolPolicyStore::load(&dir);
            assert!(store.entries().is_empty());
            assert!(
                store.load_error().is_some(),
                "{tag}: an existing blank file denies, and the Status surface names it"
            );
            let policy = store.get(Some("claude")).expect("{tag}: deny-all row");
            assert!(
                !is_tool_enabled(Some(&policy), "some_tool"),
                "{tag}: denied"
            );
            assert_eq!(
                std::fs::read(dir.join(POLICY_FILE)).expect("bytes"),
                bytes,
                "{tag}: the file is kept, not replaced"
            );
            no_corrupt_siblings(&dir);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_missing_policy_file_is_still_a_first_run() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert_eq!(store.load_error(), None, "no file means no policy");
        assert!(
            is_tool_enabled(store.get(Some("claude")).as_ref(), "some_tool"),
            "no row means enabled"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_utf16_policy_file_fails_closed_and_names_its_encoding() {
        let dir = temp_dir();
        // UTF-16LE with BOM, the way older Windows editors save: decoded
        // here so the seed is honestly UTF-16, never hand-mangled bytes.
        let text = r#"{"claude": {"providerId": "claude", "enabled": false, "disabledTools": []}}"#;
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        std::fs::write(dir.join(POLICY_FILE), &bytes).expect("seed");

        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        assert!(
            store.load_error().is_some(),
            "UTF-16 is not decoded, it is denied"
        );
        let reason = store.load_error().expect("the Status surface names it");
        assert!(
            reason.contains("UTF-16"),
            "the reason says what the file is: {reason}"
        );
        let policy = store.get(Some("claude")).expect("deny-all row");
        assert!(!is_tool_enabled(Some(&policy), "some_future_tool"));
        assert_eq!(
            std::fs::read(dir.join(POLICY_FILE)).expect("bytes"),
            bytes,
            "the file is kept for the user to re-save as UTF-8"
        );
        no_corrupt_siblings(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S-1: the temp is the one name a half-written policy could sit under, so
    /// it carries the target's DACL before it holds any of its bytes. The order
    /// itself — DACL the moment `create_new` succeeds, `write_all` only after —
    /// is not observable from outside, so what is pinned here is the end state
    /// that order produces: a `tool-policies.tmp` left by a run that died
    /// mid-write is removed rather than written through or renamed over the
    /// target, the target holds this write and not the stale bytes, and it
    /// carries the current-user-only DACL.
    #[cfg(windows)]
    #[test]
    fn the_policy_file_dacl_names_only_the_current_user() {
        let dir = temp_dir();
        let stale = dir.join("tool-policies.tmp");
        std::fs::write(&stale, b"stale temp from a run that died mid-write").expect("seed stale");
        let store = ToolPolicyStore::load(&dir);
        store.set("claude", Some(false), Vec::new()).expect("set");
        assert!(
            !stale.exists(),
            "the stale temp is removed, not written through"
        );
        let text = std::fs::read_to_string(dir.join(POLICY_FILE)).expect("policy file");
        let document: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            document["claude"]["enabled"], false,
            "the target holds this write, not the stale temp's bytes"
        );
        let sddl =
            crate::security::dacl_sddl_for_path(&dir.join(POLICY_FILE)).expect("policy DACL");
        let sid = crate::security::current_user_sid().expect("sid");
        assert!(
            crate::security::dacl_is_current_user_only(&sddl, &sid),
            "the tool policy DACL must name only the current user: {sddl}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_path_is_inside_the_runtime_dir() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        assert_eq!(store.path(), dir.join(POLICY_FILE).as_path());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
