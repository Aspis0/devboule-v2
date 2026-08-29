import { useMemo } from "react";
import type { ReactNode } from "react";
import type { OracleIndexStatus, OracleModelStatus, OracleWorkspace } from "../../types/ipc";
import type { TrackedRequestState } from "./oracleRequests";
import {
  formatCount,
  modelProgressPercentage,
  modelStateLabel,
  progressPercentage,
  getOracleErrorAction,
  type OracleStage,
} from "./oracleUtils";
import { RerankerStatus } from "./OracleSearch";

interface OracleSetupProps {
  stage: Exclude<OracleStage, "ready" | "incomplete">;
  workspaceRequest: TrackedRequestState<OracleWorkspace>;
  statusRequest: TrackedRequestState<OracleIndexStatus>;
  workspaceBusy: boolean;
  indexStarting: boolean;
  cancelBusy: boolean;
  modelDownloadBusy: boolean;
  workspaceActionError: string | null;
  indexActionError: string | null;
  onChooseWorkspace: () => void;
  onStartIndex: () => void;
  onCancel: () => void;
  onRefreshStatus: () => void;
  onRetryModels: () => void;
}

const SETUP_STEPS = ["Folder", "Models", "Index", "Ask"] as const;

export function OracleSetup({
  stage,
  workspaceRequest,
  statusRequest,
  workspaceBusy,
  indexStarting,
  cancelBusy,
  modelDownloadBusy,
  workspaceActionError,
  indexActionError,
  onChooseWorkspace,
  onStartIndex,
  onCancel,
  onRefreshStatus,
  onRetryModels,
}: OracleSetupProps) {
  const workspace = workspaceRequest.status === "ready" ? workspaceRequest.value : null;
  const status = statusRequest.status === "ready" ? statusRequest.value : null;

  return (
    <article className={`oracle-stage-card oracle-stage-card-${stage}`}>
      <SetupRail stage={stage} hasWorkspace={Boolean(workspace?.exists)} />
      {stage === "workspace-loading" && <WorkspaceLoading />}
      {stage === "choose-workspace" && (
        <ChooseWorkspace
          workspace={workspace}
          busy={workspaceBusy}
          requestError={workspaceRequest.status === "error" ? workspaceRequest.message : null}
          actionError={workspaceActionError}
          onChooseWorkspace={onChooseWorkspace}
        />
      )}
      {stage === "oracle-loading" && <OracleLoading />}
      {stage === "models" && (
        <ModelSetup
          status={status}
          actionError={workspaceActionError}
          onCancel={onCancel}
          cancelBusy={cancelBusy}
          onRetry={onRetryModels}
          retryDisabled={modelDownloadBusy}
        />
      )}
      {stage === "index" && (
        <IndexSetup
          workspace={workspace}
          status={status}
          actionError={indexActionError}
          starting={indexStarting}
          onStart={onStartIndex}
          onRetryReranker={onRetryModels}
          retryDisabled={modelDownloadBusy}
        />
      )}
      {stage === "indexing" && (
        <IndexingSetup status={status} onCancel={onCancel} cancelBusy={cancelBusy} />
      )}
      {stage === "oracle-error" && (
        <OracleErrorSetup
          workspace={workspace}
          status={status}
          statusRequest={statusRequest}
          actionError={indexActionError ?? workspaceActionError}
          onChooseWorkspace={onChooseWorkspace}
          onRetryStatus={onRefreshStatus}
          onRetryIndex={onStartIndex}
          workspaceBusy={workspaceBusy}
        />
      )}
    </article>
  );
}

function SetupRail({ stage, hasWorkspace }: { stage: OracleStage; hasWorkspace: boolean }) {
  const completed =
    stage === "models" ||
    stage === "index" ||
    stage === "indexing" ||
    stage === "ready" ||
    stage === "oracle-error";
  const modelComplete = stage === "index" || stage === "indexing" || stage === "ready";
  const indexComplete = stage === "ready";
  const currentStep =
    stage === "workspace-loading" || stage === "choose-workspace"
      ? 0
      : stage === "models" || stage === "oracle-loading"
        ? 1
        : stage === "index" || stage === "indexing" || stage === "oracle-error"
          ? 2
          : 3;
  const done = [hasWorkspace && completed, modelComplete, indexComplete, false];

  return (
    <div className="oracle-setup-rail" aria-label="Oracle setup progress">
      {SETUP_STEPS.map((label, index) => (
        <div
          className={`oracle-setup-step${index === currentStep ? " oracle-setup-step-current" : ""}${done[index] ? " oracle-setup-step-done" : ""}`}
          key={label}
        >
          <span className="oracle-setup-step-number">{done[index] ? "✓" : index + 1}</span>
          <span>{label}</span>
        </div>
      ))}
    </div>
  );
}

