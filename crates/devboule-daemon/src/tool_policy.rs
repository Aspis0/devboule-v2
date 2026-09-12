//! Per-provider tool policy: which of the daemon's MCP broker tools a
//! provider's sessions are served.
//!
//! The daemon is the only writer. The app sends `ToolPolicySet` over the
//! named pipe; the broker reads the store on every `tools/list` and on every
//! `tools/call`, so a toggle takes effect on the next call rather than at the
//! next session. The store is one JSON file beside the journal
//! (`tool-policies.json`), written the way the MCP config is: a create-new temp
//! file, a current-user-only DACL on Windows, then a rename over the target.
//! A crash leaves either the old policy or the new one, never half a file, and
//! a policy that decides what an agent may call is never briefly readable by
//! another user.
//!
//! Each device owns its own file. A tool toggle is a local decision about
//! what this machine hands to an agent, so paired devices do not inherit or
//! propagate one another's policies.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
    if name == crate::provider_catalog::MCP_ROSTER_TOOL {
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
}

impl ToolPolicyStore {
    /// Read `runtime_dir/tool-policies.json`.
    ///
    /// A missing file is the normal first run and is not an error. A file that
    /// cannot be read, parsed, or admitted — over a cap, or not a policy
    /// document at all — also yields an empty store, but never silently and
    /// never destructively: the bad file is renamed aside first, best effort,
    /// and one line names it and the reason. A daemon that refused to start
    /// over a malformed settings file would lock the user out of the app that
    /// could fix it; a daemon that overwrote the file would erase the evidence
    /// of what it refused.
    ///
    /// A row no session can ever be gated by — a key that disagrees with its own
    /// `providerId`, or a provider id the catalog publishes no MCP tools for —
    /// is dropped instead of quarantining the document, and one line reports how
    /// many of each. Dropping is safe in both cases for the same reason: the row
    /// could not have been consulted the way it was written.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(POLICY_FILE);
        let policies = match load_policies(&path) {
            Ok((policies, dropped)) => {
                if dropped.mismatched > 0 || dropped.unknown_provider > 0 {
                    eprintln!(
                        "tool policy: {} dropped {} mismatched row(s) (key and providerId disagreed) and {} row(s) for provider ids with no MCP tools; starting without them",
                        path.display(),
                        dropped.mismatched,
                        dropped.unknown_provider
                    );
                }
                policies
            }
            Err(reason) => {
                match quarantine(&path) {
                    Some(kept) => eprintln!(
                        "tool policy: {} is unusable ({reason}); moved to {} and starting with no policies",
                        path.display(),
                        kept.display()
                    ),
                    None => eprintln!(
                        "tool policy: {} is unusable ({reason}); it could not be moved aside, starting with no policies",
                        path.display()
                    ),
                }
                HashMap::new()
            }
        };
        Self {
            path,
            policies: Mutex::new(policies),
        }
    }

    /// This provider's policy, or `None` when it has none.
    pub(crate) fn get(&self, provider_id: Option<&str>) -> Option<ToolPolicyEntry> {
        let provider_id = provider_id?;
        self.policies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(provider_id)
            .cloned()
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
    /// A request over a cap, or one naming a provider the daemon publishes no
    /// MCP tools for, is refused before anything is written: the first is a
    /// caller bug the daemon will not store, the second could never be
    /// consulted, because the broker looks a session's own catalog id up.
    pub(crate) fn set(
        &self,
        provider_id: &str,
        enabled: Option<bool>,
        disabled_tools: Vec<String>,
    ) -> Result<(), PolicyError> {
        check_row(provider_id, &disabled_tools).map_err(PolicyError::InvalidRequest)?;
        if !is_policy_provider(provider_id) {
            return Err(PolicyError::InvalidRequest(format!(
                "'{provider_id}' is not a provider the daemon publishes MCP tools for"
            )));
        }
        let entry = ToolPolicyEntry {
            provider_id: provider_id.to_string(),
            enabled,
            disabled_tools,
        };
        let mut policies = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !policies.contains_key(provider_id) && policies.len() >= MAX_POLICY_ROWS {
            return Err(PolicyError::InvalidRequest(format!(
                "the store already holds its cap of {MAX_POLICY_ROWS} providers, so '{provider_id}' cannot be added"
            )));
        }
        let mut next: HashMap<String, ToolPolicyEntry> = (*policies).clone();
        next.insert(provider_id.to_string(), entry);
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
    /// Rows whose outer key and `providerId` disagreed.
    mismatched: usize,
    /// Rows for a provider id the catalog publishes no MCP tools for.
    unknown_provider: usize,
}

/// May a policy row name this provider?
///
/// The predicate is the catalog's own: [`crate::provider_catalog::mcp_tools_for`]
/// is what fills `ProviderInfo.tools`, so a provider with no broker tools has
/// nothing a policy could gate, and a row under its name could never be
/// consulted. `set` and `load` both go through this one function, so a row the
/// store accepts is a row it would also accept from its file.
fn is_policy_provider(provider_id: &str) -> bool {
    !crate::provider_catalog::mcp_tools_for(provider_id).is_empty()
}

/// Parse the file, refusing one larger than the cap before it is read.
///
/// A missing file is an empty document, not an error: that is the first run.
fn read_policies(path: &Path) -> io::Result<HashMap<String, ToolPolicyEntry>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    if metadata.len() > MAX_POLICY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is {} bytes, over the {MAX_POLICY_FILE_BYTES}-byte cap",
                path.display(),
                metadata.len()
            ),
        ));
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a tool policy document: {error}", path.display()),
        )
    })
}

