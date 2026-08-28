import type {
  FileTab,
  IndexedFile,
  OracleHealth,
  OracleHealthCheck,
  OracleIndexProgress,
  OracleIndexStats,
  OracleIndexStatus,
  OracleSearchResponse,
} from "../../types/ipc";

/**
 * M1b mock boundary.
 *
 * Replace this module with typed IPC adapters when the Oracle daemon is wired.
 * The values below intentionally describe pointers and index state, never a
 * generated answer.
 */

export type OracleFileTab = FileTab;

export interface OracleFile extends Pick<IndexedFile, "path" | "chunks"> {
  updated_at: string;
}

export const ORACLE_INDEXED_FOLDER = "~/dev/devboule";
export const ORACLE_DATA_DIR = "oracle-data/";

export const ORACLE_INDEX_STATUS: OracleIndexStatus = {
  state: "indexing",
  indexed_files: 412,
  total_files: 1314,
  indexed_chunks: 4177,
  pending_files: 902,
  stale_files: 37,
  resource_budget: {
    max_cpu_percent: 20,
    max_memory_mb: 768,
    max_parallelism: 2,
  },
};

export const ORACLE_INDEX_PROGRESS: OracleIndexProgress = {
  state: "running",
  completed_files: 412,
  total_files: 1314,
  completed_chunks: 4177,
  total_chunks: 13000,
  percentage: 31,
  eta_seconds: 240,
  current_path: "src/features/workspace/Workspace.tsx",
};

export const ORACLE_STATS: OracleIndexStats = {
  indexed_files: ORACLE_INDEX_STATUS.indexed_files,
  indexed_chunks: ORACLE_INDEX_STATUS.indexed_chunks,
  pending_files: ORACLE_INDEX_STATUS.pending_files,
  stale_files: ORACLE_INDEX_STATUS.stale_files,
  backend: "sqlite + lance",
};

export const ORACLE_SEARCH_RESPONSE: OracleSearchResponse = {
  query: "where the workspace root is resolved",
  results: [
    {
      path: "crates/oracle-core/src/config.rs",
      line_start: 176,
      line_end: 196,
      snippet:
        "pub fn resolve_data_dir(root: &Path) -> PathBuf {\n    root.join(DEFAULT_ORACLE_DIR)\n}",
      score: 0.94,
      symbol_name: "resolve_data_dir",
      match_type: "dense+lexical",
    },
    {
      path: "crates/oracle-core/src/doctor.rs",
      line_start: 168,
      line_end: 188,
      snippet:
        'fn check_workspace(root: Option<&Path>, manifest_path: &Path) -> DoctorCheck {\n    let Some(root) = root else {\n        return check("workspace", false, "No indexed workspace");\n    };',
      score: 0.82,
      symbol_name: "check_workspace",
      match_type: "lexical",
    },
    {
      path: "crates/oracle-core/src/ingest/collect.rs",
      line_start: 610,
      line_end: 628,
      snippet:
        "pub fn collect_workspace(root: &Path) -> CollectedFiles {\n    let ignore_policy = load_workspace_ignore_policy(root);\n    collect_files(root, &ignore_policy)\n}",
      score: 0.76,
      symbol_name: "collect_workspace",
      match_type: "dense",
    },
    {
      path: "crates/oracle-core/src/query/redact.rs",
      line_start: 10,
      line_end: 22,
      snippet:
        'const SECRET_REDACTION: &str = "[redacted-secret]";\n\n/// Redact secret-looking tokens in chunk text.\npub fn redact_secret_tokens(text: &str) -> String {',
      score: 0.63,
      match_type: "lexical",
    },
  ],
};

export const ORACLE_HEALTH: OracleHealth = {
  state: "degraded",
  checks: getOracleChecks(false),
  message: "Run doctor to verify the embedder before starting a full index.",
};

export const ORACLE_FILES: Record<OracleFileTab, readonly OracleFile[]> = {
  indexed: [
    { path: "src/features/workspace/Workspace.tsx", chunks: 31, updated_at: "2m ago" },
    { path: "src/lib/tauri.ts", chunks: 12, updated_at: "2m ago" },
    { path: "crates/oracle-core/src/ingest/indexer.rs", chunks: 28, updated_at: "14m ago" },
    { path: "crates/oracle-core/src/query/engine.rs", chunks: 19, updated_at: "1h ago" },
  ],
  pending: [
    { path: "crates/oracle-core/src/store/lance.rs", chunks: 0, updated_at: "now" },
    { path: "crates/oracle-core/src/query/lexical.rs", chunks: 0, updated_at: "now" },
  ],
  stale: [
    { path: "crates/oracle-core/src/config.rs", chunks: 9, updated_at: "6d ago" },
    { path: "crates/oracle-core/src/ingest/collect.rs", chunks: 23, updated_at: "8d ago" },
    { path: "crates/oracle-core/src/doctor.rs", chunks: 6, updated_at: "12d ago" },
  ],
};

export const ORACLE_FILE_TABS: readonly {
  id: OracleFileTab;
  label: string;
  count: number;
}[] = [
  { id: "indexed", label: "Indexed", count: 412 },
  { id: "pending", label: "Pending", count: 902 },
  { id: "stale", label: "Stale", count: 37 },
];

export function getOracleChecks(doctorRun: boolean): OracleHealthCheck[] {
  const checks = [
    ["runtime", "ok"],
    ["embedder", doctorRun ? "ok" : "unknown"],
    ["workspace", "ok"],
    ["index", "ok"],
    ["watcher", "ok"],
  ] as const;

  return checks.map(([id, state]) => ({
    id,
    state,
    message:
      state === "ok" ? `${id} is available` : "Run doctor to verify the embedder before indexing",
  }));
}
