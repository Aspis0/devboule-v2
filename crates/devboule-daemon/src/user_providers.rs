//! User provider rows: the catalogue's open half, read from disk.
//!
//! Paseo's row shape (`RECON-paseo-provider-config.md` §1), scoped to the
//! fields this pass consumes: `extends`, `label`, `description`, `command`
//! (a non-empty argv list) and `env`. A field the daemon accepts and ignores
//! is a promise it does not keep, so the shape carries nothing else and
//! unknown fields are refused (`deny_unknown_fields`) — a document naming
//! `models` or `enabled` gets a sentence that says so rather than a row that
//! silently does nothing. What is deferred, deliberately, is stated in the
//! refusal sentences: a row whose id is a built-in (Paseo's override half)
//! and a row extending a native family are both refused "not supported yet",
//! never silently ignored and never quietly treated as ACP.
//!
//! **What this file failing means.** The live registry is never emptied by a
//! bad read. A missing file is the normal first run (or the user removed
//! every row) and applies an empty record; a file that cannot be *read* —
//! any error but NotFound — keeps the providers already loaded and logs one
//! line naming the address and the reason; a document that fails parsing or
//! any row validation is refused whole, for the reason `agent_profiles.rs`
//! states: half a list of providers is not a smaller catalogue, it is a
//! different one. Only a fully valid document is built into a whole new
//! registry and swapped through the pass-2e-1 seam in one step.
//!
//! The read happens at the boundaries that answer "which providers exist"
//! (the create and resume roads), not on a timer: a create already pays for
//! a process spawn, so one small read there is nothing, and an edit takes
//! effect on the next creation rather than at the next restart — the same
//! liveness rule the profile store states for itself. The raw bytes are
//! compared with the last document seen and an unchanged document swaps
//! nothing (Paseo's deep-equal early return), so a boundary costs a read and
//! a compare, not a swap.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde::Deserialize;

/// The file inside the runtime directory, beside `agent-profiles.json`.
/// Named, not hashed, so a support session can read it.
pub(crate) const PROVIDERS_FILE: &str = "providers.json";

/// The largest provider file this daemon will read (1 MiB) — the same cap
/// shape `agent_profiles.rs` uses: checked against the file's metadata is
/// unnecessary at these sizes, so the read is capped by the byte count
/// actually read.
pub(crate) const MAX_PROVIDERS_FILE_BYTES: u64 = 1024 * 1024;

/// The most user rows one document may hold.
pub(crate) const MAX_PROVIDERS: usize = 64;

/// The longest provider id. The closed alphabet below already bounds what a
/// valid id can contain; this bounds its length.
pub(crate) const MAX_PROVIDER_ID_BYTES: usize = 64;

/// The longest `label`, in bytes.
pub(crate) const MAX_PROVIDER_LABEL_BYTES: usize = 200;

/// The longest `description`, in bytes: the same bound a profile's note has.
pub(crate) const MAX_PROVIDER_DESCRIPTION_BYTES: usize = 2 * 1024;

/// The most argv elements one row's `command` may carry.
pub(crate) const MAX_PROVIDER_COMMAND_ARGS: usize = 32;

/// The longest one argv element may be, in bytes.
pub(crate) const MAX_PROVIDER_COMMAND_ARG_BYTES: usize = 4 * 1024;

/// The most `env` entries one row may carry.
pub(crate) const MAX_PROVIDER_ENV_ENTRIES: usize = 32;

/// The longest one env key or value may be, in bytes.
pub(crate) const MAX_PROVIDER_ENV_BYTES: usize = 4 * 1024;

/// One user provider row: how a local process is born. That is the question
/// Paseo's row answers and A2A's Agent Card does not, which is why the shape
/// is Paseo's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UserProviderRow {
    /// The provider implementation this row rides. Mandatory for every row
    /// this pass accepts: this pass accepts `extends: "acp"` only, and a row
    /// extending a native family is refused with the sentence that says it is
    /// not supported yet.
    #[serde(default)]
    pub(crate) extends: Option<String>,
    #[serde(default)]
    pub(crate) label: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    /// The argv the row spawns. Mandatory: a row that cannot name a process
    /// is a profile target nothing can spawn, and that is the exact defect
    /// the design's ordering rule exists to prevent.
    #[serde(default)]
    pub(crate) command: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) env: Option<BTreeMap<String, String>>,
}

