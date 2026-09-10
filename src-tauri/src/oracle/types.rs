//! Wire types exchanged with the Oracle panel: the workspace descriptor, the
//! model status, the index status and health reports, and the search results.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleWorkspace {
    pub path: Option<String>,
    pub source: String,
    pub exists: bool,
    pub editable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleModelState {
    NotApplicable,
    Missing,
    Downloading,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleModelStatus {
    pub state: OracleModelState,
    pub model_id: String,
    pub directory: String,
    pub file: Option<String>,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub approximate_bytes: u64,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleResourceBudget {
    pub max_cpu_percent: f64,
    pub max_memory_mb: f64,
    pub max_parallelism: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleIndexStatus {
    pub state: String,
    pub indexed_files: usize,
    pub total_files: usize,
    pub indexed_chunks: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub resource_budget: OracleResourceBudget,
    pub model: OracleModelStatus,
    pub reranker: Option<OracleModelStatus>,
    /// Explanation for an index that is incomplete or currently waiting on a
    /// resource, such as available memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleHealthCheck {
    pub id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleHealth {
    pub state: String,
    pub checks: Vec<OracleHealthCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleIndexStats {
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub backend: String,
}

/// Where an arbitrary folder's Oracle index stands.
///
/// `Unreadable` is deliberately distinct from `NeverIndexed`: a folder whose
/// index cannot be read is not an empty index, and reporting "nothing indexed"
/// there would invite a full re-index over a store that may be intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleFolderIndexState {
    NeverIndexed,
    Partial,
    Ready,
    Unreadable,
}

/// The answer to "does this folder have an index, and how complete is it?".
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleFolderIndexStatus {
    /// The folder that was probed, canonicalized when the filesystem allowed.
    pub path: String,
    /// The Oracle data directory derived for that folder.
    pub data_dir: String,
    pub state: OracleFolderIndexState,
    pub indexed_files: usize,
    /// Expected indexable files. Zero for a folder with no index artifacts,
    /// which is not walked: the probe answers without a traversal when there
    /// is nothing to measure.
    pub total_files: usize,
    pub pending_files: usize,
    pub stale_files: usize,
    pub indexed_chunks: usize,
    /// Why the folder is not fully indexed. `None` only when `state` is
    /// `ready`; caller errors reject instead of being described here.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileTab {
    Indexed,
    Pending,
    Stale,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct IndexedFile {
    pub path: String,
    pub chunks: usize,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleResult {
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    /// The narrower span inside `[line_start, line_end]` that the cross-encoder
    /// scored as the answer, when it could pick one. It is a suggestion about
    /// where to look first, not a replacement for the range: `snippet` still
    /// carries the whole chunk, so a caller that disagrees loses nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_line_start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_line_end: Option<usize>,
    pub snippet: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_type: Option<OracleMatchType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OracleMatchType {
    #[serde(rename = "lexical")]
    Lexical,
    #[serde(rename = "dense")]
    Dense,
    #[serde(rename = "dense+lexical")]
    DenseLexical,
    #[serde(rename = "dense+reranked")]
    DenseReranked,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OracleSearchResponse {
    pub query: String,
    pub results: Vec<OracleResult>,
}
