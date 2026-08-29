import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  isCommandError,
  oracleAsk,
  oracleDoctor,
  oracleFiles,
  oracleIndexCancel,
  oracleIndexStart,
  oracleModelDownloadCancel,
  oracleModelDownloadStart,
  oracleStats,
  oracleStatus,
  oracleWorkspaceGet,
  oracleWorkspaceSet,
  oracleWatchStart,
  oracleWatchStop,
} from "../../lib/tauri";
import type {
  FileTab,
  OracleIndexStats,
  OracleIndexStatus,
  OracleResult,
  OracleWorkspace,
} from "../../types/ipc";
import "./oracle.css";

const ORACLE_FILE_PANEL_ID = "oracle-file-panel";
const ORACLE_FILE_PAGE = 0;
const EMPTY_ORACLE_RESULTS: OracleResult[] = [];

const ORACLE_FILE_TABS: readonly { id: FileTab; label: string }[] = [
  { id: "indexed", label: "Indexed" },
  { id: "pending", label: "Pending" },
  { id: "stale", label: "Stale" },
];

type RequestState<T> =
  | { status: "loading" }
  | { status: "ready"; value: T }
  | { status: "error"; message: string };

type TrackedRequestState<T> = { status: "idle" } | RequestState<T>;

type WatchNotice = { kind: "unimplemented"; message: string } | { kind: "error"; message: string };

function normalizedLineRange(result: OracleResult): [number, number] {
  return [
    Math.min(result.line_start, result.line_end),
    Math.max(result.line_start, result.line_end),
  ];
}

function resultLineCount(result: OracleResult): number {
  const [start, end] = normalizedLineRange(result);
  if (import.meta.env.DEV && result.line_start > result.line_end) {
    console.warn("Oracle returned an inverted line range", {
      path: result.path,
      line_start: result.line_start,
      line_end: result.line_end,
    });
  }
  return Math.max(0, end - start + 1);
}

function formatCount(value: number): string {
  return value.toLocaleString("en-US").replaceAll(",", " ");
}

function formatMegabytes(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

function totalReadLines(results: OracleResult[]): number {
  const rangesByPath = new Map<string, Array<[number, number]>>();

  for (const result of results) {
    const ranges = rangesByPath.get(result.path) ?? [];
    ranges.push(normalizedLineRange(result));
    rangesByPath.set(result.path, ranges);
  }

  let total = 0;
  for (const ranges of rangesByPath.values()) {
    ranges.sort(([firstStart], [secondStart]) => firstStart - secondStart);
    let [start, end] = ranges[0];

    for (const [nextStart, nextEnd] of ranges.slice(1)) {
      if (nextStart <= end + 1) {
        end = Math.max(end, nextEnd);
      } else {
        total += Math.max(0, end - start + 1);
        start = nextStart;
        end = nextEnd;
      }
    }

    total += Math.max(0, end - start + 1);
  }

  return total;
}

function commandErrorMessage(error: unknown): string {
  if (isCommandError(error) && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Unknown Oracle error.";
}

function isUnimplemented(error: unknown): boolean {
  return isCommandError(error) && error.code === "unimplemented";
}

function useTrackedRequest<T>(
  request: () => Promise<T>,
  initialState: RequestState<T>,
  autoStart?: boolean,
): { state: RequestState<T>; run: (showLoadingState?: boolean) => void };
function useTrackedRequest<T>(
  request: () => Promise<T>,
  initialState: { status: "idle" },
  autoStart?: boolean,
): { state: TrackedRequestState<T>; run: (showLoadingState?: boolean) => void };
function useTrackedRequest<T>(
  request: () => Promise<T>,
  initialState: TrackedRequestState<T>,
  autoStart = false,
): { state: TrackedRequestState<T>; run: (showLoadingState?: boolean) => void } {
  const [state, setState] = useState<TrackedRequestState<T>>(initialState);
  const mountedRef = useRef(false);
  const requestIdRef = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const executeRequest = useCallback(
    (requestId: number) => {
      void Promise.resolve()
        .then(request)
        .then((value) => {
          if (mountedRef.current && requestIdRef.current === requestId) {
            setState({ status: "ready", value });
          }
        })
        .catch((error: unknown) => {
          if (mountedRef.current && requestIdRef.current === requestId) {
            setState({ status: "error", message: commandErrorMessage(error) });
          }
        });
    },
    [request],
  );

  const run = useCallback(
    (showLoadingState = true) => {
      const requestId = requestIdRef.current + 1;
      requestIdRef.current = requestId;
      if (showLoadingState) setState({ status: "loading" });
      executeRequest(requestId);
    },
    [executeRequest],
  );

  useEffect(() => {
    if (!autoStart) return;
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    executeRequest(requestId);
  }, [autoStart, executeRequest]);

  return { state, run };
}

function fileCount(tab: FileTab, stats: OracleIndexStats | null): string {
  if (!stats) return "—";
  const count =
    tab === "indexed"
      ? stats.indexed_files
      : tab === "pending"
        ? stats.pending_files
        : stats.stale_files;
  return formatCount(count);
}

function progressPercentage(status: OracleIndexStatus | null): number | null {
  if (!status || status.total_files <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((status.indexed_files / status.total_files) * 100)));
}

function modelProgressPercentage(status: OracleIndexStatus | null): number | null {
  const model = status?.model;
  if (!model?.bytes_total || model.bytes_total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((model.bytes_done / model.bytes_total) * 100)));
}

