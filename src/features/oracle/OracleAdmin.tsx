import type {
  FileTab,
  IndexedFile,
  OracleHealth,
  OracleIndexStats,
  OracleIndexStatus,
  OracleModelStatus,
  OracleWorkspace,
} from "../../types/ipc";
import type { TrackedRequestState } from "./oracleRequests";
import { fileCount, formatCount, modelStateLabel } from "./oracleUtils";
import { RerankerStatus } from "./OracleSearch";

const FILE_TABS: readonly { id: FileTab; label: string }[] = [
  { id: "indexed", label: "Indexed" },
  { id: "pending", label: "Pending" },
  { id: "stale", label: "Stale" },
];

interface OracleAdminProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  workspace: OracleWorkspace | null;
  statusRequest: TrackedRequestState<OracleIndexStatus>;
  doctorRequest: TrackedRequestState<OracleHealth>;
  statsRequest: TrackedRequestState<OracleIndexStats>;
  reranker: OracleModelStatus | null;
  modelDownloadBusy: boolean;
  filesByTab: Record<FileTab, TrackedRequestState<IndexedFile[]>>;
  fileTab: FileTab;
  watching: boolean;
  watchBusy: boolean;
  watchNotice: WatchNotice | null;
  workspaceBusy: boolean;
  indexStarting: boolean;
  cancelBusy: boolean;
  onChooseWorkspace: () => void;
  onStartIndex: () => void;
  onCancel: () => void;
  onWatch: (action: "start" | "stop") => void;
  onRunDoctor: () => void;
  onRetryReranker: () => void;
  onFileTabChange: (tab: FileTab) => void;
}

export type WatchNotice =
  | { kind: "unimplemented"; message: string }
  | { kind: "error"; message: string };

export function OracleAdmin({
  open,
  onToggle,
  workspace,
  statusRequest,
  doctorRequest,
  statsRequest,
  reranker,
  modelDownloadBusy,
  filesByTab,
  fileTab,
  watching,
  watchBusy,
  watchNotice,
  workspaceBusy,
  indexStarting,
  cancelBusy,
  onChooseWorkspace,
  onStartIndex,
  onCancel,
  onWatch,
  onRunDoctor,
  onRetryReranker,
  onFileTabChange,
}: OracleAdminProps) {
  const status = statusRequest.status === "ready" ? statusRequest.value : null;
  const stats = statsRequest.status === "ready" ? statsRequest.value : null;
  const incomplete = status?.state === "incomplete" || (status?.pending_files ?? 0) > 0;
  const files = filesByTab[fileTab];
  return (
    <details
      className="oracle-admin"
      open={open}
      onToggle={(event) => onToggle(event.currentTarget.open)}
    >
      <summary>
        <span className="oracle-admin-summary-copy">
          <span className="oracle-eyebrow">Administration</span>
          <strong>Index, models & diagnostics</strong>
        </span>
        <span className="oracle-admin-summary-meta">
          {stats ? `${formatCount(stats.indexed_files)} files indexed` : "Index details"}
          {status?.state === "stale" && " · stale"}
          {incomplete && " · incomplete"}
        </span>
        <span className="oracle-admin-summary-action">
          {open ? "Hide details" : "Show details"}
        </span>
      </summary>

      <div className="oracle-admin-body">
        <div className="oracle-admin-grid">
          <AdminOverview workspace={workspace} status={status} stats={stats} />
          <AdminHealth doctorRequest={doctorRequest} onRunDoctor={onRunDoctor} />
          <AdminModels
            reranker={reranker}
            model={status?.model ?? null}
            onRetryReranker={onRetryReranker}
            retryDisabled={modelDownloadBusy}
          />
        </div>

        <OracleAdminActions
          status={status}
          watching={watching}
          watchBusy={watchBusy}
          workspaceBusy={workspaceBusy}
          indexStarting={indexStarting}
          cancelBusy={cancelBusy}
          watchNotice={watchNotice}
          onChooseWorkspace={onChooseWorkspace}
          onStartIndex={onStartIndex}
          onCancel={onCancel}
          onWatch={onWatch}
        />

        <OracleFiles fileTab={fileTab} files={files} stats={stats} onTabChange={onFileTabChange} />
      </div>
    </details>
  );
}