function WorkspaceLoading() {
  return (
    <StageContent
      eyebrow="Getting started"
      title="Checking for an Oracle folder"
      description="Oracle is looking for the folder saved on this machine."
    >
      <div className="oracle-state-message" role="status" aria-live="polite">
        Loading the saved folder…
      </div>
    </StageContent>
  );
}

function ChooseWorkspace({
  workspace,
  busy,
  requestError,
  actionError,
  onChooseWorkspace,
}: {
  workspace: OracleWorkspace | null;
  busy: boolean;
  requestError: string | null;
  actionError: string | null;
  onChooseWorkspace: () => void;
}) {
  const inaccessible = Boolean(workspace?.path && !workspace.exists);
  return (
    <StageContent
      eyebrow="Step 1 · Folder"
      title={inaccessible ? "This folder cannot be read" : "Choose a folder for Oracle"}
      description={
        inaccessible
          ? "Oracle cannot index the saved location. Choose a folder you can open and read."
          : "Pick the repository or directory you want Oracle to understand. Oracle searches one local folder at a time."
      }
    >
      {workspace?.path && (
        <div className="oracle-path-card">
          <span className="oracle-eyebrow">Saved folder</span>
          <code>{workspace.path}</code>
        </div>
      )}
      <button
        className="oracle-button oracle-button-primary oracle-stage-action"
        type="button"
        onClick={onChooseWorkspace}
        disabled={busy || workspace?.editable === false}
      >
        {busy ? "Choosing…" : inaccessible ? "Choose another folder" : "Choose folder"}
      </button>
      {(requestError || actionError) && (
        <div className="oracle-error-message" role="alert">
          {requestError ?? actionError}
        </div>
      )}
      <FailureGuide kind="folder" />
    </StageContent>
  );
}

function OracleLoading() {
  return (
    <StageContent
      eyebrow="Preparing"
      title="Checking Oracle’s local setup"
      description="Oracle is checking the local models and index before it asks you to do anything."
    >
      <div className="oracle-state-message" role="status" aria-live="polite">
        Loading Oracle status…
      </div>
    </StageContent>
  );
}

function ModelSetup({
  status,
  actionError,
  onCancel,
  cancelBusy,
  onRetry,
  retryDisabled,
}: {
  status: OracleIndexStatus | null;
  actionError: string | null;
  onCancel: () => void;
  cancelBusy: boolean;
  onRetry: () => void;
  retryDisabled: boolean;
}) {
  const model = status?.model ?? null;
  const reranker = status?.reranker ?? null;
  const hasFailure = [model, reranker].some(
    (item) => item?.state === "failed" || item?.state === "cancelled",
  );
  return (
    <StageContent
      eyebrow="Step 2 · Models"
      title={hasFailure ? "Oracle needs its local models" : "Preparing Oracle locally"}
      description="These small models stay on this machine. The embedding model turns code into searchable vectors; the reranker reorders the best candidates so useful pointers rise to the top."
    >
      <div className="oracle-model-list">
        <ModelDownloadCard
          label="Embedding model"
          size="about 34 MB"
          purpose="Builds the local index Oracle searches."
          status={model}
          onRetry={onRetry}
          retryDisabled={retryDisabled}
        />
        <ModelDownloadCard
          label="Reranker"
          size="about 5 MB"
          purpose="Reorders candidates after dense retrieval."
          status={reranker}
          optional
          onRetry={onRetry}
          retryDisabled={retryDisabled}
        />
      </div>
      {actionError && (
        <div className="oracle-error-message" role="alert">
          {actionError}
        </div>
      )}
      <div className="oracle-help-note">
        <strong>What is happening?</strong>
        <span>Oracle downloads both models automatically after you choose a folder.</span>
      </div>
      {(model?.state === "downloading" || reranker?.state === "downloading") && (
        <button
          className="oracle-button oracle-button-secondary oracle-cancel-action"
          type="button"
          onClick={onCancel}
          disabled={cancelBusy}
        >
          {cancelBusy ? "Cancelling…" : "Cancel download"}
        </button>
      )}
      <FailureGuide kind="models" />
    </StageContent>
  );
}