/// The ids the closed `extends` set may name, in the sentence the refusal
/// shows a user: the one implementation every row can ride (`"acp"`, the open
/// dimension's sentinel — a fixed token of the design, not a catalog row's
/// name) plus the native families, passed in from the impls so this module
/// never spells one.
pub(crate) fn extendable_ids_sentence(native_ids: &[String]) -> String {
    let mut names = vec!["acp".to_string()];
    names.extend(native_ids.iter().cloned());
    names.join(", ")
}

/// Is this id in the closed alphabet `^[a-z][a-z0-9-]*$`? Hand-rolled — the
/// pattern is one line, and a regex crate would drag the dependency-majors
/// gate into this commit for it.
fn id_in_closed_alphabet(id: &str) -> bool {
    if id.len() > MAX_PROVIDER_ID_BYTES {
        return false;
    }
    let bytes = id.as_bytes();
    matches!(bytes.first(), Some(first) if first.is_ascii_lowercase())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Parse and validate a whole document, or refuse it whole with the reason.
/// A record wrong in any row is refused whole, never truncated: half a list
/// of providers is not a smaller catalogue, it is a different one.
///
/// `native_ids` is the ids the native families bind by, from the impls —
/// this module never spells one.
pub(crate) fn parse_providers_document(
    bytes: &[u8],
    native_ids: &[String],
) -> Result<BTreeMap<String, UserProviderRow>, String> {
    if bytes.len() as u64 > MAX_PROVIDERS_FILE_BYTES {
        return Err(format!(
            "the document is {} bytes, over the {MAX_PROVIDERS_FILE_BYTES}-byte cap",
            bytes.len()
        ));
    }
    let document: BTreeMap<String, UserProviderRow> =
        serde_json::from_slice(bytes).map_err(|error| {
            format!("the document is not a provider record the daemon can read: {error}")
        })?;
    if document.len() > MAX_PROVIDERS {
        return Err(format!(
            "the document holds {} providers, over the {MAX_PROVIDERS}-provider cap",
            document.len()
        ));
    }
    for (id, row) in &document {
        validate_row(id, row, native_ids)?;
    }
    Ok(document)
}

/// The two Paseo validations, verbatim in meaning, plus the two scope limits
/// this pass states rather than hides.
fn validate_row(id: &str, row: &UserProviderRow, native_ids: &[String]) -> Result<(), String> {
    // Paseo validation 1: the id is a closed alphabet. A provider id becomes
    // a path segment and a config key; an unvalidated id is the
    // absolute-path-join defect waiting to happen.
    if !id_in_closed_alphabet(id) {
        return Err(format!(
            "provider id \"{id}\" is not a valid provider id: ids match ^[a-z][a-z0-9-]*$ \
             (a lowercase letter, then lowercase letters, digits and hyphens)"
        ));
    }
    // Scope limit, stated rather than hidden: Paseo's builtin-override half
    // (a built-in id with no `extends` overrides that built-in) is not
    // imported by this pass.
    // Ids **and** aliases: several catalog names are alias-only
    // (`claude-code`, `grok-build`, ...), and a row taking one of those would
    // shadow the built-in on the spawn road — `resolve_named` consults the
    // user rows before the catalog walk — while the profile lookup went on
    // canonicalising the same name to the built-in. One name, two answers.
    if crate::provider_catalog::catalog_reserved_names()
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(id))
    {
        return Err(format!(
            "provider id \"{id}\" is built in; user rows may not redefine a built-in provider \
             (not supported yet)"
        ));
    }
    // Paseo validation 2: a non-builtin id MUST declare `extends`, and
    // `extends` is checked against a closed set.
    let Some(extends) = row
        .extends
        .as_deref()
        .map(str::trim)
        .filter(|extends| !extends.is_empty())
    else {
        return Err(format!("provider \"{id}\" must declare extends"));
    };
    if !native_ids.iter().any(|native| native == extends) && extends != "acp" {
        return Err(format!(
            "provider \"{id}\" extends \"{extends}\", which is not in the set extends may name: {}",
            extendable_ids_sentence(native_ids)
        ));
    }
    // Scope limit, with its own sentence: a row extending a native family is
    // a real case we are deferring, so it is refused with a sentence that
    // says it is not supported yet — never silently ignored, never quietly
    // treated as ACP.
    if native_ids.iter().any(|native| native == extends) {
        return Err(format!(
            "provider \"{id}\" extends native provider \"{extends}\"; \
             extending a native family is not supported yet"
        ));
    }
    // A row this pass accepts is an ACP row, and an ACP row is its command:
    // without one it is a profile target nothing can spawn.
    let Some(command) = &row.command else {
        return Err(format!(
            "provider \"{id}\" must declare command (a non-empty argv list)"
        ));
    };
    if command.is_empty() {
        return Err(format!(
            "provider \"{id}\" must declare command (a non-empty argv list)"
        ));
    }
    for (position, arg) in command.iter().enumerate() {
        if arg.is_empty() {
            return Err(format!(
                "provider \"{id}\" has an empty command element at position {}",
                position + 1
            ));
        }
        if arg.len() > MAX_PROVIDER_COMMAND_ARG_BYTES {
            return Err(format!(
                "the command element at position {} of provider \"{id}\" is {} bytes, \
                 over the {MAX_PROVIDER_COMMAND_ARG_BYTES}-byte cap",
                position + 1,
                arg.len()
            ));
        }
    }
    if command.len() > MAX_PROVIDER_COMMAND_ARGS {
        return Err(format!(
            "provider \"{id}\" carries {} command elements, over the \
             {MAX_PROVIDER_COMMAND_ARGS}-element cap",
            command.len()
        ));
    }
    if let Some(env) = &row.env {
        if env.len() > MAX_PROVIDER_ENV_ENTRIES {
            return Err(format!(
                "provider \"{id}\" carries {} env entries, over the \
                 {MAX_PROVIDER_ENV_ENTRIES}-entry cap",
                env.len()
            ));
        }
        for (key, value) in env {
            if key.is_empty() {
                return Err(format!("provider \"{id}\" has an empty env key"));
            }
            if key.len() > MAX_PROVIDER_ENV_BYTES {
                return Err(format!(
                    "the env key \"{key}\" of provider \"{id}\" is {} bytes, \
                     over the {MAX_PROVIDER_ENV_BYTES}-byte cap",
                    key.len()
                ));
            }
            if value.len() > MAX_PROVIDER_ENV_BYTES {
                return Err(format!(
                    "the env value of \"{key}\" in provider \"{id}\" is {} bytes, \
                     over the {MAX_PROVIDER_ENV_BYTES}-byte cap",
                    key.len()
                ));
            }
        }
    }
    check_optional_text(id, "label", &row.label, MAX_PROVIDER_LABEL_BYTES)?;
    check_optional_text(
        id,
        "description",
        &row.description,
        MAX_PROVIDER_DESCRIPTION_BYTES,
    )?;
    Ok(())
}