function AdminOverview({
  workspace,
  status,
  stats,
}: {
  workspace: OracleWorkspace | null;
  status: OracleIndexStatus | null;
  stats: OracleIndexStats | null;
}) {
  return (
    <section className="oracle-admin-block" aria-labelledby="oracle-admin-index-title">
      <div className="oracle-eyebrow">Index</div>
      <h4 id="oracle-admin-index-title">{status?.state ?? "status unavailable"}</h4>
      <dl className="oracle-admin-facts">
        <div>
          <dt>Folder</dt>
          <dd title={workspace?.path ?? undefined}>{workspace?.path ?? "Not selected"}</dd>
        </div>
        <div>
          <dt>Coverage</dt>
          <dd>
            {stats
              ? `${formatCount(stats.indexed_files)} files · ${formatCount(stats.indexed_chunks)} chunks`
              : "Loading…"}
          </dd>
        </div>
        <div>
          <dt>Backend</dt>
          <dd>{stats?.backend ?? "Loading…"}</dd>
        </div>
      </dl>
    </section>
  );
}

function AdminHealth({
  doctorRequest,
  onRunDoctor,
}: {
  doctorRequest: TrackedRequestState<OracleHealth>;
  onRunDoctor: () => void;
}) {
  return (
    <section className="oracle-admin-block" aria-labelledby="oracle-admin-health-title">
      <div className="oracle-admin-block-heading">
        <div>
          <div className="oracle-eyebrow">Diagnostics</div>
          <h4 id="oracle-admin-health-title">
            {doctorRequest.status === "ready" ? doctorRequest.value.state : "checking"}
          </h4>
        </div>
        <button
          className="oracle-button oracle-button-secondary"
          type="button"
          onClick={onRunDoctor}
          disabled={doctorRequest.status === "loading"}
        >
          {doctorRequest.status === "loading" ? "Checking…" : "Run doctor"}
        </button>
      </div>
      {doctorRequest.status === "ready" && (
        <div className="oracle-health-check-list">
          {doctorRequest.value.checks.map((check) => (
            <span className="oracle-check" key={check.id} title={check.message ?? check.id}>
              <span
                className={`oracle-check-dot oracle-check-dot-${check.state}`}
                aria-hidden="true"
              />
              {check.id}
            </span>
          ))}
        </div>
      )}
      {doctorRequest.status === "loading" && <p>Running local health checks…</p>}
      {doctorRequest.status === "error" && (
        <div className="oracle-error-message" role="alert">
          {doctorRequest.message}
        </div>
      )}
    </section>
  );
}

function AdminModels({
  model,
  reranker,
  onRetryReranker,
  retryDisabled,
}: {
  model: OracleModelStatus | null;
  reranker: OracleModelStatus | null;
  onRetryReranker: () => void;
  retryDisabled: boolean;
}) {
  return (
    <section className="oracle-admin-block" aria-labelledby="oracle-admin-models-title">
      <div className="oracle-eyebrow">Local models</div>
      <h4 id="oracle-admin-models-title">What powers retrieval</h4>
      <div className="oracle-model-facts">
        <span>
          <strong>Embedding</strong>
          {modelStateLabel(model)}
        </span>
        <span>
          <strong>Reranker</strong>
          {modelStateLabel(reranker)}
        </span>
      </div>
      <RerankerStatus status={reranker} onRetry={onRetryReranker} retryDisabled={retryDisabled} />
    </section>
  );
}

