//! Aiming Oracle at a folder that is not the runtime's active workspace.
//!
//! Two commands live here. `oracle_folder_status` answers "does this folder
//! have an index, and how complete is it?". It takes no
//! `State<OracleRuntime>` parameter at all, so it cannot change the active
//! root — not by discipline, by signature. `oracle_ask_folder` runs the
//! ordinary query pipeline against one explicitly named folder's stores; the
//! caller names the root and the runtime keeps its own.
//!
//! Both build [`OracleDataPaths::from_root`] per call. Neither registers a
//! root, persists settings, starts a model download, or starts the index job.
//! The status probe never creates the data directory or the stores: it only
//! inspects paths that already exist, and only opens the metadata store when
//! that file is already there.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tauri::State;

use oracle_core::{
    chunk_index_status, manifest_files_for_root, IndexStatusSnapshot, LanceStore, Manifest,
    OracleDataPaths, SqliteStore,
};

use crate::backend::error::CommandError;

use super::errors::{core_error, invalid_configuration};
use super::query::{search_paths, validate_query};
use super::runtime::{OracleRuntime, ResolvedOraclePaths};
use super::status::folder_state_from_snapshot;
use super::types::{OracleFolderIndexState, OracleFolderIndexStatus, OracleSearchResponse};

/// What one read-only inspection of a folder's index files found.
///
/// Every field is a fact gathered from `fs::metadata` / `fs::read_to_string`;
/// nothing here opens a store or writes anything.
pub(super) struct FolderIndexProbe {
    /// The probed folder, canonicalized when the filesystem allowed it.
    pub(super) root: PathBuf,
    pub(super) data: OracleDataPaths,
    /// `Some` when the folder exists but cannot be listed. This is the
    /// distinction the answer must never collapse into "never indexed".
    pub(super) read_error: Option<String>,
    pub(super) metadata: Artifact,
    pub(super) chunks: Artifact,
    pub(super) manifest: ManifestProbe,
}

/// The state of one path that an index is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Artifact {
    /// Nothing at that path.
    Absent,
    /// A readable file (or directory) of the expected kind.
    Present,
    /// Something exists there, but it is not the kind of thing Oracle writes.
    WrongKind,
    /// The path could not be inspected at all (permissions, I/O).
    Unreadable(String),
}

/// The chunk manifest, with the read and parse failures that
/// [`oracle_core::load_manifest`] deliberately swallows kept apart, because
/// "the manifest is missing" and "the manifest is corrupt" are different
/// answers.
pub(super) enum ManifestProbe {
    Absent,
    Parsed(Manifest),
    Invalid(String),
    Unreadable(String),
}

/// Validate the shape of a requested folder without requiring it to be
/// readable. Only caller mistakes reject here: empty, relative, missing, or
/// not a directory. An unreadable directory is a real folder with a definite
/// answer, and each command decides what that answer is.
pub(super) fn resolve_folder_root(requested: &str) -> Result<PathBuf, CommandError> {
    let trimmed = requested.trim();
    if trimmed.is_empty() {
        return Err(invalid_configuration(
            "Choose a folder; the Oracle folder path was empty.",
        ));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(invalid_configuration(
            "The Oracle folder must be an absolute path, not a relative path.",
        ));
    }
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(invalid_configuration(format!(
                "The Oracle path is not a folder: {}. Choose an existing folder.",
                path.display()
            )))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(invalid_configuration(format!(
                "The Oracle folder does not exist: {}. Choose a folder that is already on disk.",
                path.display()
            )))
        }
        Err(error) => {
            return Err(core_error(
                &format!(
                    "accessing the Oracle folder {} failed. Check that it exists and that Devboule can read it",
                    path.display()
                ),
                error,
            ))
        }
    }
    // Best-effort: canonicalizing a directory the process cannot read can fail
    // on Windows, and that folder still needs the definite answer "unreadable"
    // rather than a caller error from this function.
    Ok(fs::canonicalize(&path).unwrap_or(path))
}

/// Inspect a folder's index files without changing anything.
pub(super) fn probe_folder(root: &Path) -> FolderIndexProbe {
    let data = OracleDataPaths::from_root(root);
    // `read_dir` is the one operation that needs list permission on the folder
    // itself; `metadata` on a denied directory usually still succeeds, so this
    // is the probe that actually detects an unreadable folder.
    let read_error = fs::read_dir(root).err().map(|error| error.to_string());
    let metadata = probe_artifact(&data.metadata, false);
    // `chunks.lancedb` is a directory in the default layout but a JSON file
    // when `CHUNK_DB_PATH` selects the JSON backend, so existence is the only
    // thing that can be asserted about it without assuming a store layout.
    let chunks = probe_present(&data.chunks);
    let manifest = probe_manifest(&data.manifest);
    FolderIndexProbe {
        root: root.to_path_buf(),
        data,
        read_error,
        metadata,
        chunks,
        manifest,
    }
}

