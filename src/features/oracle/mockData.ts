import type { IndexedFile } from '../../types/ipc';

/**
 * M1b mock boundary.
 *
 * Replace this module with typed IPC adapters when the Oracle daemon is wired.
 */

export type OracleFileTab = 'indexed' | 'pending' | 'stale';

export interface OracleFile extends Pick<IndexedFile, 'path' | 'chunks'> {
  when: string;
}

export interface OracleCheck {
  id: string;
  ok: boolean;
  title: string;
}

export const ORACLE_ANSWER =
  'The workspace root is resolved once, in the index preferences: indexRoot wins, and the dense-index status root is the fallback. Everything downstream — the watcher, the chunk store and every citation path — is relative to that one value, which is why a folder change kicks a full index job and restarts the watcher.';

export const ORACLE_FILE_COUNT = '1 314';
export const ORACLE_CHUNK_COUNT = '4 177';
export const ORACLE_BACKEND = 'lancedb';
export const ORACLE_INDEXED_FOLDER = '~/dev/devboule';
export const ORACLE_DATA_DIR = 'oracle-data/';
export const ORACLE_INDEXING_PROGRESS = {
  indexed: '412',
  expected: '1 314',
  percentage: '31%',
  eta: 'about 4 min left',
};
export const ORACLE_LLM = {
  readyLine: 'openrouter · qwen3-8b · answers written remotely',
  readyAnswerBy: 'openrouter · qwen3-8b · dense+rerank · 3 sources',
  missingLine: 'no provider — retrieval only',
  missingAnswerBy: 'retrieval only · 3 sources',
} as const;

export const ORACLE_CITATIONS = [
  { label: 'src-tauri/src/oracle/prefs.rs:1841-2360', title: 'src-tauri/src/oracle/prefs.rs#chunk-12' },
  { label: 'oracle/server/index_status.py:604-1180', title: 'oracle/server/index_status.py#chunk-3' },
  { label: 'src/components/oracle/OracleAdminPanel.tsx:9042-9610', title: 'src/components/oracle/OracleAdminPanel.tsx#chunk-27' },
] as const;

export const ORACLE_FILES: Record<OracleFileTab, readonly OracleFile[]> = {
  indexed: [
    { path: 'src/components/views/WorkspaceView.tsx', chunks: 31, when: '2m ago' },
    { path: 'src-tauri/src/oracle/prefs.rs', chunks: 12, when: '2m ago' },
    { path: 'oracle/server/index_status.py', chunks: 8, when: '14m ago' },
    { path: 'devboule-mcp/src/tools.rs', chunks: 19, when: '1h ago' },
  ],
  pending: [],
  stale: [
    { path: 'src-tauri/src/oracle/mod.rs', chunks: 14, when: '6d ago' },
    { path: 'oracle-core/src/lib.rs', chunks: 9, when: '8d ago' },
    { path: 'rig/world.py', chunks: 6, when: '12d ago' },
  ],
};

export const ORACLE_FILE_TABS: readonly {
  id: OracleFileTab;
  label: string;
  count: string;
}[] = [
  { id: 'indexed', label: 'Indexed', count: '1 314' },
  { id: 'pending', label: 'Pending', count: '0' },
  { id: 'stale', label: 'Stale', count: '37' },
];

export const ORACLE_STATS = [
  { label: 'Files', value: '1 314', kind: 'normal' },
  { label: 'Vectors', value: '4 177', kind: 'normal' },
  { label: 'Pending', value: '0', kind: 'normal' },
  { label: 'Chunks', value: '4 177', kind: 'normal' },
  { label: 'Stale', value: '37', kind: 'warning' },
  { label: 'Backend', value: 'lancedb', kind: 'ink-soft' },
] as const;

export function getOracleChecks(doctorRun: boolean, providerKeyPresent: boolean): OracleCheck[] {
  const checks = [
    ['runtime', true],
    ['embedder', doctorRun],
    ['workspace', true],
    ['index', true],
    ['live_server', true],
    ['provider', providerKeyPresent],
  ] as const;

  return checks.map(([id, ok]) => ({
    id,
    ok,
    title: `${id} · ${ok ? 'ok' : doctorRun ? 'failed' : 'run doctor for the full check'}`,
  }));
}