/// An optional text field is either absent, or present and saying something:
/// omitting the field is how a caller says "none", so `""` — or whitespace —
/// is a caller that meant to say something it did not manage to say (the
/// reason `agent_profiles.rs` gives its own optional fields).
fn check_optional_text(
    id: &str,
    field: &str,
    value: &Option<String>,
    cap: usize,
) -> Result<(), String> {
    let Some(text) = value else {
        return Ok(());
    };
    if text.trim().is_empty() {
        return Err(format!(
            "provider \"{id}\" has an empty {field}; omit the field to say it has none"
        ));
    }
    if text.len() > cap {
        return Err(format!(
            "the {field} of provider \"{id}\" is {} bytes, over the {cap}-byte cap",
            text.len()
        ));
    }
    Ok(())
}

/// The last raw document this process saw — applied or refused — so a
/// boundary re-reads, compares, and stops before re-applying or re-logging
/// the same document (Paseo's deep-equal early return). Guarded together
/// with the refresh itself: two threads arriving together must not
/// interleave read-and-swap.
pub(crate) struct RowsState {
    last_seen: Option<Vec<u8>>,
}

static ROWS_STATE: Mutex<RowsState> = Mutex::new(RowsState { last_seen: None });

/// The lock every swap of the live registry takes — not only the rows
/// file's. A test that puts rows into the live registry holds it across its
/// assertions, so a concurrent production refresh (which blocks on the same
/// lock) cannot swap its rows out under it; and `provider.rs`'s seam test
/// holds it because snapshot *identity* is only observable if nothing else
/// swaps between its read and its swap. Production only ever holds it for
/// one read-compare-swap.
pub(crate) fn lock_rows_state() -> MutexGuard<'static, RowsState> {
    // A test that fails while holding the guard poisons it; the next refresh
    // must still work, so the poison is unwound rather than inherited.
    ROWS_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Read the user rows file and, if it changed, apply it to the live