fn probe_artifact(path: &Path, directory: bool) -> Artifact {
    match fs::metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir() == directory {
                Artifact::Present
            } else {
                Artifact::WrongKind
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Artifact::Absent,
        Err(error) => Artifact::Unreadable(error.to_string()),
    }
}

/// Existence and inspectability only, for a store whose on-disk layout depends
/// on configuration (a Lance directory or a JSON file).
fn probe_present(path: &Path) -> Artifact {
    match fs::metadata(path) {
        Ok(_) => Artifact::Present,
        Err(error) if error.kind() == ErrorKind::NotFound => Artifact::Absent,
        Err(error) => Artifact::Unreadable(error.to_string()),
    }
}

fn probe_manifest(path: &Path) -> ManifestProbe {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Manifest>(&text) {
            Ok(manifest) => ManifestProbe::Parsed(manifest),
            Err(error) => ManifestProbe::Invalid(error.to_string()),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => ManifestProbe::Absent,
        Err(error) => ManifestProbe::Unreadable(error.to_string()),
    }
}

/// Files the readable manifest records for this root, if there is one.
fn manifest_file_count(probe: &FolderIndexProbe) -> usize {
    match &probe.manifest {
        ManifestProbe::Parsed(manifest) => {
            // Explicit type: `manifest` is a reference, and the count needs an
            // owned value to hand to the pruning lookup.
            let mut manifest: Manifest = manifest.clone();
            manifest_files_for_root(&mut manifest, &probe.root, false)
                .map(|files| files.len())
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// Why the folder or its index files cannot be read, in the order the answer
/// must respect: the folder itself first, then the stores that a query reads.
fn unreadable_reason(probe: &FolderIndexProbe) -> Option<String> {
    if let Some(error) = &probe.read_error {
        return Some(format!(
            "Oracle cannot read the folder {}: {error}. Check the folder permissions.",
            probe.root.display()
        ));
    }
    match &probe.metadata {
        Artifact::Unreadable(error) => {
            return Some(format!(
                "Oracle cannot read the metadata store {}: {error}. Check the folder permissions.",
                probe.data.metadata.display()
            ))
        }
        Artifact::WrongKind => {
            return Some(format!(
                "{} exists but is not a file, so the Oracle index cannot be read. Move it aside and re-index this folder.",
                probe.data.metadata.display()
            ))
        }
        Artifact::Absent | Artifact::Present => {}
    }
    match &probe.chunks {
        Artifact::Unreadable(error) => {
            return Some(format!(
            "Oracle cannot read the chunk vector store {}: {error}. Check the folder permissions.",
            probe.data.chunks.display()
        ))
        }
        // `WrongKind` is unreachable for this store: it is probed by existence
        // only, because its on-disk layout depends on configuration.
        Artifact::Absent | Artifact::Present | Artifact::WrongKind => {}
    }
    match &probe.manifest {
        ManifestProbe::Unreadable(error) => Some(format!(
            "Oracle cannot read the chunk manifest {}: {error}. Check the folder permissions.",
            probe.data.manifest.display()
        )),
        ManifestProbe::Invalid(error) => Some(format!(
            "The chunk manifest {} is not valid JSON ({error}), so the Oracle index cannot be trusted. Re-index this folder to rebuild it.",
            probe.data.manifest.display()
        )),
        ManifestProbe::Absent | ManifestProbe::Parsed(_) => None,
    }
}

/// The five `usize` tallies that describe how much of a folder is indexed.
///
/// They travel together because they are read as a set: passed positionally,
/// two of them can be transposed in a call that still compiles and still
/// reports an index — just the wrong one. Naming them at the call site makes
/// that unexpressible.
#[derive(Default)]
struct IndexCounts {
    indexed_files: usize,
    total_files: usize,
    pending_files: usize,
    stale_files: usize,
    indexed_chunks: usize,
}

fn status_from_parts(
    probe: &FolderIndexProbe,
    state: OracleFolderIndexState,
    counts: IndexCounts,
    message: Option<String>,
) -> OracleFolderIndexStatus {
    OracleFolderIndexStatus {
        path: probe.root.display().to_string(),
        data_dir: probe.data.root.display().to_string(),
        state,
        indexed_files: counts.indexed_files,
        total_files: counts.total_files,
        pending_files: counts.pending_files,
        stale_files: counts.stale_files,
        indexed_chunks: counts.indexed_chunks,
        message,
    }
}

fn unreadable_status(probe: &FolderIndexProbe, reason: String) -> OracleFolderIndexStatus {
    status_from_parts(
        probe,
        OracleFolderIndexState::Unreadable,
        IndexCounts::default(),
        Some(reason),
    )
}

fn never_indexed_status(probe: &FolderIndexProbe, message: String) -> OracleFolderIndexStatus {
    status_from_parts(
        probe,
        OracleFolderIndexState::NeverIndexed,
        IndexCounts::default(),
        Some(message),
    )
}

/// Read the same snapshot the panel's `oracle_status` command reads, mapped to
/// a plain string so a failure becomes an "unreadable" answer instead of an
/// error the caller has to unpack.
///
/// Only called when `metadata.sqlite` already exists: `SqliteStore::new`
/// creates its parent directory, and this probe must never bring a data
/// directory into existence.
async fn read_folder_snapshot(probe: &FolderIndexProbe) -> Result<IndexStatusSnapshot, String> {
    let sqlite = SqliteStore::new(&probe.data.metadata)
        .map_err(|error| format!("opening the metadata store failed: {error}"))?;
    let chunk_vectors = LanceStore::new(&probe.data.chunks);
    chunk_index_status(&probe.root, &sqlite, &chunk_vectors, &probe.data.manifest)
        .await
        .map_err(|error| format!("reading the index failed: {error}"))
}

/// Refuse a query against a folder whose index is missing or unreadable.
///
/// A folder with no index is an error that names the folder. Falling back to
/// the runtime's active root would answer from a different corpus, which is
/// worse than not answering.
pub(super) fn ensure_folder_index_is_usable(probe: &FolderIndexProbe) -> Result<(), CommandError> {
    if let Some(error) = &probe.read_error {
        return Err(invalid_configuration(format!(
            "Oracle cannot read the folder {}: {error}. Check the folder permissions and try again.",
            probe.root.display()
        )));
    }
    match &probe.metadata {
        Artifact::Present => {}
        Artifact::Absent => {
            return Err(invalid_configuration(format!(
                "The folder {} has no Oracle index ({} does not exist). Index this folder before searching it; Oracle will not answer from another folder's index.",
                probe.root.display(),
                probe.data.metadata.display()
            )))
        }
        Artifact::WrongKind => {
            return Err(invalid_configuration(format!(
                "The folder {} has no usable Oracle index: {} exists but is not a file. Re-index this folder.",
                probe.root.display(),
                probe.data.metadata.display()
            )))
        }
        Artifact::Unreadable(error) => {
            return Err(invalid_configuration(format!(
                "Oracle cannot read the index of {}: {} could not be inspected ({error}). Check the folder permissions.",
                probe.root.display(),
                probe.data.metadata.display()
            )))
        }
    }
    match &probe.manifest {
        ManifestProbe::Absent | ManifestProbe::Parsed(_) => Ok(()),
        ManifestProbe::Invalid(error) => Err(invalid_configuration(format!(
            "The folder {} has no usable Oracle index: its chunk manifest is not valid JSON ({error}). Re-index this folder.",
            probe.root.display()
        ))),
        ManifestProbe::Unreadable(error) => Err(invalid_configuration(format!(
            "Oracle cannot read the chunk manifest of {}: {error}. Check the folder permissions.",
            probe.root.display()
        ))),
    }
}

pub(super) async fn oracle_folder_status_inner(
    requested: &str,
) -> Result<OracleFolderIndexStatus, CommandError> {
    let root = resolve_folder_root(requested)?;
    let probe = probe_folder(&root);

    // 1. Anything unreadable is its own answer. It must not fall through to
    //    "never indexed": re-indexing a folder whose index is intact but
    //    unreadable would rebuild hours of work that was already there.
    if let Some(reason) = unreadable_reason(&probe) {
        return Ok(unreadable_status(&probe, reason));
    }

    // 2. No index artifacts at all. This is decided from metadata alone, so a
    //    folder nobody ever indexed costs one directory listing and nothing
    //    else — no walk, no store, no model.
    if matches!(probe.metadata, Artifact::Absent) && matches!(probe.manifest, ManifestProbe::Absent)
    {
        return Ok(never_indexed_status(
            &probe,
            format!(
                "Oracle has no index for {} yet (nothing in {}). Index this folder to make it searchable.",
                probe.root.display(),
                probe.data.root.display()
            ),
        ));
    }

    // 3. The manifest records which files were indexed; the metadata store is
    //    what a query reads. With the manifest present but the store gone the
    //    index cannot be complete, and no snapshot can be read.
    if !matches!(probe.metadata, Artifact::Present) {
        let files = manifest_file_count(&probe);
        if files == 0 {
            return Ok(never_indexed_status(
                &probe,
                format!(
                    "Oracle has no index for {} yet; {} is empty. Index this folder to make it searchable.",
                    probe.root.display(),
                    probe.data.manifest.display()
                ),
            ));
        }
        return Ok(status_from_parts(
            &probe,
            OracleFolderIndexState::Partial,
            IndexCounts {
                // Only the manifest survives, so every recorded file is
                // still pending and no chunk could be counted.
                indexed_files: 0,
                total_files: files,
                pending_files: files,
                stale_files: 0,
                indexed_chunks: 0,
            },
            Some(format!(
                "The index of {} is incomplete: {} records {files} file(s) but the metadata store {} is missing. Re-index this folder to rebuild it.",
                probe.root.display(),
                probe.data.manifest.display(),
                probe.data.metadata.display()
            )),
        ));
    }

    // 4. An index exists: ask the same snapshot function the panel's status
    //    command uses, so "complete" here cannot drift from "complete" there.
    let snapshot = match read_folder_snapshot(&probe).await {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            return Ok(unreadable_status(
                &probe,
                format!(
                    "Oracle cannot read the index of {}: {reason}. The index may be corrupt; re-index this folder to rebuild it.",
                    probe.root.display()
                ),
            ))
        }
    };

    let state = folder_state_from_snapshot(&snapshot);
    let message = match state {
        OracleFolderIndexState::Ready => None,
        OracleFolderIndexState::Partial => Some(format!(
            "The index of {} is incomplete: {} of {} files indexed, {} pending, {} stale. Re-index this folder to finish it.",
            probe.root.display(),
            snapshot.indexed_files,
            snapshot.expected_files,
            snapshot.pending_files,
            snapshot.stale_files
        )),
        // The keyed states are unreachable here: an unreadable probe returned
        // at step 1, and an empty snapshot is handled by the mapping but still
        // needs a sentence.
        OracleFolderIndexState::NeverIndexed => Some(format!(
            "The index stores of {} hold no files yet. Index this folder to make it searchable.",
            probe.root.display()
        )),
        OracleFolderIndexState::Unreadable => None,
    };
    let counts = IndexCounts {
        indexed_files: snapshot.indexed_files,
        total_files: snapshot.expected_files,
        pending_files: snapshot.pending_files,
        stale_files: snapshot.stale_files,
        indexed_chunks: snapshot.sqlite_chunks,
    };

    Ok(status_from_parts(&probe, state, counts, message))
}

pub(super) async fn oracle_ask_folder_inner(
    runtime: &OracleRuntime,
    requested: &str,
    query: String,
) -> Result<OracleSearchResponse, CommandError> {
    let query = validate_query(query)?;
    let root = resolve_folder_root(requested)?;
    let probe = probe_folder(&root);
    ensure_folder_index_is_usable(&probe)?;

    // The stores come from the named folder; the embedder and reranker come
    // from the runtime. That split is deliberate: the model is
    // root-independent, and borrowing it is what keeps this command from
    // switching the runtime's workspace.
    let paths = ResolvedOraclePaths {
        workspace: probe.root.clone(),
        data: probe.data.clone(),
    };
    let pool = runtime.pool()?;
    let reranker = runtime.reranker();
    let model_status = runtime.model_status();
    // No model download is started here on purpose: asking about a folder must
    // not begin a multi-hundred-megabyte transfer as a side effect. A missing
    // model is reported by `ensure_model_is_available` inside `search_paths`.
    search_paths(&paths, &query, &pool, reranker, &model_status).await
}

/// Answer whether a folder has an Oracle index, and how complete it is.
///
/// Read-only by construction: the command has no runtime state to change, so
/// it cannot switch the active root, and it never starts indexing or a model
/// download. A missing or relative path rejects; an unreadable folder comes
/// back as `state: "unreadable"`, which is a different answer from "never
/// indexed".
#[tauri::command]
pub async fn oracle_folder_status(path: String) -> Result<OracleFolderIndexStatus, CommandError> {
    oracle_folder_status_inner(&path).await
}

/// Run one Oracle search against a named folder's index instead of the active
/// workspace, leaving the runtime's own root untouched.
///
/// A folder with no usable index is an error that names the folder; there is
/// no fallback to the active root, because an answer from the wrong index is
/// worse than no answer.
#[tauri::command]
pub async fn oracle_ask_folder(
    runtime: State<'_, OracleRuntime>,
    path: String,
    query: String,
) -> Result<OracleSearchResponse, CommandError> {
    oracle_ask_folder_inner(&runtime, &path, query).await
}