function ModelDownloadCard({
  label,
  size,
  purpose,
  status,
  optional,
  onRetry,
  retryDisabled,
}: {
  label: string;
  size: string;
  purpose: string;
  status: OracleModelStatus | null;
  optional?: boolean;
  onRetry: () => void;
  retryDisabled: boolean;
}) {
  const percentage = modelProgressPercentage(status);
  const ready = status?.state === "ready";
  const failed = status?.state === "failed" || status?.state === "cancelled";
  return (
    <div className={`oracle-model-card${failed ? " oracle-model-card-failed" : ""}`}>
      <div className="oracle-model-card-heading">
        <div>
          <strong>{label}</strong>
          <span>{size}</span>
        </div>
        <span
          className={`oracle-model-state oracle-model-state-${ready ? "ready" : failed ? "failed" : "pending"}`}
        >
          {modelStateLabel(status)}
        </span>
      </div>
      <p>{purpose}</p>
      {status?.state === "downloading" && (
        <>
          <div className="oracle-model-progress-copy">
            <span>{status.file ?? "Downloading model files…"}</span>
            <span>{percentage === null ? "—" : `${percentage}%`}</span>
          </div>
          {percentage !== null && (
            <div
              className="oracle-progress-track"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percentage}
              aria-label={`${label} download progress`}
            >
              <div className="oracle-progress-fill" style={{ width: `${percentage}%` }} />
            </div>
          )}
          <small>{status.message ?? `Saving to ${status.directory}`}</small>
        </>
      )}
      {ready && <small className="oracle-model-ready">Downloaded and ready.</small>}
      {failed && (
        <div className="oracle-model-failure" role="alert">
          <span>{status?.message ?? `The ${label.toLowerCase()} download did not finish.`}</span>
          <button
            className="oracle-button oracle-button-secondary oracle-model-retry-button"
            type="button"
            onClick={onRetry}
            disabled={retryDisabled}
          >
            Retry download
          </button>
        </div>
      )}
      {!status && <small>Waiting for Oracle to report the download status…</small>}
      {optional && failed && (
        <small className="oracle-model-degradation">
          Oracle can still search densely, but results may be less precise without this step.
        </small>
      )}
    </div>
  );
}

function IndexSetup({
  workspace,
  status,
  actionError,
  starting,
  onStart,
  onRetryReranker,
  retryDisabled,
}: {
  workspace: OracleWorkspace | null;
  status: OracleIndexStatus | null;
  actionError: string | null;
  starting: boolean;
  onStart: () => void;
  onRetryReranker: () => void;
  retryDisabled: boolean;
}) {
  return (
    <StageContent
      eyebrow="Step 3 · Index"
      title="Index this folder"
      description="The first pass reads and chunks every file locally. It can take several minutes, especially in a large repository, but Oracle will show its progress."
    >
      <div className="oracle-path-card">
        <span className="oracle-eyebrow">Folder to index</span>
        <code>{workspace?.path ?? "Selected folder"}</code>
      </div>
      <div className="oracle-ready-models">
        <span className="oracle-check-dot oracle-check-dot-ok" aria-hidden="true" />
        Embedding model ready · Oracle can build the local index.
      </div>
      {status?.reranker?.state !== "ready" && (
        <div className="oracle-watch-notice" role="status">
          <RerankerStatus
            status={status?.reranker ?? null}
            onRetry={onRetryReranker}
            retryDisabled={retryDisabled}
          />
        </div>
      )}
      <button
        className="oracle-button oracle-button-primary oracle-stage-action"
        type="button"
        onClick={onStart}
        disabled={starting}
      >
        {starting ? "Starting index…" : "Index folder"}
      </button>
      {actionError && (
        <div className="oracle-error-message" role="alert">
          {actionError}
        </div>
      )}
      <div className="oracle-help-note">
        <strong>Why does indexing take minutes?</strong>
        <span>
          Oracle reads the folder once, splits it into useful source spans, and stores only the
          local search index.
        </span>
      </div>
    </StageContent>
  );
}

