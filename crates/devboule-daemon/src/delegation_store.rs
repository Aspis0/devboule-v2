//! The permission-delegation switch: whether an agent that created a child
//! may answer that child's permission cards.
//!
//! The daemon is the only writer. The app sends `DelegationSet` over the named
//! pipe; the file is one JSON document beside the journal (`delegation.json`)
//! holding **only** `{ "enabled": bool }` — no pause, no cap, no per-session
//! grants. The brake was refused twice by the committente and is deleted, not
//! defaulted off; and a session id can wake up on a stranger's session after a
//! daemon restart (`ids.rs` `compose_session_id`), so nothing per-session may
//! exist on disk or in memory to revoke. There is nothing per creator, which
//! is why there is nothing to revoke per creator.
//!
//! The file is written the way `tool_policy.rs` writes its store: a
//! create-new temp file, a current-user-only DACL on Windows applied to that
//! temp before its first byte is written, then a rename over the target. A
//! crash leaves either the old switch or the new one, never half a file, and
//! a setting that decides what an agent may answer on its child's behalf is
//! never briefly readable by another user.
//!
//! **What this file failing means.** A document that cannot be read, parsed,
//! or admitted — over a cap, not a delegation document, or carrying a field
//! the switch does not hold — is quarantined exactly as a corrupt
//! tool-policy file is, and the store then reads **off** and says
//! `quarantined`. The failure has to mean "the human's answer is not on
//! disk", because the alternative is delegation running under a value nobody
//! wrote. The quarantine is logged, which is what keeps it from being silent;
//! the repair is re-reading the file, not trusting a last good value the
//! daemon was never given.
//!
//! **Read cadence — the rule every consumer inherits.** Read the switch
//! through [`DelegationStore::get`] **at the moment the decision is made**,
//! the way `tool_policy.rs` reads its store on every `tools/list` and every
//! `tools/call`. No caller may carry `enabled` past the call that read it:
//! not cached in a session, not snapshotted when a card is handed over, not
//! stashed in any longer-lived value. A cached OFF hides the control that
//! stops delegation; a cached ON answers a card after the human turned the
//! switch off — the second is exactly the authority the switch exists to
//! withhold. At this commit the readers are the `DelegationGet` and
//! `DelegationSet` dispatch arms in `server.rs` and the three registry reads
//! in `session.rs` (`delegation_enabled`, at the delegated answer, the
//! parked-card surfacing, and the snapshot builder); the
//! `nothing_reads_the_switch_outside_the_two_requests` test below pins that
//! until the delegated-answer pass inherits this cadence.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use devboule_protocol::DelegationSource;

/// The file inside the runtime directory. Named, not hashed, so a support
/// session can read it.
pub(crate) const DELEGATION_FILE: &str = "delegation.json";

/// The largest switch file this daemon will read (1 MiB).
///
/// The file holds one boolean, so anything larger is refused — never
/// truncated. Checked against the file's metadata before it is read, so an
/// oversized file is quarantined without being allocated.
pub(crate) const MAX_DELEGATION_FILE_BYTES: u64 = 1024 * 1024;

/// Why a switch write failed.
#[derive(Debug)]
pub(crate) enum DelegationError {
    /// The file could not be replaced. The store kept the switch it already
    /// had, so a restart still reads the value the caller was told was in
    /// force.
    Io(io::Error),
}

impl From<io::Error> for DelegationError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for DelegationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

/// The stored document. The file holds only this, and serde refuses anything
/// else: a consent switch with unknown fields would silently read past a
/// value it does not understand, and refusing the document quarantines it
/// into the answer that withholds power.
#[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DelegationDocument {
    enabled: bool,
}

/// The switch, in memory, with the file as its durable copy.
///
/// One mutex covers the value and the write, the same shape as the tool-policy
/// and agent-profile stores: two writes arriving together must not interleave
/// their read-and-replace of the file.
pub(crate) struct DelegationStore {
    path: PathBuf,
    inner: Mutex<Switch>,
}

struct Switch {
    enabled: bool,
    source: DelegationSource,
}