/// The document as the store will hold it, or the reason it cannot be used,
/// plus what was dropped.
///
/// Caps are checked on the document as it was written, then a row is dropped
/// and counted when it cannot be consulted as written: its outer key disagrees
/// with its own `providerId`, or its provider id is one the catalog publishes
/// no MCP tools for ([`is_policy_provider`], the predicate `set` enforces too).
/// Dropping is the right repair for both rather than a quarantine: neither row
/// could have been reached by the lookup the broker performs — which is keyed by
/// the session's own catalog id — so removing them cannot invent a policy, and
/// trusting the outer key instead would hand one provider another provider's
/// deny list.
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
    for (provider_id, entry) in document {
        if provider_id != entry.provider_id {
            dropped.mismatched += 1;
            continue;
        }
        if !is_policy_provider(&provider_id) {
            dropped.unknown_provider += 1;
            continue;
        }
        check_row(&provider_id, &entry.disabled_tools)?;
        admitted.insert(provider_id, entry);
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

/// Move a file the store refused aside, so the next write cannot erase it.
///
/// Best effort by design: this runs while the daemon is already reporting a
/// load failure, and a path that cannot be renamed is left exactly where it is
/// — the caller's line then says so, rather than claiming a move that did not
/// happen. The suffix is the unix time in milliseconds, so repeated failures
/// keep one file each instead of overwriting the previous evidence.
fn quarantine(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    let kept = path.with_file_name(format!("{POLICY_FILE}.corrupt-{millis}"));
    std::fs::rename(path, &kept).ok().map(|()| kept)
}

/// Serialize the whole store and replace the file with it.
///
/// The write is the MCP config's write (`mcp_broker::write_protected_json`): a
/// temp file created with `create_new`, flushed to disk, given a
/// current-user-only DACL on Windows, and only then renamed over the target.
fn write_policies(path: &Path, policies: &HashMap<String, ToolPolicyEntry>) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(policies).map_err(io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "tool policy file has no parent directory",
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
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // The policy decides which tools an agent may call, so the temp file
        // carries the same current-user-only DACL as the MCP config before the
        // rename: `security.rs` owns that call, so neither writer can drift on
        // what "protected" means. Off Windows there is no DACL to set.
        #[cfg(windows)]
        crate::security::apply_current_user_dacl(&temp)?;
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
    use std::sync::atomic::{AtomicU64, Ordering};

    fn entry(provider_id: &str, enabled: Option<bool>, disabled: &[&str]) -> ToolPolicyEntry {
        ToolPolicyEntry {
            provider_id: provider_id.to_string(),
            enabled,
            disabled_tools: disabled.iter().map(|name| (*name).to_string()).collect(),
        }
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule-tool-policy-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Every `<POLICY_FILE>.corrupt-*` sibling in `dir`, sorted.
    fn quarantined(dir: &Path) -> Vec<PathBuf> {
        let prefix = format!("{POLICY_FILE}.corrupt-");
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
    fn a_malformed_file_is_quarantined_rather_than_dropped() {
        let dir = temp_dir();
        std::fs::write(dir.join(POLICY_FILE), b"{ this is not json").expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());

        // The load failed loudly and the bytes are still on disk under the
        // name the `eprintln!` names: a daemon that started empty is not a
        // daemon that threw the file away.
        assert!(!dir.join(POLICY_FILE).exists());
        let kept = quarantined(&dir);
        assert_eq!(kept.len(), 1, "one quarantine file, got {kept:?}");
        assert!(
            kept[0]
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("tool-policies.json.corrupt-")),
            "the quarantine name names the file it keeps: {kept:?}"
        );
        assert_eq!(
            std::fs::read(&kept[0]).expect("quarantined bytes"),
            b"{ this is not json"
        );

        // And a successful set repairs the live file.
        store.set("claude", Some(false), Vec::new()).expect("set");
        assert!(ToolPolicyStore::load(&dir).get(Some("claude")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_over_the_byte_cap_is_quarantined_before_it_is_read() {
        let dir = temp_dir();
        assert_eq!(MAX_POLICY_FILE_BYTES, 1024 * 1024);
        // One byte over the cap, and not JSON either: the refusal has to come
        // from the metadata, so the daemon can never be made to allocate what
        // the file claims to hold.
        let oversized = vec![b' '; MAX_POLICY_FILE_BYTES as usize + 1];
        std::fs::write(dir.join(POLICY_FILE), &oversized).expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        let kept = quarantined(&dir);
        assert_eq!(kept.len(), 1, "one quarantine file, got {kept:?}");
        assert_eq!(
            std::fs::metadata(&kept[0]).expect("metadata").len(),
            MAX_POLICY_FILE_BYTES + 1,
            "the oversized file is moved, not truncated"
        );

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
    fn a_file_over_the_row_cap_is_quarantined() {
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
        assert_eq!(quarantined(&dir).len(), 1, "over the row cap is corrupt");
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
        assert!(quarantined(&dir).is_empty());
        assert!(
            !is_policy_provider("claude-acp"),
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
        assert!(quarantined(&dir).is_empty());

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

    #[cfg(windows)]
    #[test]
    fn the_policy_file_dacl_names_only_the_current_user() {
        let dir = temp_dir();
        let store = ToolPolicyStore::load(&dir);
        store.set("claude", Some(false), Vec::new()).expect("set");
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
