//! Per-provider tool policy: which of the daemon's MCP broker tools a
//! provider's sessions are served.
//!
//! The daemon is the only writer. The app sends `ToolPolicySet` over the
//! named pipe; the broker reads the store on every `tools/list` and on every
//! `tools/call`, so a toggle takes effect on the next call rather than at the
//! next session. The store is one JSON file beside the journal
//! (`tool-policies.json`), written through [`crate::atomic::atomic_write`]:
//! a crash leaves either the old policy or the new one, never half a file.
//!
//! Each device owns its own file. A tool toggle is a local decision about
//! what this machine hands to an agent, so paired devices do not inherit or
//! propagate one another's policies.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use devboule_protocol::ToolPolicyEntry;

/// The file inside the runtime directory. Named, not hashed, so a support
/// session can read it.
pub(crate) const POLICY_FILE: &str = "tool-policies.json";

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
    /// A missing file is the normal first run and is not an error. A file
    /// that cannot be read or parsed also yields an empty store, loudly: a
    /// daemon that refused to start over a malformed settings file would lock
    /// the user out of the app that could fix it.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(POLICY_FILE);
        let policies = match read_policies(&path) {
            Ok(policies) => policies,
            Err(error) => {
                eprintln!(
                    "tool policy: {} could not be read ({error}); starting with no policies",
                    path.display()
                );
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
    pub(crate) fn set(
        &self,
        provider_id: &str,
        enabled: Option<bool>,
        disabled_tools: Vec<String>,
    ) -> io::Result<()> {
        let entry = ToolPolicyEntry {
            provider_id: provider_id.to_string(),
            enabled,
            disabled_tools,
        };
        let mut policies = self
            .policies
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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

fn read_policies(path: &Path) -> io::Result<HashMap<String, ToolPolicyEntry>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a tool policy document: {error}", path.display()),
        )
    })
}

fn write_policies(path: &Path, policies: &HashMap<String, ToolPolicyEntry>) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(policies).map_err(io::Error::other)?;
    crate::atomic::atomic_write(path, &bytes)
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
        // `atomic_write` stages through a sibling temp file and removes it.
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
    fn a_malformed_file_starts_empty_instead_of_panicking() {
        let dir = temp_dir();
        std::fs::write(dir.join(POLICY_FILE), b"{ this is not json").expect("seed");
        let store = ToolPolicyStore::load(&dir);
        assert!(store.entries().is_empty());
        // And a successful set repairs the file.
        store.set("claude", Some(false), Vec::new()).expect("set");
        assert!(ToolPolicyStore::load(&dir).get(Some("claude")).is_some());
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