/// registry. Never panics on a bad file: every failure keeps the providers
/// already loaded and names the address once per distinct document.
pub(crate) fn refresh_user_rows(runtime_dir: &Path) {
    let mut state = lock_rows_state();
    refresh_user_rows_with(&mut state, runtime_dir);
}

/// The refresh body, for a caller that already holds the rows lock (the
/// tests, which hold it across their assertions so a concurrent production
/// refresh cannot swap their rows out under them). A caller that does not
/// hold the lock must use [`refresh_user_rows`].
pub(crate) fn refresh_user_rows_with(state: &mut RowsState, runtime_dir: &Path) {
    let path = runtime_dir.join(PROVIDERS_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        // A missing file is the normal first run, or the user removed every
        // row. Absence is deliberate, so it retires whatever rows were
        // loaded. (A zero-byte file that exists is not absence: it is
        // malformed JSON, and it is refused like any other bad document.)
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if state.last_seen.as_deref() != Some(&[]) {
                crate::session::apply_user_rows(BTreeMap::new());
                state.last_seen = Some(Vec::new());
            }
            return;
        }
        // Unreadable is not empty: a read error must not delete the user's
        // providers. The live snapshot stands; one line names the address.
        Err(error) => {
            eprintln!(
                "user providers: {} could not be read ({error}); keeping the providers already loaded",
                path.display()
            );
            return;
        }
    };
    apply_document(state, bytes, &path);
}