impl DelegationStore {
    /// Read `runtime_dir/delegation.json`.
    ///
    /// A missing file is the normal first run and is not an error: the switch
    /// reads off and `DelegationGet` says `default`, so the app can render
    /// "never configured" instead of "the human turned it off". A file that
    /// cannot be read, parsed, or admitted is quarantined — moved aside first,
    /// best effort, never overwritten — and the store reads off and says
    /// `quarantined`, with one log line naming the file and the reason. A
    /// daemon that refused to start over a malformed settings file would lock
    /// the user out of the app that could fix it; a daemon that overwrote the
    /// file would erase the evidence of what it refused.
    pub(crate) fn load(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(DELEGATION_FILE);
        let switch = match read_switch(&path) {
            Ok(Some(enabled)) => Switch {
                enabled,
                source: DelegationSource::File,
            },
            // No file yet is the first run, not damage: read off, and say
            // `default` so the app can render "never configured".
            Ok(None) => Switch {
                enabled: false,
                source: DelegationSource::Default,
            },
            Err(reason) => {
                match quarantine(&path) {
                    Some(kept) => eprintln!(
                        "delegation: {} is unusable ({reason}); moved to {} and starting with the switch off",
                        path.display(),
                        kept.display()
                    ),
                    None => eprintln!(
                        "delegation: {} is unusable ({reason}); it could not be moved aside, starting with the switch off",
                        path.display()
                    ),
                }
                Switch {
                    enabled: false,
                    source: DelegationSource::Quarantined,
                }
            }
        };
        Self {
            path,
            inner: Mutex::new(switch),
        }
    }

    /// The switch, and where this answer came from. The pair is one value:
    /// an `enabled` without its `source` collapses the three states the app
    /// must render distinctly (`default`, `quarantined`, `file`) back into
    /// one benign-looking "off".
    ///
    /// See the module comment for the read cadence: call this at the moment
    /// of the decision, never carry the answer forward.
    pub(crate) fn get(&self) -> (bool, DelegationSource) {
        let switch = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        (switch.enabled, switch.source)
    }

    /// Replace the switch and persist the file before the in-memory copy
    /// moves. A failed write leaves both the file and the memory as they
    /// were, so the caller can report a real failure instead of a value the
    /// next restart would forget.
    ///
    /// The reply is `(enabled, source)` read back from what was stored, not
    /// the caller's argument: the dispatch arm puts exactly this pair on the
    /// wire and into the push, so no client can hold a value the daemon does
    /// not. `source` is `file` on every success — a written switch is by
    /// definition a configured one, and a `quarantined` store leaves that
    /// state the moment a write lands.
    pub(crate) fn set(&self, enabled: bool) -> Result<(bool, DelegationSource), DelegationError> {
        let mut switch = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        write_switch(&self.path, &DelegationDocument { enabled })?;
        switch.enabled = enabled;
        switch.source = DelegationSource::File;
        Ok((switch.enabled, switch.source))
    }

    /// The file this store reads and writes, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Read the file, refusing one larger than the cap before it is read.
///
/// `Ok(None)` is a missing file — the first run, not an error. `Ok(Some)`
/// carries the switch a real document holds; `Err` is every other failure,
/// and each one quarantines.
fn read_switch(path: &Path) -> Result<Option<bool>, String> {
    match read_document(path) {
        Ok(document) => Ok(Some(document.enabled)),
        Err(reason) if reason == MISSING_FILE => Ok(None),
        Err(reason) => Err(reason),
    }
}

/// The sentinel `read_document` returns for a file that is not there, so
/// `read_switch` can tell the first run from damage without a second stat.
const MISSING_FILE: &str = "no delegation file exists yet";

fn read_document(path: &Path) -> Result<DelegationDocument, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(MISSING_FILE.to_string());
        }
        Err(error) => return Err(error.to_string()),
    };
    if metadata.len() > MAX_DELEGATION_FILE_BYTES {
        return Err(format!(
            "{} is {} bytes, over the {MAX_DELEGATION_FILE_BYTES}-byte cap",
            path.display(),
            metadata.len()
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "{path} is not a delegation document: {error}",
            path = path.display()
        )
    })
}

/// The most quarantine destinations one failed load tries before it gives up
/// and leaves the file where it is.
const MAX_QUARANTINE_ATTEMPTS: u64 = 32;