function OracleAdminActions({
  status,
  watching,
  watchBusy,
  workspaceBusy,
  indexStarting,
  cancelBusy,
  watchNotice,
  onChooseWorkspace,
  onStartIndex,
  onCancel,
  onWatch,
}: {
  status: OracleIndexStatus | null;
  watching: boolean;
  watchBusy: boolean;
  workspaceBusy: boolean;
  indexStarting: boolean;
  cancelBusy: boolean;
  watchNotice: WatchNotice | null;
  onChooseWorkspace: () => void;
  onStartIndex: () => void;
  onCancel: () => void;
  onWatch: (action: "start" | "stop") => void;
}) {
  const canResume = status?.state === "incomplete" || (status?.pending_files ?? 0) > 0;
  const waitingForMemory = Boolean(status?.pause_reason && status.state === "indexing");
  const canCancel =
    status?.state === "indexing" ||
    status?.model.state === "downloading" ||
    status?.reranker?.state === "downloading";
  return (
    <div className="oracle-admin-actions">
      <button
        className="oracle-button oracle-button-primary"
        type="button"
        onClick={onStartIndex}
        disabled={indexStarting || status?.state === "indexing"}
      >
        {waitingForMemory
          ? "Waiting for memory…"
          : indexStarting
          ? canResume
            ? "Resuming index…"
            : "Starting index…"
          : canResume
            ? "Resume indexing"
            : "Re-index folder"}
      </button>
      <button
        className="oracle-button oracle-button-secondary"
        type="button"
        onClick={onChooseWorkspace}
        disabled={workspaceBusy}
      >
        {workspaceBusy ? "Choosing…" : "Change folder"}
      </button>
      {canCancel && (
        <button
          className="oracle-button oracle-button-secondary oracle-stop-button"
          type="button"
          onClick={onCancel}
          disabled={cancelBusy}
        >
          {cancelBusy ? "Cancelling…" : "Cancel"}
        </button>
      )}
      <span className="oracle-action-spacer" />
      <button
        className="oracle-button oracle-button-secondary"
        type="button"
        onClick={() => onWatch("start")}
        disabled={watchBusy || watching}
      >
        {watchBusy ? "Working…" : "Watch changes"}
      </button>
      <button
        className="oracle-button oracle-button-secondary oracle-stop-button"
        type="button"
        onClick={() => onWatch("stop")}
        disabled={watchBusy || !watching}
      >
        Stop watching
      </button>
      {watchNotice && (
        <div
          className={
            watchNotice.kind === "unimplemented" ? "oracle-watch-notice" : "oracle-error-message"
          }
          role={watchNotice.kind === "unimplemented" ? "status" : "alert"}
        >
          {watchNotice.message}
        </div>
      )}
    </div>
  );
}

function OracleFiles({
  fileTab,
  files,
  stats,
  onTabChange,
}: {
  fileTab: FileTab;
  files: TrackedRequestState<IndexedFile[]>;
  stats: OracleIndexStats | null;
  onTabChange: (tab: FileTab) => void;
}) {
  return (
    <section className="oracle-files" aria-labelledby="oracle-files-title">
      <div className="oracle-files-heading">
        <div>
          <div className="oracle-eyebrow">File inventory</div>
          <h4 id="oracle-files-title">What Oracle has seen</h4>
        </div>
        <span className="oracle-files-note">First page only</span>
      </div>
      <div className="oracle-file-tabs" role="tablist" aria-label="Oracle files">
        {FILE_TABS.map((tab) => (
          <button
            className={`oracle-file-tab${fileTab === tab.id ? " oracle-file-tab-active" : ""}`}
            type="button"
            role="tab"
            aria-selected={fileTab === tab.id}
            aria-controls="oracle-file-panel"
            id={`oracle-file-tab-${tab.id}`}
            key={tab.id}
            onClick={() => onTabChange(tab.id)}
          >
            <span>{tab.label}</span>
            <span>{fileCount(tab.id, stats)}</span>
          </button>
        ))}
      </div>
      <div
        id="oracle-file-panel"
        className="oracle-file-list"
        role="tabpanel"
        aria-label="Oracle files"
        aria-live="polite"
      >
        {files.status === "loading" && (
          <div className="oracle-state-message" role="status">
            Loading {fileTab} files…
          </div>
        )}
        {files.status === "error" && (
          <div className="oracle-error-message" role="alert">
            {files.message}
          </div>
        )}
        {files.status === "ready" && files.value.length === 0 && (
          <div className="oracle-empty-state" role="status">
            No {fileTab} files reported by Oracle.
          </div>
        )}
        {files.status === "ready" &&
          files.value.length > 0 &&
          files.value.map((file) => (
            <div className="oracle-file-row" key={file.path}>
              <span className="oracle-file-path">{file.path}</span>
              <span className="oracle-file-detail">{file.chunks} chunks</span>
              <span className="oracle-file-when">{file.updated_at}</span>
            </div>
          ))}
      </div>
    </section>
  );
}