/// Apply one raw document, deduped by bytes. A document that fails is
/// remembered as seen — the refusal logs once per distinct document, not
/// once per boundary — and the live registry keeps whatever was loaded
/// before it.
fn apply_document(state: &mut RowsState, bytes: Vec<u8>, path: &Path) {
    if state.last_seen.as_deref() == Some(bytes.as_slice()) {
        return;
    }
    let native_ids = crate::session::native_family_ids();
    match parse_providers_document(&bytes, &native_ids) {
        Ok(rows) => {
            crate::session::apply_user_rows(rows);
            state.last_seen = Some(bytes);
        }
        Err(reason) => {
            eprintln!(
                "user providers: {} refused ({reason}); keeping the providers already loaded",
                path.display()
            );
            state.last_seen = Some(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The ids the impls bind by, the same source production validation uses.
    fn natives() -> Vec<String> {
        crate::session::native_family_ids()
    }

    fn parse(bytes: &[u8]) -> Result<BTreeMap<String, UserProviderRow>, String> {
        parse_providers_document(bytes, &natives())
    }

    /// A row this pass accepts: extends acp, an argv, an env entry.
    fn valid_row_json(id: &str) -> String {
        format!(
            r#"{{"{id}": {{"extends": "acp", "label": "My agent",
                "description": "a local agent",
                "command": ["/usr/local/bin/{id}", "--chat"],
                "env": {{"MY_KEY": "value"}}}}}}"#
        )
    }

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule-user-providers-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_valid_row_parses_with_every_field_it_declares() {
        let rows = parse(valid_row_json("my-agent").as_bytes()).expect("valid row");
        let row = rows.get("my-agent").expect("the row is keyed by its id");
        assert_eq!(row.extends.as_deref(), Some("acp"));
        assert_eq!(
            row.command.as_deref(),
            Some(&["/usr/local/bin/my-agent".to_string(), "--chat".to_string()][..])
        );
        assert_eq!(
            row.env.as_ref().and_then(|env| env.get("MY_KEY")),
            Some(&"value".to_string())
        );
    }

    /// M2-e2-a: an id outside the closed alphabet is refused, naming the id
    /// and the pattern — an unvalidated id is the path-join defect waiting
    /// to happen.
    #[test]
    fn an_id_outside_the_closed_alphabet_is_refused() {
        for id in [
            "My-Agent",
            "bad/id",
            "has_underscore",
            "-leading",
            "1leading",
            "a b",
        ] {
            let error = parse(valid_row_json(id).as_bytes())
                .err()
                .unwrap_or_else(|| panic!("id {id:?} must be refused"));
            assert!(
                error.contains(&format!("\"{id}\"")) && error.contains("^[a-z][a-z0-9-]*$"),
                "the refusal names the id and the pattern: {error}"
            );
        }
    }

    /// M2-e2-b: a custom row MUST declare `extends`.
    #[test]
    fn a_row_without_extends_is_refused() {
        let document = r#"{"my-agent": {"command": ["/bin/my-agent"]}}"#;
        let error = parse(document.as_bytes()).expect_err("refused");
        assert!(
            error.contains("my-agent") && error.contains("must declare extends"),
            "the refusal is Paseo's, naming the row: {error}"
        );
    }

    /// M2-e2-c: `extends` is checked against a closed set — ours is the
    /// native ids plus the `acp` sentinel, and the refusal names the set.
    #[test]
    fn extends_outside_the_closed_set_is_refused() {
        let document = r#"{"my-agent": {"extends": "gemini", "command": ["/bin/my-agent"]}}"#;
        let error = parse(document.as_bytes()).expect_err("refused");
        assert!(
            error.contains("gemini"),
            "the refusal names the value: {error}"
        );
        assert!(
            error.contains(&extendable_ids_sentence(&natives())),
            "the refusal names the closed set: {error}"
        );
    }

    /// The scope limit, stated in its own sentence: a native-extending row
    /// is a real case deferred, so it is refused "not supported yet" — never
    /// silently ignored, never quietly treated as ACP.
    #[test]
    fn a_row_extending_a_native_family_is_refused_as_not_supported_yet() {
        for native in natives() {
            let document = format!(
                r#"{{"my-agent": {{"extends": "{native}", "command": ["/bin/my-agent"]}}}}"#
            );
            let error = parse(document.as_bytes()).expect_err("refused");
            assert!(
                error.contains("not supported yet") && error.contains(&native),
                "the refusal says it is not supported yet and names the family: {error}"
            );
        }
    }

    /// The other deferred half: Paseo's builtin-override shape is not
    /// imported, so a row taking a built-in's id is refused "not supported
    /// yet" rather than shadowing the built-in.
    #[test]
    fn a_row_taking_a_builtin_id_is_refused_as_not_supported_yet() {
        // A canonical id, and an **alias-only** name. The second is the one
        // that nearly shipped: aliases are catalog names too, and a row
        // taking one would be spawned by `resolve_named` (which reads the
        // user rows first) while `catalog_provider_id` still canonicalised
        // the same name to the built-in.
        for document in [
            r#"{"claude": {"extends": "acp", "command": ["/bin/claude"]}}"#,
            r#"{"claude-code": {"extends": "acp", "command": ["/bin/mine"]}}"#,
        ] {
            let error = parse(document.as_bytes()).expect_err("refused");
            assert!(
                error.contains("built in") && error.contains("not supported yet"),
                "the refusal says the id is built in and not supported yet: {error}"
            );
        }
    }

    /// A row that cannot name a process cannot spawn, and a profile target
    /// nothing can spawn is the defect the design's ordering rule exists to
    /// prevent — so the command is mandatory and non-empty at validation,
    /// before any row can reach the registry.
    #[test]
    fn a_row_without_a_spawnable_command_is_refused() {
        for document in [
            r#"{"my-agent": {"extends": "acp"}}"#.to_string(),
            r#"{"my-agent": {"extends": "acp", "command": []}}"#.to_string(),
            r#"{"my-agent": {"extends": "acp", "command": ["/bin/ok", ""]}}"#.to_string(),
        ] {
            let error = parse(document.as_bytes()).expect_err("refused");
            assert!(
                error.contains("command"),
                "the refusal names the command: {error}"
            );
        }
    }

    /// A field the daemon would accept and ignore is a promise it does not
    /// keep, so the shape refuses unknown fields instead of swallowing them.
    #[test]
    fn an_unknown_field_is_refused_not_ignored() {
        let document =
            r#"{"my-agent": {"extends": "acp", "command": ["/bin/x"], "enabled": true}}"#;
        let error = parse(document.as_bytes()).expect_err("refused");
        assert!(
            error.contains("enabled") || error.contains("unknown field"),
            "the refusal names the field: {error}"
        );
    }

    #[test]
    fn an_oversize_document_is_refused_whole() {
        let big = vec![b' '; (MAX_PROVIDERS_FILE_BYTES + 1) as usize];
        let error = parse(&big).expect_err("refused");
        assert!(error.contains("byte cap"), "{error}");
    }

    #[test]
    fn an_empty_optional_field_must_be_omitted_not_left_blank() {
        let document = r#"{"my-agent": {"extends": "acp", "command": ["/bin/x"], "label": "  "}}"#;
        let error = parse(document.as_bytes()).expect_err("refused");
        assert!(error.contains("label"), "{error}");
    }

    /// The refresh loads a valid document through the seam: the row becomes
    /// live (resolvable, and visible on the snapshot the profile lookup
    /// reads), and deleting the file retires it. The rows lock is held
    /// across the assertions so a concurrent production refresh cannot swap
    /// the rows out under the test.
    #[test]
    fn refresh_loads_and_retires_rows_through_the_seam() {
        let dir = temp_dir();
        let mut gate = lock_rows_state();
        std::fs::write(dir.join(PROVIDERS_FILE), valid_row_json("my-agent")).expect("seed");

        refresh_user_rows_with(&mut gate, &dir);
        let live = crate::session::catalog_registry();
        let row = live
            .user_row_for("my-agent")
            .expect("the row is live after the refresh");
        assert_eq!(
            row.command.as_deref(),
            Some(&["/usr/local/bin/my-agent".to_string(), "--chat".to_string()][..])
        );

        // Deleting the file is a deliberate removal: the next boundary
        // retires the rows.
        std::fs::remove_file(dir.join(PROVIDERS_FILE)).expect("remove");
        refresh_user_rows_with(&mut gate, &dir);
        assert!(
            crate::session::catalog_registry()
                .user_row_for("my-agent")
                .is_none(),
            "the retired row is no longer live"
        );

        crate::session::apply_user_rows(BTreeMap::new());
        drop(gate);
    }

    /// Unreadable is not empty: a file that cannot be read (here: a
    /// directory where the file belongs) keeps the providers already loaded,
    /// and the refresh does not panic.
    #[test]
    fn an_unreadable_file_keeps_the_loaded_rows() {
        // A distinct id (and therefore distinct document bytes) from the
        // other refresh tests: the last-seen dedup is content-based and
        // process-wide, and a skipped refresh would defeat the test.
        let dir = temp_dir();
        let mut gate = lock_rows_state();
        std::fs::write(dir.join(PROVIDERS_FILE), valid_row_json("kept-agent")).expect("seed");
        refresh_user_rows_with(&mut gate, &dir);
        assert!(crate::session::catalog_registry()
            .user_row_for("kept-agent")
            .is_some());

        std::fs::remove_file(dir.join(PROVIDERS_FILE)).expect("remove");
        std::fs::create_dir(dir.join(PROVIDERS_FILE)).expect("a directory in its place");
        refresh_user_rows_with(&mut gate, &dir);
        assert!(
            crate::session::catalog_registry()
                .user_row_for("kept-agent")
                .is_some(),
            "a read error never empties the catalogue"
        );

        crate::session::apply_user_rows(BTreeMap::new());
        drop(gate);
    }

    /// A document wrong in any row is refused whole: the valid row it also
    /// carries must not sneak in, and the rows loaded before stay live.
    #[test]
    fn a_document_with_one_bad_row_is_refused_whole() {
        let dir = temp_dir();
        let mut gate = lock_rows_state();
        std::fs::write(dir.join(PROVIDERS_FILE), valid_row_json("first-agent")).expect("seed");
        refresh_user_rows_with(&mut gate, &dir);
        assert!(crate::session::catalog_registry()
            .user_row_for("first-agent")
            .is_some());

        let mixed = r#"{"second-agent": {"extends": "acp", "command": ["/bin/second"]},
                "Bad/Id": {"extends": "acp", "command": ["/bin/bad"]}}"#
            .to_string();
        std::fs::write(dir.join(PROVIDERS_FILE), mixed).expect("rewrite");
        refresh_user_rows_with(&mut gate, &dir);
        let live = crate::session::catalog_registry();
        assert!(
            live.user_row_for("second-agent").is_none(),
            "the refused document contributes nothing"
        );
        assert!(
            live.user_row_for("first-agent").is_some(),
            "the previously loaded rows stand"
        );

        crate::session::apply_user_rows(BTreeMap::new());
        drop(gate);
    }
}