export function OraclePanel() {
  const [oracleQuery, setOracleQuery] = useState("");
  const [submittedQuery, setSubmittedQuery] = useState<string | null>(null);
  const [indexStarting, setIndexStarting] = useState(false);
  const [indexActionError, setIndexActionError] = useState<string | null>(null);
  const [watching, setWatching] = useState(false);
  const [watchBusy, setWatchBusy] = useState(false);
  const [watchNotice, setWatchNotice] = useState<WatchNotice | null>(null);
  const [fileTab, setFileTab] = useState<FileTab>("indexed");
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [workspaceActionError, setWorkspaceActionError] = useState<string | null>(null);
  const [cancelBusy, setCancelBusy] = useState(false);

  const mountedRef = useRef(false);
  const searchQueryRef = useRef("");

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      // React StrictMode performs a development-only mount/cleanup/remount.
      // Defer the signal one tick so that simulated cleanup does not cancel a
      // real download; a genuine unmount still cancels both long operations.
      window.setTimeout(() => {
        if (!mountedRef.current) {
          void oracleIndexCancel();
          void oracleModelDownloadCancel();
        }
      }, 0);
    };
  }, []);

  const searchRequest = useCallback(() => oracleAsk(searchQueryRef.current), []);
  const requestIndexedFiles = useCallback(() => oracleFiles("indexed", ORACLE_FILE_PAGE), []);
  const requestPendingFiles = useCallback(() => oracleFiles("pending", ORACLE_FILE_PAGE), []);
  const requestStaleFiles = useCallback(() => oracleFiles("stale", ORACLE_FILE_PAGE), []);
  const { state: searchState, run: runSearch } = useTrackedRequest(searchRequest, {
    status: "idle",
  });
  const { state: statusRequest, run: refreshStatus } = useTrackedRequest(
    oracleStatus,
    { status: "loading" },
    true,
  );
  const { state: doctorRequest, run: refreshDoctor } = useTrackedRequest(
    oracleDoctor,
    { status: "loading" },
    true,
  );
  const { state: statsRequest, run: refreshStats } = useTrackedRequest(
    oracleStats,
    { status: "loading" },
    true,
  );
  const { state: indexedFilesRequest, run: refreshIndexedFiles } = useTrackedRequest(
    requestIndexedFiles,
    { status: "loading" },
    true,
  );
  const { state: pendingFilesRequest, run: refreshPendingFiles } = useTrackedRequest(
    requestPendingFiles,
    { status: "loading" },
    true,
  );
  const { state: staleFilesRequest, run: refreshStaleFiles } = useTrackedRequest(
    requestStaleFiles,
    { status: "loading" },
    true,
  );
  const { state: workspaceRequest, run: refreshWorkspace } = useTrackedRequest<OracleWorkspace>(
    oracleWorkspaceGet,
    { status: "loading" },
    true,
  );
  const filesByTab = {
    indexed: indexedFilesRequest,
    pending: pendingFilesRequest,
    stale: staleFilesRequest,
  };
  const refreshFiles = useCallback(() => {
    refreshIndexedFiles();
    refreshPendingFiles();
    refreshStaleFiles();
  }, [refreshIndexedFiles, refreshPendingFiles, refreshStaleFiles]);

  useEffect(() => {
    if (workspaceRequest.status !== "ready" || !workspaceRequest.value.exists) return;
    void oracleModelDownloadStart()
      .then(() => refreshStatus(false))
      .catch((error: unknown) => {
        if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
      });
  }, [refreshStatus, workspaceRequest]);

  function submitOracleQuery() {
    const query = oracleQuery.trim();
    if (!query) return;

    searchQueryRef.current = query;
    setSubmittedQuery(query);
    runSearch();
  }

  function handleSearchSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    submitOracleQuery();
  }

  function handleQueryKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      submitOracleQuery();
    }
  }

  const handleChooseWorkspace = useCallback(async () => {
    if (workspaceBusy) return;
    setWorkspaceBusy(true);
    setWorkspaceActionError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose the folder Oracle should index",
      });
      if (typeof selected !== "string") return;
      await oracleWorkspaceSet(selected);
      refreshWorkspace();
      refreshStatus();
      refreshDoctor();
      refreshStats();
      refreshFiles();
    } catch (error: unknown) {
      if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
    } finally {
      if (mountedRef.current) setWorkspaceBusy(false);
    }
  }, [refreshDoctor, refreshFiles, refreshStats, refreshStatus, refreshWorkspace, workspaceBusy]);

  const handleCancelOperations = useCallback(() => {
    if (cancelBusy) return;
    setCancelBusy(true);
    void Promise.all([oracleIndexCancel(), oracleModelDownloadCancel()])
      .then(() => {
        if (!mountedRef.current) return;
        refreshStatus();
        refreshStats();
        refreshFiles();
      })
      .catch((error: unknown) => {
        if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
      })
      .finally(() => {
        if (mountedRef.current) setCancelBusy(false);
      });
  }, [cancelBusy, refreshFiles, refreshStats, refreshStatus]);

  const handleIndexStart = useCallback(() => {
    if (indexStarting) return;
    setIndexStarting(true);
    setIndexActionError(null);
    refreshStatus();
    refreshStats();
    refreshFiles();
    void oracleIndexStart()
      .then(() => {
        if (!mountedRef.current) return;
        refreshStatus();
        refreshStats();
        refreshFiles();
      })
      .catch((error: unknown) => {
        if (!mountedRef.current) return;
        setIndexActionError(commandErrorMessage(error));
        refreshStatus();
        refreshStats();
        refreshFiles();
      })
      .finally(() => {
        if (mountedRef.current) setIndexStarting(false);
      });
  }, [indexStarting, refreshFiles, refreshStats, refreshStatus]);

  const handleWatchAction = useCallback(
    (action: "start" | "stop") => {
      if (watchBusy) return;
      setWatchBusy(true);
      setWatchNotice(null);
      const command = action === "start" ? oracleWatchStart : oracleWatchStop;
      void command()
        .then(() => {
          if (mountedRef.current) setWatching(action === "start");
        })
        .catch((error: unknown) => {
          if (!mountedRef.current) return;
          if (isUnimplemented(error)) {
            setWatchNotice({
              kind: "unimplemented",
              message: "File watching isn't available yet.",
            });
          } else {
            setWatchNotice({ kind: "error", message: commandErrorMessage(error) });
          }
        })
        .finally(() => {
          if (mountedRef.current) setWatchBusy(false);
        });
    },
    [watchBusy],
  );

  const status = statusRequest.status === "ready" ? statusRequest.value : null;
  const stats = statsRequest.status === "ready" ? statsRequest.value : null;
  const workspace = workspaceRequest.status === "ready" ? workspaceRequest.value : null;
  const selectedFiles = filesByTab[fileTab];
  const searchResults =
    searchState.status === "ready" ? searchState.value.results : EMPTY_ORACLE_RESULTS;
  const totalResultLines = useMemo(() => totalReadLines(searchResults), [searchResults]);
  const percentage = progressPercentage(status);
  useEffect(() => {
    const poll = window.setInterval(() => {
      if (!mountedRef.current) return;
      refreshStatus(false);
      if (status?.state === "indexing") {
        refreshStats(false);
        refreshFiles();
      }
    }, 1500);
    return () => window.clearInterval(poll);
  }, [refreshFiles, refreshStats, refreshStatus, status?.state]);

  const modelPercentage = modelProgressPercentage(status);
  const allFilesReady = ORACLE_FILE_TABS.every((tab) => filesByTab[tab.id].status === "ready");
  const noFiles =
    allFilesReady &&
    ORACLE_FILE_TABS.every((tab) => {
      const request = filesByTab[tab.id];
      return request.status === "ready" && request.value.length === 0;
    });
  const indexIsEmpty =
    status !== null &&
    stats !== null &&
    noFiles &&
    status.total_files === 0 &&
    stats.indexed_files === 0 &&
    stats.indexed_chunks === 0 &&
    stats.pending_files === 0 &&
    stats.stale_files === 0;
  const doctorRunning = doctorRequest.status === "loading";
  const indexDisplayState = status?.state ?? statusRequest.status;
  const indexDotTone =
    indexDisplayState === "error"
      ? "stopped"
      : indexDisplayState === "loading"
        ? "starting"
        : indexDisplayState === "indexing"
          ? "indexing"
          : "running";

  return (
    <div className="oracle-panel">
      <div className="oracle-page-heading">
        <h2>Oracle pointers</h2>
        <p>Oracle finds the smallest useful code spans. It points; you read the source.</p>
      </div>

      <div className="oracle-health" aria-label="Oracle health" aria-live="polite">
        <span className="oracle-server-state">
          <span
            className={`oracle-server-dot oracle-server-dot-${indexDotTone}`}
            aria-hidden="true"
          />
          <span>index: {indexDisplayState}</span>
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span
          className={`oracle-health-state oracle-health-state-${
            doctorRequest.status === "ready" ? doctorRequest.value.state : doctorRequest.status
          }`}
        >
          health:{" "}
          {doctorRequest.status === "ready" ? doctorRequest.value.state : doctorRequest.status}
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-checks" aria-label="Oracle health checks">
          {doctorRequest.status === "ready" ? (
            doctorRequest.value.checks.map((check) => (
              <span className="oracle-check" key={check.id} title={check.message ?? check.id}>
                <span
                  className={`oracle-check-dot oracle-check-dot-${check.state}`}
                  aria-hidden="true"
                />
                <span>{check.id}</span>
              </span>
            ))
          ) : doctorRequest.status === "loading" ? (
            <span className="oracle-check">Loading checks…</span>
          ) : (
            <span className="oracle-check oracle-health-state-unavailable" role="alert">
              {doctorRequest.message}
            </span>
          )}
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-health-summary">
          {statsRequest.status === "ready" ? (
            `${formatCount(statsRequest.value.indexed_files)} files · ${formatCount(statsRequest.value.indexed_chunks)} chunks · ${statsRequest.value.backend}`
          ) : statsRequest.status === "loading" ? (
            "Loading index stats…"
          ) : (
            <span className="oracle-health-state-unavailable" role="alert">
              {statsRequest.message}
            </span>
          )}
        </span>
        <button
          className="oracle-button oracle-button-secondary oracle-doctor-button"
          type="button"
          onClick={() => refreshDoctor()}
          disabled={doctorRunning}
        >
          {doctorRunning ? "Running doctor…" : "Run doctor"}
        </button>
      </div>

      <form className="oracle-search" onSubmit={handleSearchSubmit}>
        <span className="oracle-search-mark" aria-hidden="true">
          ?
        </span>
        <input
          value={oracleQuery}
          onChange={(event) => setOracleQuery(event.target.value)}
          onKeyDown={handleQueryKeyDown}
          placeholder="Find code to read — e.g. where the workspace root is resolved"
          aria-label="Search Oracle pointers"
        />
        <button
          className="oracle-button oracle-button-primary"
          type="submit"
          disabled={searchState.status === "loading"}
        >
          {searchState.status === "loading" ? "Finding…" : "Find pointers"}
        </button>
      </form>

      <section className="oracle-results-card" aria-labelledby="oracle-results-title">
        <div className="oracle-results-heading">
          <div>
            <div className="oracle-eyebrow">Ranked code pointers</div>
            <h3 id="oracle-results-title">
              {submittedQuery ? `Results for “${submittedQuery}”` : "Search the Oracle index"}
            </h3>
          </div>
          <div
            className="oracle-reading-cost"
            aria-label={`${formatCount(totalResultLines)} lines to read`}
          >
            <strong>{formatCount(totalResultLines)}</strong>
            <span>lines to read</span>
          </div>
        </div>
        <p className="oracle-results-note">
          No generated answer. Each result is a source span, and the line count is the context cost
          you pay to inspect it. Overlapping spans count once in the total.
        </p>
        {searchState.status === "idle" && (
          <div className="oracle-state-message" role="status" aria-live="polite">
            Enter a query to find ranked source pointers.
          </div>
        )}
        {searchState.status === "loading" && (
          <div className="oracle-state-message" role="status" aria-live="polite">
            Finding pointers…
          </div>
        )}
        {searchState.status === "error" && (
          <div className="oracle-error-message" role="alert">
            {searchState.message}
          </div>
        )}
        {searchState.status === "ready" && searchResults.length === 0 && (
          <div className="oracle-empty-state" role="status" aria-live="polite">
            {indexIsEmpty
              ? "Oracle has no indexed files yet. Start an index pass to find pointers."
              : "No pointers matched this query."}
          </div>
        )}
        {searchState.status === "ready" && searchResults.length > 0 && (
          <ol className="oracle-result-list" aria-label="Ranked Oracle results" aria-live="polite">
            {searchResults.map((result, index) => {
              const lineCount = resultLineCount(result);
              return (
                <li
                  className="oracle-result"
                  key={`${result.path}:${result.line_start}:${result.line_end}:${index}`}
                  tabIndex={0}
                  aria-label={`${result.path}, lines ${result.line_start} to ${result.line_end}`}
                >
                  <div className="oracle-result-heading">
                    <span className="oracle-result-rank">
                      #{String(index + 1).padStart(2, "0")}
                    </span>
                    <code className="oracle-result-path">{result.path}</code>
                    <span className="oracle-result-range">
                      lines {result.line_start}–{result.line_end}
                    </span>
                  </div>
                  <pre className="oracle-result-snippet">
                    <code>{result.snippet}</code>
                  </pre>
                  <div className="oracle-result-reason" aria-label="Why this result was returned">
                    <span className="oracle-result-cost">{lineCount} lines to read</span>
                    {result.match_type && <span>match {result.match_type}</span>}
                    {result.symbol_name && <span>symbol {result.symbol_name}</span>}
                  </div>
                </li>
              );
            })}
          </ol>
        )}
      </section>

      <div className="oracle-section-heading">
        <span>Index</span>
        <span>workspace coverage, progress, and resource ceiling</span>
        <span className={`oracle-watch-badge oracle-watch-badge-${watching ? "watching" : "idle"}`}>
          {watching ? "watching" : "idle"}
        </span>
      </div>

      <div className="oracle-workspace-card">
        <div className="oracle-eyebrow">Oracle workspace</div>
        <div className="oracle-folder-row">
          <span className="oracle-path">
            {workspace?.path ?? "Choose an existing folder to start Oracle"}
          </span>
          <button
            className="oracle-button oracle-button-secondary oracle-change-button"
            type="button"
            onClick={() => void handleChooseWorkspace()}
            disabled={workspaceBusy || workspace?.editable === false}
          >
            {workspaceBusy ? "Choosing…" : workspace?.path ? "Change" : "Choose folder"}
          </button>
        </div>
        <div className="oracle-folder-meta">
          {workspace?.source === "environment"
            ? "Developer override: DEVBOULE_ORACLE_ROOT is active and takes precedence over the saved choice."
            : workspace?.exists
              ? "This folder is saved on this machine and is the source Oracle indexes."
              : "Choose a folder on this machine; Oracle saves the choice for the next launch."}
        </div>
        {workspaceRequest.status === "loading" && (
          <div className="oracle-state-message" role="status">
            Loading the saved Oracle folder…
          </div>
        )}
        {workspaceRequest.status === "error" && (
          <div className="oracle-error-message" role="alert">
            {workspaceRequest.message}
          </div>
        )}
        {workspaceActionError && (
          <div className="oracle-error-message" role="alert">
            {workspaceActionError}
          </div>
        )}
        {status?.model && status.model.state === "downloading" && (
          <div className="oracle-model-download" aria-live="polite">
            <div className="oracle-job-heading">
              <span>
                Downloading {status.model.model_id} · file {status.model.file_index || 1} /{" "}
                {status.model.total_files}
              </span>
              <span>{modelPercentage === null ? "—" : `${modelPercentage}%`}</span>
            </div>
            <div className="oracle-folder-meta">
              {status.model.file ?? "Preparing download…"} · expected at {status.model.directory}
            </div>
            {modelPercentage !== null && (
              <div
                className="oracle-progress-track"
                role="progressbar"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={modelPercentage}
                aria-label="Oracle model download progress"
              >
                <div className="oracle-progress-fill" style={{ width: `${modelPercentage}%` }} />
              </div>
            )}
            <div className="oracle-job-note">
              {status.model.message ?? `About ${formatMegabytes(status.model.approximate_bytes)}.`}
            </div>
          </div>
        )}
        {status?.model && ["missing", "failed", "cancelled"].includes(status.model.state) && (
          <div className="oracle-error-message" role="alert">
            {status.model.message ?? `Oracle needs ${status.model.model_id} before it can run.`}
            <button
              className="oracle-button oracle-button-secondary oracle-model-retry-button"
              type="button"
              onClick={() => {
                void oracleModelDownloadStart()
                  .then(() => refreshStatus())
                  .catch((error: unknown) => setWorkspaceActionError(commandErrorMessage(error)));
              }}
            >
              Retry download
            </button>
          </div>
        )}
        {status?.model?.state === "ready" && (
          <div className="oracle-folder-ok" role="status">
            {status.model.model_id} is ready ({formatMegabytes(status.model.approximate_bytes)}).
          </div>
        )}
        {indexIsEmpty && (
          <div className="oracle-empty-state" role="status" aria-live="polite">
            The Oracle index is empty. No files are indexed yet.
          </div>
        )}

        <div className="oracle-job" aria-label="Indexing progress" aria-live="polite">
          {statusRequest.status === "loading" && (
            <div className="oracle-state-message" role="status">
              Loading index status…
            </div>
          )}
          {statusRequest.status === "error" && (
            <div className="oracle-error-message" role="alert">
              {statusRequest.message}
            </div>
          )}
          {statusRequest.status === "ready" && (
            <>
              <div className="oracle-job-heading">
                <span>
                  {statusRequest.value.state} · {formatCount(statusRequest.value.indexed_files)} /{" "}
                  {formatCount(statusRequest.value.total_files)} files
                  <span className="oracle-muted"> · ETA unavailable from Oracle IPC</span>
                </span>
                <span>{percentage === null ? "—" : `${percentage}%`}</span>
              </div>
              {percentage === null ? (
                <div className="oracle-state-message" role="status">
                  Oracle has not reported any files for this index yet.
                </div>
              ) : (
                <div
                  className="oracle-progress-track"
                  role="progressbar"
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={percentage}
                  aria-label="Oracle indexing progress"
                >
                  <div className="oracle-progress-fill" style={{ width: `${percentage}%` }} />
                </div>
              )}
              <div className="oracle-job-note">
                {statusRequest.value.state === "indexing"
                  ? "Oracle is indexing the workspace."
                  : `Index state: ${statusRequest.value.state}.`}
              </div>
            </>
          )}
        </div>

        <div className="oracle-resource-cap" aria-label="Declared Oracle resource cap">
          {statusRequest.status === "ready" ? (
            <>
              <div>
                <div className="oracle-eyebrow">Declared resource cap</div>
                <div className="oracle-resource-copy">
                  Background work stays within these limits.
                </div>
              </div>
              <div className="oracle-resource-values">
                <span>
                  <strong>{statusRequest.value.resource_budget.max_cpu_percent}%</strong>
                  <small>CPU</small>
                </span>
                <span>
                  <strong>
                    {formatCount(statusRequest.value.resource_budget.max_memory_mb)} MB
                  </strong>
                  <small>memory</small>
                </span>
                <span>
                  <strong>{statusRequest.value.resource_budget.max_parallelism}</strong>
                  <small>workers</small>
                </span>
              </div>
            </>
          ) : statusRequest.status === "loading" ? (
            <div className="oracle-state-message" role="status">
              Loading resource cap…
            </div>
          ) : (
            <div className="oracle-error-message" role="alert">
              {statusRequest.message}
            </div>
          )}
        </div>

        <div className="oracle-stats" aria-live="polite">
          {statsRequest.status === "ready" ? (
            <>
              <div className="oracle-stat">
                <div className="oracle-stat-value">
                  {formatCount(statsRequest.value.indexed_files)}
                </div>
                <div className="oracle-stat-label">Indexed</div>
              </div>
              <div className="oracle-stat">
                <div className="oracle-stat-value">
                  {formatCount(statsRequest.value.indexed_chunks)}
                </div>
                <div className="oracle-stat-label">Chunks</div>
              </div>
              <div className="oracle-stat">
                <div className="oracle-stat-value oracle-stat-value-warning">
                  {formatCount(statsRequest.value.pending_files)}
                </div>
                <div className="oracle-stat-label">Pending</div>
              </div>
              <div className="oracle-stat">
                <div className="oracle-stat-value oracle-stat-value-warning">
                  {formatCount(statsRequest.value.stale_files)}
                </div>
                <div className="oracle-stat-label">Stale</div>
              </div>
              <div className="oracle-stat">
                <div className="oracle-stat-value oracle-stat-value-ink-soft">
                  {statsRequest.value.backend}
                </div>
                <div className="oracle-stat-label">Backend</div>
              </div>
            </>
          ) : statsRequest.status === "loading" ? (
            <div className="oracle-state-message" role="status">
              Loading Oracle stats…
            </div>
          ) : (
            <div className="oracle-error-message" role="alert">
              {statsRequest.message}
            </div>
          )}
        </div>

        <div className="oracle-actions">
          <button
            className="oracle-button oracle-button-primary"
            type="button"
            onClick={handleIndexStart}
            disabled={indexStarting || status?.state === "indexing" || !workspace?.exists}
          >
            {indexStarting ? "Starting index…" : "Index now"}
          </button>
          {(status?.state === "indexing" || status?.model.state === "downloading") && (
            <button
              className="oracle-button oracle-button-secondary oracle-stop-button"
              type="button"
              onClick={handleCancelOperations}
              disabled={cancelBusy}
            >
              {cancelBusy ? "Cancelling…" : "Cancel"}
            </button>
          )}
          <button
            className="oracle-button oracle-button-secondary"
            type="button"
            onClick={() => handleWatchAction("start")}
            disabled={watchBusy || watching}
          >
            {watchBusy ? "Working…" : "Watch"}
          </button>
          <button
            className="oracle-button oracle-button-secondary oracle-stop-button"
            type="button"
            onClick={() => handleWatchAction("stop")}
            disabled={watchBusy}
          >
            Stop
          </button>
        </div>
        {indexActionError && (
          <div className="oracle-error-message" role="alert">
            {indexActionError}
          </div>
        )}
        {watchNotice && (
          <div
            className={
              watchNotice.kind === "unimplemented" ? "oracle-watch-notice" : "oracle-error-message"
            }
            role={watchNotice.kind === "unimplemented" ? "status" : "alert"}
            aria-live="polite"
          >
            {watchNotice.message}
          </div>
        )}
      </div>

      <div className="oracle-file-heading">
        <div className="oracle-file-tabs" role="tablist" aria-label="Oracle files">
          {ORACLE_FILE_TABS.map((tab) => (
            <button
              className={`oracle-file-tab${fileTab === tab.id ? " oracle-file-tab-active" : ""}`}
              type="button"
              role="tab"
              aria-selected={fileTab === tab.id}
              aria-controls={ORACLE_FILE_PANEL_ID}
              id={`oracle-file-tab-${tab.id}`}
              key={tab.id}
              onClick={() => setFileTab(tab.id)}
            >
              <span>{tab.label}</span>
              <span>{fileCount(tab.id, stats)}</span>
            </button>
          ))}
        </div>
        <span className="oracle-filter-pill">Filter files</span>
      </div>

      <div
        id={ORACLE_FILE_PANEL_ID}
        className="oracle-file-list"
        role="tabpanel"
        aria-label="Oracle files"
        aria-live="polite"
      >
        {selectedFiles.status === "loading" && (
          <div className="oracle-state-message" role="status">
            Loading {fileTab} files…
          </div>
        )}
        {selectedFiles.status === "error" && (
          <div className="oracle-error-message" role="alert">
            {selectedFiles.message}
          </div>
        )}
        {selectedFiles.status === "ready" && selectedFiles.value.length === 0 && (
          <div className="oracle-empty-state" role="status">
            No {fileTab} files reported by Oracle.
          </div>
        )}
        {selectedFiles.status === "ready" &&
          selectedFiles.value.map((file) => (
            <div className="oracle-file-row" key={file.path}>
              <span className="oracle-file-path">{file.path}</span>
              <span className="oracle-file-detail">{file.chunks} chunks</span>
              <span className="oracle-file-when">{file.updated_at}</span>
            </div>
          ))}
        <div className="oracle-page-label">
          {selectedFiles.status === "ready" && selectedFiles.value.length > 0
            ? "Showing the first page of Oracle files."
            : ""}
        </div>
      </div>
    </div>
  );
}
