import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  IndexedFile,
  OracleHealth,
  OracleIndexStats,
  OracleIndexStatus,
  OracleSearchResponse,
  OracleWorkspace,
} from "../../types/ipc";
import { OracleAdmin, type WatchNotice } from "./OracleAdmin";
import { OracleSearch } from "./OracleSearch";
import { OracleSetup } from "./OracleSetup";
import { useTrackedRequest, type TrackedRequestState } from "./oracleRequests";
import {
  commandErrorMessage,
  formatCount,
  getOracleStage,
  isIndexEmpty,
  type OracleStage,
} from "./oracleUtils";
import "./oracle.css";

const ORACLE_FILE_PAGE = 0;

export function OraclePanel() {
  const [searchQuery, setSearchQuery] = useState("");
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
  const [adminOpen, setAdminOpen] = useState(false);

  const mountedRef = useRef(false);
  const searchQueryRef = useRef("");
  const modelDownloadWorkspaceRef = useRef<string | null>(null);
  const modelDownloadInFlightRef = useRef(false);
  const [modelDownloadBusy, setModelDownloadBusy] = useState(false);

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

  const { state: workspaceRequest, run: refreshWorkspace } = useTrackedRequest<OracleWorkspace>(
    oracleWorkspaceGet,
    { status: "loading" },
    true,
  );
  const workspace = workspaceRequest.status === "ready" ? workspaceRequest.value : null;
  const workspaceExists = workspace?.exists === true;

  const { state: statusRequest, run: refreshStatus } = useTrackedRequest<OracleIndexStatus>(
    oracleStatus,
    { status: "idle" },
    workspaceExists,
  );
  const { state: doctorRequest, run: refreshDoctor } = useTrackedRequest<OracleHealth>(
    oracleDoctor,
    { status: "idle" },
    workspaceExists,
  );
  const { state: statsRequest, run: refreshStats } = useTrackedRequest<OracleIndexStats>(
    oracleStats,
    { status: "idle" },
    workspaceExists,
  );

  const searchRequest = useCallback(() => oracleAsk(searchQueryRef.current), []);
  const {
    state: searchState,
    run: runSearch,
    reset: resetSearch,
  } = useTrackedRequest<OracleSearchResponse>(searchRequest, { status: "idle" });

  const submitOracleQuery = useCallback(
    (query: string) => {
      searchQueryRef.current = query;
      setSubmittedQuery(query);
      runSearch();
    },
    [runSearch],
  );

  const requestIndexedFiles = useCallback(() => oracleFiles("indexed", ORACLE_FILE_PAGE), []);
  const requestPendingFiles = useCallback(() => oracleFiles("pending", ORACLE_FILE_PAGE), []);
  const requestStaleFiles = useCallback(() => oracleFiles("stale", ORACLE_FILE_PAGE), []);
  const { state: indexedFilesState, run: refreshIndexedFiles } = useTrackedRequest<IndexedFile[]>(
    requestIndexedFiles,
    { status: "idle" },
    workspaceExists && adminOpen,
  );
  const { state: pendingFilesState, run: refreshPendingFiles } = useTrackedRequest<IndexedFile[]>(
    requestPendingFiles,
    { status: "idle" },
    workspaceExists && adminOpen,
  );
  const { state: staleFilesState, run: refreshStaleFiles } = useTrackedRequest<IndexedFile[]>(
    requestStaleFiles,
    { status: "idle" },
    workspaceExists && adminOpen,
  );
  const filesByTab = useMemo<Record<FileTab, typeof indexedFilesState>>(
    () => ({
      indexed: indexedFilesState,
      pending: pendingFilesState,
      stale: staleFilesState,
    }),
    [indexedFilesState, pendingFilesState, staleFilesState],
  );

  const refreshFiles = useCallback(() => {
    refreshIndexedFiles();
    refreshPendingFiles();
    refreshStaleFiles();
  }, [refreshIndexedFiles, refreshPendingFiles, refreshStaleFiles]);

  const handleModelDownload = useCallback(() => {
    if (!mountedRef.current || modelDownloadInFlightRef.current) return;
    modelDownloadInFlightRef.current = true;
    setModelDownloadBusy(true);
    if (mountedRef.current) setWorkspaceActionError(null);
    void oracleModelDownloadStart()
      .then(() => refreshStatus())
      .catch((error: unknown) => {
        if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
      })
      .finally(() => {
        modelDownloadInFlightRef.current = false;
        if (mountedRef.current) setModelDownloadBusy(false);
      });
  }, [refreshStatus]);

  const workspacePath = workspaceExists ? workspace?.path : null;
  useEffect(() => {
    if (!workspacePath) {
      modelDownloadWorkspaceRef.current = null;
      return;
    }
    if (modelDownloadWorkspaceRef.current === workspacePath) return;
    modelDownloadWorkspaceRef.current = workspacePath;
    handleModelDownload();
  }, [handleModelDownload, workspacePath]);

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
      setIndexActionError(null);
      setSubmittedQuery(null);
      resetSearch();
      setAdminOpen(false);
      await oracleWorkspaceSet(selected);
      refreshWorkspace();
      refreshStatus();
      refreshDoctor();
      refreshStats();
    } catch (error: unknown) {
      if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
    } finally {
      if (mountedRef.current) setWorkspaceBusy(false);
    }
  }, [refreshDoctor, refreshStats, refreshStatus, refreshWorkspace, resetSearch, workspaceBusy]);

  const handleCancelOperations = useCallback(() => {
    if (cancelBusy) return;
    setCancelBusy(true);
    void Promise.all([oracleIndexCancel(), oracleModelDownloadCancel()])
      .then(() => {
        if (!mountedRef.current) return;
        refreshStatus();
        refreshStats();
        if (adminOpen) refreshFiles();
      })
      .catch((error: unknown) => {
        if (mountedRef.current) setWorkspaceActionError(commandErrorMessage(error));
      })
      .finally(() => {
        if (mountedRef.current) setCancelBusy(false);
      });
  }, [adminOpen, cancelBusy, refreshFiles, refreshStats, refreshStatus]);

  const handleIndexStart = useCallback(() => {
    if (indexStarting) return;
    setIndexStarting(true);
    setIndexActionError(null);
    setSubmittedQuery(null);
    resetSearch();
    setAdminOpen(false);
    refreshStatus();
    refreshStats();
    void oracleIndexStart()
      .then(() => {
        if (!mountedRef.current) return;
        refreshStatus();
        refreshStats();
      })
      .catch((error: unknown) => {
        if (!mountedRef.current) return;
        setIndexActionError(commandErrorMessage(error));
        refreshStatus();
        refreshStats();
      })
      .finally(() => {
        if (mountedRef.current) setIndexStarting(false);
      });
  }, [indexStarting, refreshStats, refreshStatus, resetSearch]);

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
          if (isCommandError(error) && error.code === "unimplemented") {
            setWatchNotice({
              kind: "unimplemented",
              message: "File watching is not available yet.",
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
  const stage = getOracleStage({ workspaceRequest, statusRequest });
  const indexEmpty = isIndexEmpty(stats, status);
  const chooseWorkspace = useCallback(() => {
    void handleChooseWorkspace();
  }, [handleChooseWorkspace]);
  const refreshStatusAction = useCallback(() => {
    refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    const poll = window.setInterval(() => {
      if (!mountedRef.current || !workspaceExists) return;
      refreshStatus(false);
      if (status?.state === "indexing") {
        refreshStats(false);
        if (adminOpen) refreshFiles();
      }
    }, 1500);
    return () => window.clearInterval(poll);
  }, [adminOpen, refreshFiles, refreshStats, refreshStatus, status?.state, workspaceExists]);

  const handleAdminToggle = useCallback((open: boolean) => {
    setAdminOpen(open);
  }, []);

  return (
    <div className="oracle-panel">
      <header className="oracle-page-heading">
        <div className="oracle-eyebrow">Local code search</div>
        <h2>Oracle</h2>
        <p>Ask where code lives and get the smallest useful source spans to open.</p>
      </header>

      {stage === "ready" || stage === "incomplete" ? (
        <ReadyOracle
          workspace={workspace}
          status={status}
          stats={stats}
          searchState={searchState}
          searchQuery={searchQuery}
          submittedQuery={submittedQuery}
          indexEmpty={indexEmpty}
          reranker={status?.reranker ?? null}
          adminOpen={adminOpen}
          statusRequest={statusRequest}
          doctorRequest={doctorRequest}
          statsRequest={statsRequest}
          filesByTab={filesByTab}
          fileTab={fileTab}
          watching={watching}
          watchBusy={watchBusy}
          watchNotice={watchNotice}
          workspaceBusy={workspaceBusy}
          indexStarting={indexStarting}
          cancelBusy={cancelBusy}
          modelDownloadBusy={modelDownloadBusy}
          incomplete={stage === "incomplete"}
          onSearch={submitOracleQuery}
          onSearchQueryChange={setSearchQuery}
          onChooseWorkspace={chooseWorkspace}
          onStartIndex={handleIndexStart}
          onCancel={handleCancelOperations}
          onWatch={handleWatchAction}
          onRunDoctor={refreshDoctor}
          onRetryReranker={handleModelDownload}
          onAdminToggle={handleAdminToggle}
          onFileTabChange={setFileTab}
        />
      ) : (
        <OracleSetup
          stage={stage as Exclude<OracleStage, "ready" | "incomplete">}
          workspaceRequest={workspaceRequest}
          statusRequest={statusRequest}
          workspaceBusy={workspaceBusy}
          indexStarting={indexStarting}
          cancelBusy={cancelBusy}
          modelDownloadBusy={modelDownloadBusy}
          workspaceActionError={workspaceActionError}
          indexActionError={indexActionError}
          onChooseWorkspace={chooseWorkspace}
          onStartIndex={handleIndexStart}
          onCancel={handleCancelOperations}
          onRefreshStatus={refreshStatusAction}
          onRetryModels={handleModelDownload}
        />
      )}
    </div>
  );
}

interface ReadyOracleProps {
  workspace: OracleWorkspace | null;
  status: OracleIndexStatus | null;
  stats: OracleIndexStats | null;
  searchState: TrackedRequestState<OracleSearchResponse>;
  searchQuery: string;
  submittedQuery: string | null;
  indexEmpty: boolean;
  reranker: OracleIndexStatus["reranker"];
  adminOpen: boolean;
  statusRequest: TrackedRequestState<OracleIndexStatus>;
  doctorRequest: TrackedRequestState<OracleHealth>;
  statsRequest: TrackedRequestState<OracleIndexStats>;
  filesByTab: Record<FileTab, TrackedRequestState<IndexedFile[]>>;
  fileTab: FileTab;
  watching: boolean;
  watchBusy: boolean;
  watchNotice: WatchNotice | null;
  workspaceBusy: boolean;
  indexStarting: boolean;
  cancelBusy: boolean;
  modelDownloadBusy: boolean;
  incomplete: boolean;
  onSearch: (query: string) => void;
  onSearchQueryChange: (query: string) => void;
  onChooseWorkspace: () => void;
  onStartIndex: () => void;
  onCancel: () => void;
  onWatch: (action: "start" | "stop") => void;
  onRunDoctor: () => void;
  onRetryReranker: () => void;
  onAdminToggle: (open: boolean) => void;
  onFileTabChange: (tab: FileTab) => void;
}

const ReadyOracle = memo(function ReadyOracle({
  workspace,
  status,
  stats,
  searchState,
  searchQuery,
  submittedQuery,
  indexEmpty,
  reranker,
  adminOpen,
  statusRequest,
  doctorRequest,
  statsRequest,
  filesByTab,
  fileTab,
  watching,
  watchBusy,
  watchNotice,
  workspaceBusy,
  indexStarting,
  cancelBusy,
  modelDownloadBusy,
  incomplete,
  onSearch,
  onSearchQueryChange,
  onChooseWorkspace,
  onStartIndex,
  onCancel,
  onWatch,
  onRunDoctor,
  onRetryReranker,
  onAdminToggle,
  onFileTabChange,
}: ReadyOracleProps) {
  return (
    <>
      <div className="oracle-ready-context">
        <div>
          <span className="oracle-ready-dot" aria-hidden="true" />
          <strong>{incomplete ? "Oracle has a partial index" : "Oracle is ready"}</strong>
          <code title={workspace?.path ?? undefined}>{workspace?.path ?? "Selected folder"}</code>
        </div>
        <span
          className={`oracle-index-state oracle-index-state-${incomplete ? "incomplete" : (status?.state ?? "unknown")}`}
        >
          {incomplete
            ? `${status?.indexed_files ?? 0} of ${status?.total_files ?? 0} files indexed`
            : status?.state === "stale"
              ? "Index needs a refresh"
              : `${stats?.indexed_files ?? status?.indexed_files ?? 0} files indexed`}
        </span>
      </div>
      {incomplete && status && (
        <div className="oracle-incomplete-notice" role="status">
          <div>
            <strong>
              {status.pause_reason ? "Indexing is paused for memory." : "Indexing is incomplete."}
            </strong>
            <span>
              {status.pause_reason ?? (
                <>
                  Search is available across {formatCount(status.indexed_files)} indexed files, but{" "}
                  {formatCount(status.pending_files)} files still need to be indexed.
                </>
              )}
            </span>
            <small>
              {status.pause_reason && status.state === "indexing"
                ? "Close memory-heavy apps; Oracle will resume automatically when memory recovers."
                : "Resume continues from the existing index; it does not start over."}
            </small>
          </div>
          <button
            className="oracle-button oracle-button-primary"
            type="button"
            onClick={onStartIndex}
            disabled={indexStarting || status.state === "indexing"}
          >
            {status.pause_reason && status.state === "indexing"
              ? "Waiting for memory…"
              : indexStarting
                ? "Resuming index…"
                : "Resume indexing"}
          </button>
        </div>
      )}
      <OracleSearch
        searchState={searchState}
        query={searchQuery}
        onQueryChange={onSearchQueryChange}
        submittedQuery={submittedQuery}
        stats={stats}
        indexIsEmpty={indexEmpty}
        reranker={reranker}
        onSearch={onSearch}
        onRetryReranker={onRetryReranker}
        retryDisabled={modelDownloadBusy}
      />
      <OracleAdmin
        open={adminOpen}
        onToggle={onAdminToggle}
        workspace={workspace}
        statusRequest={statusRequest}
        doctorRequest={doctorRequest}
        statsRequest={statsRequest}
        reranker={reranker}
        modelDownloadBusy={modelDownloadBusy}
        filesByTab={filesByTab}
        fileTab={fileTab}
        watching={watching}
        watchBusy={watchBusy}
        watchNotice={watchNotice}
        workspaceBusy={workspaceBusy}
        indexStarting={indexStarting}
        cancelBusy={cancelBusy}
        onChooseWorkspace={onChooseWorkspace}
        onStartIndex={onStartIndex}
        onCancel={onCancel}
        onWatch={onWatch}
        onRunDoctor={onRunDoctor}
        onRetryReranker={onRetryReranker}
        onFileTabChange={onFileTabChange}
      />
    </>
  );
});