/// Distinguishes the destinations of two quarantines inside one process, so
/// two files refused in the same millisecond cannot be offered the same name.
static QUARANTINE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Move a file the store refused aside, so the next write cannot erase it.
///
/// Best effort by design, the same recipe as `tool_policy.rs`: a path that
/// cannot be renamed is left exactly where it is, and the caller's line says
/// so rather than claiming a move that did not happen.
fn quarantine(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    quarantine_at(path, now_millis())
}

/// [`quarantine`] with the clock supplied, so the names one call considers
/// are a property a test can occupy.
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

/// The name one quarantine attempt keeps `path` under: `path`'s own
/// directory, the millisecond, and this process's nonce for that attempt.
fn quarantine_name(path: &Path, millis: u128, nonce: u64) -> PathBuf {
    path.with_file_name(format!("{DELEGATION_FILE}.corrupt-{millis}-{nonce:08x}"))
}

/// Unix time in milliseconds, or 0 before the epoch: the timestamp in a
/// quarantine name, not a duration anything measures.
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// Write the switch file: a temp file created with `create_new`, its DACL
/// replaced with a current-user-only one on Windows before the first byte,
/// `sync_all`, then a rename over the target.
fn write_switch(path: &Path, document: &DelegationDocument) -> Result<(), DelegationError> {
    let bytes = serde_json::to_vec_pretty(document).map_err(io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        DelegationError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "delegation file has no parent directory",
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(DelegationError::from)?;
    let temp = path.with_extension("tmp");
    let result = (|| {
        // The temp name is this writer's own. `create_new` below refuses to
        // follow a file already at it, and a run that died between the create
        // and the rename would otherwise leave a temp that no later write can
        // ever get past.
        let _ = std::fs::remove_file(&temp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        // The switch decides what an agent may answer on its child's behalf,
        // so the temp file carries the same current-user-only DACL as the
        // tool-policy file — applied after the create and before the first
        // `write_all`, so no byte of it is ever on disk under a weaker DACL.
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
    result.map_err(DelegationError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_dir(label: &str) -> PathBuf {
        crate::test_dirs::test_temp_dir(&format!("devboule-delegation-{label}"))
    }

    fn quarantined(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read the store directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!("{DELEGATION_FILE}.corrupt-")))
            })
            .collect()
    }

    #[test]
    fn a_missing_file_reads_off_and_says_default() {
        let dir = store_dir("missing");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = DelegationStore::load(&dir);
        assert_eq!(store.get(), (false, DelegationSource::Default));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_written_switch_round_trips_through_a_restart() {
        let dir = store_dir("roundtrip");
        let store = DelegationStore::load(&dir);
        let (enabled, source) = store.set(true).expect("set");
        assert!(enabled);
        assert_eq!(source, DelegationSource::File);

        let reopened = DelegationStore::load(&dir);
        assert_eq!(reopened.get(), (true, DelegationSource::File));

        reopened.set(false).expect("set off");
        let reopened = DelegationStore::load(&dir);
        assert_eq!(reopened.get(), (false, DelegationSource::File));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The corrupt file is quarantined — kept as evidence, never overwritten
    /// — and the store reads off while saying `quarantined`, the state that
    /// is neither "never configured" nor "the human turned it off".
    #[test]
    fn a_corrupt_file_is_quarantined_and_reads_off_as_quarantined() {
        let dir = store_dir("corrupt");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(DELEGATION_FILE), "{ this is not json").expect("seed");

        let store = DelegationStore::load(&dir);
        assert_eq!(store.get(), (false, DelegationSource::Quarantined));
        let kept = quarantined(&dir);
        assert_eq!(kept.len(), 1, "one quarantine file, got {kept:?}");
        assert!(
            !dir.join(DELEGATION_FILE).exists(),
            "the refused file must not stay at its own name"
        );
        assert_eq!(
            std::fs::read(&kept[0]).expect("quarantined bytes"),
            b"{ this is not json",
            "the quarantine keeps the bytes it refused"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The file holds one boolean, so anything larger is refused — never
    /// truncated — and the refusal quarantines like any other unusable
    /// document. The padding trails a valid document on purpose: an
    /// implementation that read past the cap, or cut the file to the cap
    /// before parsing, would see `enabled: true` and hand the switch to a
    /// file the daemon never admitted.
    #[test]
    fn a_file_over_the_byte_cap_is_quarantined_before_it_is_read() {
        let dir = store_dir("overcap");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let mut oversized = br#"{"enabled": true}"#.to_vec();
        oversized.resize(MAX_DELEGATION_FILE_BYTES as usize + 1, b' ');
        std::fs::write(dir.join(DELEGATION_FILE), &oversized).expect("seed");

        let store = DelegationStore::load(&dir);
        assert_eq!(store.get(), (false, DelegationSource::Quarantined));
        assert_eq!(
            quarantined(&dir).len(),
            1,
            "over the cap is refused, not truncated"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A document the switch does not hold — a resurrected brake field, say
    /// — is not read past: refusing it lands in the state that withholds
    /// power and names the damage.
    #[test]
    fn a_document_carrying_a_field_the_switch_does_not_hold_is_quarantined() {
        let dir = store_dir("extra-field");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(DELEGATION_FILE),
            r#"{ "enabled": true, "pause": true }"#,
        )
        .expect("seed");

        let store = DelegationStore::load(&dir);
        assert_eq!(store.get(), (false, DelegationSource::Quarantined));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A failed write leaves the file and the memory as they were: the caller
    /// reports a real failure instead of a value the next restart would
    /// forget.
    #[test]
    fn a_failed_write_keeps_the_stored_value() {
        let dir = store_dir("failed-write");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = DelegationStore::load(&dir);
        store.set(true).expect("set on");

        // The file's own name is now a directory, so the rename inside
        // `write_switch` cannot land: a real I/O refusal, not a mock.
        std::fs::remove_file(store.path()).expect("clear the file");
        std::fs::create_dir_all(store.path()).expect("occupy the name");

        let error = store.set(false).expect_err("the write must fail");
        assert!(matches!(error, DelegationError::Io(_)), "{error}");
        assert_eq!(
            store.get(),
            (true, DelegationSource::File),
            "memory keeps the value the durable copy still holds"
        );
        std::fs::remove_dir_all(store.path()).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// C13, the constraint from `compose_session_id`: a session id can name
    /// a stranger's session after a restart, so **no per-session grant may
    /// exist on disk or in memory** — nothing keyed by session, nothing to
    /// revoke per creator. The store is one boolean; this test keeps it
    /// that way by refusing the shapes a grant store would take.
    #[test]
    fn the_store_holds_one_boolean_and_no_per_session_grants() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let store_src = std::fs::read(src.join("delegation_store.rs")).expect("store source");
        let store_src = String::from_utf8(store_src).expect("utf-8 source");
        // Built from parts so this test's own source never matches.
        let forbidden = [
            "Hash".to_owned() + "Map",
            "Hash".to_owned() + "Set",
            "BTree".to_owned() + "Map",
            "session_".to_owned() + "id:",
        ];
        for word in forbidden {
            assert!(
                !store_src.contains(&word),
                "the delegation store must not grow per-session state: found {word}"
            );
        }
        // And no second store anywhere claims the grant vocabulary. This
        // file is skipped: the collection scan above already pins it to one
        // boolean, and this test's own scanner would otherwise match itself.
        for entry in std::fs::read_dir(&src).expect("read src") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let name = name.to_str().expect("source file name");
            if !name.ends_with(".rs") || name == "delegation_store.rs" {
                continue;
            }
            let body = String::from_utf8(std::fs::read(entry.path()).expect("read source"))
                .expect("utf-8 source");
            for line in body.lines() {
                let shapes_grant = line.contains("delegation_grant")
                    || line.contains("grant_for_session")
                    || (line.contains("session_delegation") && !line.contains("SessionEvent"));
                assert!(
                    !shapes_grant,
                    "a per-session delegation grant shape appeared in {name}: {line}"
                );
            }
        }
    }

    /// The pass's assertion, as a test: the switch is read only through the
    /// store and the readers the slice names. The store is constructed once
    /// (in `server.rs`), read and written once each by the two dispatch arms
    /// there, attached once to the session registry, and read by the
    /// registry through **one** getter — `delegation_enabled` — whose call
    /// sites are the delegated answer, the parked-card surfacing, and the
    /// snapshot builder. A new reader — a cache, a broker tool, a fourth
    /// surface — constructs the forbidden state this test refuses, and the
    /// whitelist moves only in the same commit as the reader that justifies
    /// it.
    #[test]
    fn nothing_reads_the_switch_outside_the_two_requests() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed: &[&str] = &[
            "server.rs",
            "session.rs",
            // the session test module's own file: same readers, moved out of
            // `session.rs` by the pass-1 test split (no new reader)
            "session_tests.rs",
            // the delegated answer's switch road, moved out of
            // `session_tests.rs` by the split of that file: the same test store
            // attached to a test registry, no new reader
            "session_delegated_answer_tests.rs",
            // the child-profile move tests: same reader again, a test store
            // attached to a test registry (the C2 slice, no new reader)
            "session_child_profile_tests.rs",
            // the delegated-answer tests: same reader again, a test store
            // attached to a test registry (the C3 slice, no new reader —
            // the switch read stays in `session.rs`'s thin sequence)
            "session_child_permission_tests.rs",
            "session_child_permission_phase_tests.rs",
            "delegation_store.rs",
            "lib.rs",
        ];
        let mut offenders: Vec<String> = Vec::new();
        let mut server_lines: Vec<String> = Vec::new();
        let mut session_lines: Vec<String> = Vec::new();
        // One file's touches, attributed to the module it belongs to.
        // `name` is the whitelist key: files under `server/` count as
        // `server.rs` — the pass-3a domain split moved the two dispatch
        // arms there without adding a reader (same justification as the
        // pass-1 `session_tests.rs` entry above).
        let mut visit = |name: &str, body: &str| {
            for line in body.lines() {
                // Any receiver: `state.delegation`, `self.delegation`, or a
                // store handle under another name — the call shape is the
                // fact, not the variable it hangs from.
                let touches_store = line.contains("delegation.get()")
                    || line.contains("delegation.set(")
                    || line.contains("DelegationStore::load")
                    || line.contains("delegation_enabled");
                if !touches_store {
                    continue;
                }
                match name {
                    "server.rs" => server_lines.push(line.trim().to_string()),
                    "session.rs" => session_lines.push(line.trim().to_string()),
                    _ => {}
                }
                if !allowed.contains(&name) {
                    offenders.push(format!("{name}: {line}"));
                }
            }
        };
        for entry in std::fs::read_dir(&src).expect("read the crate's src") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let name = name.to_str().expect("source file name");
            if !name.ends_with(".rs") {
                continue;
            }
            let body = std::fs::read(entry.path()).expect("read source");
            let body = String::from_utf8(body).expect("utf-8 source");
            visit(name, &body);
        }
        for entry in std::fs::read_dir(src.join("server")).expect("read the server module") {
            let entry = entry.expect("entry");
            if !entry.file_name().to_str().expect("name").ends_with(".rs") {
                continue;
            }
            let body = std::fs::read(entry.path()).expect("read source");
            let body = String::from_utf8(body).expect("utf-8 source");
            visit("server.rs", &body);
        }
        assert!(
            offenders.is_empty(),
            "only the store, the dispatch arms, and the registry's named readers \
             may touch the switch; found: {offenders:?}"
        );

        // `server.rs`: one construction, one read, one write.
        let count = |lines: &[String], needle: &str| {
            lines.iter().filter(|line| line.contains(needle)).count()
        };
        assert_eq!(
            count(&server_lines, "DelegationStore::load"),
            1,
            "the store is constructed once"
        );
        assert_eq!(
            count(&server_lines, "delegation.get()"),
            1,
            "one read of the switch: the DelegationGet arm"
        );
        assert_eq!(
            count(&server_lines, "delegation.set("),
            1,
            "one write of the switch: the DelegationSet arm"
        );

        // `session.rs`: the registry reads through one getter, and exactly
        // three decisions consume it — the delegated answer, the parked-card
        // surfacing, and the snapshot builder. A fourth call site is a new
        // reader, and a new reader is a claim this test refuses until the
        // whitelist names it.
        assert_eq!(
            count(&session_lines, "fn delegation_enabled"),
            1,
            "one registry getter"
        );
        assert_eq!(
            count(&session_lines, "delegation_enabled()"),
            3,
            "the three named readers: the delegated answer, the surfacing,              the snapshot"
        );
        assert_eq!(
            count(&session_lines, "delegation.set("),
            1,
            "one store attach"
        );
    }
}