function IndexingSetup({
  status,
  onCancel,
  cancelBusy,
}: {
  status: OracleIndexStatus | null;
  onCancel: () => void;
  cancelBusy: boolean;
}) {
  const percentage = progressPercentage(status);
  const progressBucket = percentage === null ? null : Math.floor(percentage / 10);
  const hasStatus = status !== null;
  const totalFiles = status?.total_files ?? 0;
  const progressAnnouncement = useMemo(() => {
    if (!hasStatus) return "Oracle is preparing to index this folder.";
    if (progressBucket === null) return "Oracle is counting the files in this folder.";
    const milestone = progressBucket * 10;
    const milestoneFiles = Math.floor((totalFiles * milestone) / 100);
    return `Oracle indexing is about ${milestone}% complete: roughly ${formatCount(milestoneFiles)} of ${formatCount(totalFiles)} files indexed.`;
  }, [hasStatus, progressBucket, totalFiles]);
  return (
    <StageContent
      eyebrow="Step 3 · Indexing"
      title="Oracle is learning this folder"
      description="The first indexing pass can take several minutes. You can leave this panel open while Oracle works locally."
    >
      <div className="oracle-index-progress">
        <div className="oracle-index-progress-heading">
          <strong>
            {status
              ? `${formatCount(status.indexed_files)} of ${formatCount(status.total_files)} files`
              : "Preparing files…"}
          </strong>
          <span>{percentage === null ? "—" : `${percentage}%`}</span>
        </div>
        <div className="oracle-visually-hidden" role="status" aria-live="polite" aria-atomic="true">
          {progressAnnouncement}
        </div>
        {percentage === null ? (
          <div className="oracle-state-message" role="status">
            Oracle has not reported the file count yet.
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
        <p>
          {status?.indexed_chunks
            ? `${formatCount(status.indexed_chunks)} chunks ready so far.`
            : "Reading and chunking source files…"}
        </p>
      </div>
      <div className="oracle-help-note">
        <strong>What is happening?</strong>
        <span>
          Oracle is turning the folder into a local index. Searching becomes available when this
          pass finishes.
        </span>
      </div>
      <button
        className="oracle-button oracle-button-secondary oracle-cancel-action"
        type="button"
        onClick={onCancel}
        disabled={cancelBusy}
      >
        {cancelBusy ? "Cancelling…" : "Cancel indexing"}
      </button>
    </StageContent>
  );
}

function OracleErrorSetup({
  workspace,
  status,
  statusRequest,
  actionError,
  onChooseWorkspace,
  onRetryStatus,
  onRetryIndex,
  workspaceBusy,
}: {
  workspace: OracleWorkspace | null;
  status: OracleIndexStatus | null;
  statusRequest: TrackedRequestState<OracleIndexStatus>;
  actionError: string | null;
  onChooseWorkspace: () => void;
  onRetryStatus: () => void;
  onRetryIndex: () => void;
  workspaceBusy: boolean;
}) {
  const requestError = statusRequest.status === "error" ? statusRequest.message : null;
  const primaryAction = getOracleErrorAction({ statusRequest, status });
  const chooseWorkspacePrimary = primaryAction === "choose-workspace";
  return (
    <StageContent
      eyebrow="Oracle needs attention"
      title="Oracle could not prepare the index"
      description="The failure is visible here so you can act on it. Check the folder, free disk space, or restore the network before trying again."
    >
      {(requestError || actionError || status?.state === "error") && (
        <div className="oracle-error-message" role="alert">
          {requestError ?? actionError ?? "The index operation reported an error."}
        </div>
      )}
      <div className="oracle-error-actions">
        <button
          className="oracle-button oracle-button-primary"
          type="button"
          onClick={
            chooseWorkspacePrimary
              ? onChooseWorkspace
              : primaryAction === "retry-index"
                ? onRetryIndex
                : onRetryStatus
          }
          disabled={chooseWorkspacePrimary && (workspaceBusy || workspace?.editable === false)}
        >
          {chooseWorkspacePrimary
            ? "Choose another folder"
            : primaryAction === "retry-index"
              ? "Retry index"
              : "Try again"}
        </button>
        <button
          className="oracle-button oracle-button-secondary"
          type="button"
          onClick={chooseWorkspacePrimary ? onRetryStatus : onChooseWorkspace}
          disabled={chooseWorkspacePrimary ? false : workspaceBusy || workspace?.editable === false}
        >
          {chooseWorkspacePrimary ? "Try again" : "Choose another folder"}
        </button>
      </div>
      <FailureGuide kind="all" />
    </StageContent>
  );
}

function FailureGuide({ kind }: { kind: "folder" | "models" | "all" }) {
  return (
    <div className="oracle-failure-guide">
      <strong>If something fails</strong>
      <ul>
        {(kind === "models" || kind === "all") && (
          <li>No network: reconnect, then retry the model download.</li>
        )}
        {(kind === "folder" || kind === "all") && (
          <li>Unreadable folder: choose a directory you can open and read.</li>
        )}
        {(kind === "models" || kind === "all") && (
          <li>Disk full: free at least 40 MB, then retry.</li>
        )}
      </ul>
    </div>
  );
}

function StageContent({
  eyebrow,
  title,
  description,
  children,
}: {
  eyebrow: string;
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <div className="oracle-stage-content">
      <div className="oracle-eyebrow">{eyebrow}</div>
      <h3>{title}</h3>
      <p className="oracle-stage-description">{description}</p>
      <div className="oracle-stage-body">{children}</div>
    </div>
  );
}
