import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, KeyboardEvent, ReactNode } from "react";
import type {
  DesignAssistantMessage,
  DesignAttachment,
  DesignAttachmentFeedback,
  DesignDocument,
  DesignAgentSession,
  DesignHost,
  DesignLayer,
  DesignMessage,
  DesignOutputMode,
  DesignTranscriptItem,
  PendingPermission,
} from "./designHost";
import { ErrorText } from "../../components/ErrorText";
import { isImeComposition } from "../../lib/imeComposition";
import { artifactSlideNotice, readArtifactSlideShape } from "./artifactSlides";
import { findUndefinedCustomProperties } from "./artifactTokenLint";
import type { ArtifactRenderCriticResult } from "./artifactRenderCritic";
import {
  getCachedArtifactStructure,
  sectionsToLayers,
  setCachedArtifactStructure,
  type ArtifactSection,
  type ArtifactStructure,
} from "./artifactStructure";
import {
  formatSectionNotesScope,
  MAX_SECTION_NOTE_CHARS,
  MAX_SECTION_NOTES,
  resolveSectionNotes,
  type ResolvedSectionNote,
} from "./sectionNotes";
import {
  ARTIFACT_PAGE_HEIGHT,
  artifactPageHeightForCanvas,
  shouldAdaptArtifactHeight,
} from "./artifactViewport";
import {
  ARTIFACT_TOO_LARGE_MESSAGE,
  AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS,
  fencedBlockNotice,
  transcriptItems,
  type SessionError,
} from "./agentHost";
import {
  MAX_AUTOMATIC_SKILL_SECTIONS,
  builtInSkillIndex,
  builtInSkillSources,
} from "./builtInSkills";
import {
  DEFAULT_DESIGN_SKILL_SELECTION,
  loadDesignOutputMode,
  loadDesignProviderId,
  loadDesignSkillSelection,
  loadDesignWorkspaceId,
  loadStoredDesignProviderId,
  loadStoredDesignWorkspaceId,
  saveDesignOutputMode,
  saveDesignProviderId,
  saveDesignSkillSelection,
  saveDesignWorkspaceId,
  selectedSlugs,
  SKILL_MODE_LABELS,
  type DesignSkillSelection,
} from "./designSettings";
import { DesignFolderControl } from "./DesignFolderControl";
import {
  attachmentPillKey,
  attachmentReadFailureMessage,
  importDesignAttachments,
} from "./designAttachments";
import { DesignHistoryList } from "./DesignHistoryList";
import {
  historyEntryInstruction,
  recordDesignHistoryEntry,
  type DesignHistoryEntry,
} from "./designHistory";
import {
  openDesignHistoryEntry,
  type DesignHistoryOpenHandle,
  type DesignHistoryOpenResult,
} from "./designHistoryOpen";
import {
  clearDelegatedMirrorPin,
  DELEGATED_DESIGN_MESSAGE_PREFIX,
  noteHumanOpenedHistory,
} from "./delegatedDesignMirror";
import { buildSkillBlock } from "./skillLoader";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { chatCapableProviders } from "../workspace/workspaceSessions";
import type { PermissionAnswer } from "../../components/PermissionCard";
import { useModalOpen } from "../../lib/modalOpen";
import {
  projectAdd,
  projectsList,
  providersList,
  workspaceCreate,
  workspacesList,
} from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { useAppStore } from "../../store/appStore";
import { nodesBounds } from "../../lib/canvas/viewportMath";
import type { AgentSessionState } from "../../lib/agentSession";
import type { Project, ProviderInfo, Session, Workspace } from "../../types/ipc";
import type { NodeRect } from "../../types/geometry";
import {
  clampViewportZoom,
  DESIGN_MAX_ZOOM,
  DESIGN_MIN_ZOOM,
  fitViewport,
  type DesignViewport,
} from "./designViewport";
import { DesignCanvas, ZoomControls } from "./DesignCanvas";
import { ArtifactExportControls } from "./ArtifactExportControls";
import { DesignAssistant } from "./DesignAssistant";
import { LayerPanel, type LayerChainStep, type LayerViewModel } from "./DesignLayerPanel";
import { DesignCraftSheet } from "./DesignSkillControls";
import {
  ARTIFACT_CONTEXT_NAME,
  ARTIFACT_NODE_ID,
  DESIGN_FIT_MARGIN,
  artifactNodeRect,
  layerRectsFor,
} from "./designCanvasGeometry";
import {
  buildLayerTree,
  isHidden,
  layerAncestorChain,
  layerMoveTarget,
  LAYER_MOVE_BY_ARROW,
} from "./designLayerTree";
import {
  cloneMessages,
  EMPTY_ARTIFACT_STRUCTURE,
  EMPTY_DESIGN_MESSAGES,
  EMPTY_RESOLVED_NOTES,
  EMPTY_SECTION_NOTES,
  EMPTY_SECTIONS,
  EMPTY_TRANSCRIPT,
  HISTORY_OPEN_MESSAGE_PREFIX,
  isHistoryOpenMessage,
  promptForMessage,
  terminalMessagesForSave,
} from "./designMessageModel";
import type {
  AttachmentMessage,
  DesignHistory,
  DesignSnapshot,
  DesignViewState,
  MessageAction,
  SnapshotChange,
  WorkspaceProject,
} from "./designSurfaceTypes";
import "./artifactPreview.css";
import "./design.css";
import "./designSession.css";

export type { DesignDocument, DesignHost } from "./designHost";
export { artifactNodeRect, smallestSectionAt } from "./designCanvasGeometry";
export { buildLayerTree, layerAncestorIds, layerMoveTarget } from "./designLayerTree";
export { DesignCraftSheet, DesignSkillModeControl } from "./DesignSkillControls";
export { revealScrollTopFor } from "./DesignLayerPanel";

interface DesignToolbarProps {
  /**
   * The folder control that owns the toolbar's left slot. It is passed in as an
   * element rather than as a dozen props because the toolbar only positions it:
   * the attachment itself is the surface's state, not the toolbar's.
   */
  folderControl: ReactNode;
  grounded: boolean;
  outputMode: DesignOutputMode;
  busy: boolean;
  onOutputModeChange: (mode: DesignOutputMode) => void;
  canSave: boolean;
  saved: boolean;
  saving: boolean;
  saveError: string | null;
  canUndo: boolean;
  canRedo: boolean;
  historyRefreshKey: number;
  liveSessionId: string | null;
  onGroundingToggle: () => void;
  onSave: () => void;
  onUndo: () => void;
  onRedo: () => void;
  /**
   * Returns true when the pick was accepted and an attach actually started, and
   * false when it was refused (a generation running, another attach already in
   * flight, or the entry is the design already on the canvas). The popover
   * closes only on true: a refused pick changed nothing, so it must not look
   * like it did.
   */
  onHistoryOpen: (entry: DesignHistoryEntry) => boolean;
}

const WORKSPACE_NOT_REGISTERED_NOTICE = "The attached folder is no longer registered.";
const WORKSPACE_UNCONFIRMED_NOTICE =
  "The attached folder could not be confirmed because its record failed to load.";

// The persistence calls report a boolean: false means the value never reached disk and will
// revert on reload. One notice region serves all five callers because they fail the same way,
// but each message names what was lost, because the five mean different things to the user.
const PERSISTENCE_NOTICE_TEXT = {
  provider: "Your agent choice was not saved.",
  workspace: "Your folder choice was not saved.",
  skill: "Your craft selection was not saved.",
  output: "Your output choice was not saved.",
  history: "This design was not added to your history.",
} as const;

type PersistenceNoticeKind = keyof typeof PERSISTENCE_NOTICE_TEXT;

/**
 * Whether an error the host threw is `agentHost`'s own shape — an `Error`
 * whose `detail` is the rejection's raw text or null. The `in` check alone
 * would prove membership, not the type, so the value is checked before it
 * reaches the transcript's detail node.
 */
function isSessionError(error: unknown): error is SessionError {
  return (
    error instanceof Error &&
    "detail" in error &&
    (typeof error.detail === "string" || error.detail === null)
  );
}

function cloneLayers(document: DesignDocument): DesignLayer[] {
  return cloneLayerList(document.layers);
}

function cloneLayerList(layers: readonly DesignLayer[]): DesignLayer[] {
  return layers.map((layer) => ({
    ...layer,
    transform: { ...layer.transform },
  }));
}

// Exported for the modal-contract walking test in src/app/modals-over-crescent.test.tsx.
export const DesignToolbar = memo(function DesignToolbar({
  folderControl,
  grounded,
  outputMode,
  busy,
  onOutputModeChange,
  canSave,
  saved,
  saving,
  saveError,
  canUndo,
  canRedo,
  historyRefreshKey,
  liveSessionId,
  onGroundingToggle,
  onSave,
  onUndo,
  onRedo,
  onHistoryOpen,
}: DesignToolbarProps) {
  const saveText = saving ? "Saving…" : saved ? "Saved" : "Unsaved changes";
  const [historyOpen, setHistoryOpen] = useState(false);
  const historyMenuRef = useRef<HTMLDivElement>(null);
  const historyTriggerRef = useRef<HTMLButtonElement>(null);
  const historyPopoverRef = useRef<HTMLDivElement>(null);

  useModalOpen(historyOpen);

  const closeHistory = useCallback(() => {
    setHistoryOpen(false);
    queueMicrotask(() => historyTriggerRef.current?.focus());
  }, []);

  // The close belongs here, on the pick itself, not in openHistoryEntry's result
  // callback: that callback can land as loading, timeout or failed, and a menu
  // that waits for success would sit over the canvas forever on a failure. The
  // loading, timeout and failed notices render outside this popover, so closing
  // it takes none of that feedback away.
  const handleHistoryEntryOpen = useCallback(
    (entry: DesignHistoryEntry) => {
      if (onHistoryOpen(entry)) closeHistory();
    },
    [closeHistory, onHistoryOpen],
  );

  useEffect(() => {
    if (!historyOpen) return;
    historyPopoverRef.current?.focus();

    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (isImeComposition(event)) return;
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeHistory();
    };
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && !historyMenuRef.current?.contains(target)) {
        closeHistory();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [closeHistory, historyOpen]);

  return (
    <header className="design-toolbar">
      {folderControl}
      {canSave ? (
        <>
          <span className="design-save-status" aria-live="polite">
            <span
              className={`design-save-dot${saved && !saving ? " design-save-dot-saved" : ""}`}
              aria-hidden="true"
            />
            {saveText}
          </span>
          {saveError ? (
            <span className="design-save-error" role="alert">
              {saveError}
            </span>
          ) : null}
        </>
      ) : null}
      <span className="design-toolbar-spacer" />
      <span className="design-history-controls" aria-label="History controls">
        <button
          className="design-history-button"
          type="button"
          title="Undo (Ctrl+Z)"
          aria-label="Undo"
          onClick={onUndo}
          disabled={!canUndo}
        >
          ↶
        </button>
        <button
          className="design-history-button"
          type="button"
          title="Redo (Ctrl+Shift+Z)"
          aria-label="Redo"
          onClick={onRedo}
          disabled={!canRedo}
        >
          ↷
        </button>
      </span>
      <div className="design-history-menu" ref={historyMenuRef}>
        <button
          ref={historyTriggerRef}
          className="design-history-menu-button"
          type="button"
          aria-controls="design-history-popover"
          aria-expanded={historyOpen}
          aria-haspopup="dialog"
          onClick={() => {
            if (historyOpen) {
              closeHistory();
            } else {
              setHistoryOpen(true);
            }
          }}
        >
          History
        </button>
        <div
          ref={historyPopoverRef}
          id="design-history-popover"
          className="design-history-popover"
          role="dialog"
          aria-label="Design history"
          tabIndex={-1}
          hidden={!historyOpen}
        >
          <DesignHistoryList
            refreshKey={historyRefreshKey}
            liveSessionId={liveSessionId}
            onOpen={handleHistoryEntryOpen}
          />
        </div>
      </div>
      <button
        className="design-grounding-toggle"
        type="button"
        title={
          grounded
            ? "Oracle grounding on: the next run searches the repository first"
            : "Oracle grounding off: the next run does not search or read the repository"
        }
        aria-pressed={grounded}
        onClick={onGroundingToggle}
      >
        <span
          className={`design-grounding-dot${grounded ? " design-grounding-dot-on" : ""}`}
          aria-hidden="true"
        />
        {grounded ? "Grounded" : "Not grounded"}
      </button>
      {/*
        The output shape wears the grounding control's own classes — same pill,
        same dot, same states, zero new CSS — because it answers the same kind
        of question (what the next run does) in the same bar. The toggle is
        disabled while a generation runs, so the visible mode is always the
        mode of the running generation: a mid-run flip would leave it unclear
        whether Page or Slides is on its way, and "applies to the next run"
        is exactly the ambiguity this control refuses. The choice snapshots
        into generationOptions in startGeneration, like grounded does.
      */}
      <button
        className="design-grounding-toggle"
        type="button"
        title={
          busy
            ? "A generation is running; the output shape cannot change mid-run."
            : outputMode === "slides"
              ? "Slide deck: the next run produces one id-anchored section per slide."
              : "Page: the next run produces a scrolling page."
        }
        aria-label={`Output shape: ${outputMode === "slides" ? "Slides" : "Page"}`}
        aria-pressed={outputMode === "slides"}
        disabled={busy}
        onClick={() => onOutputModeChange(outputMode === "slides" ? "page" : "slides")}
      >
        <span
          className={`design-grounding-dot${outputMode === "slides" ? " design-grounding-dot-on" : ""}`}
          aria-hidden="true"
        />
        {outputMode === "slides" ? "Slides" : "Page"}
      </button>
      {canSave ? (
        <span className="design-save-actions">
          <button className="design-save-primary" type="button" onClick={onSave} disabled={saving}>
            Save to repo
          </button>
        </span>
      ) : null}
    </header>
  );
});

export interface DesignSurfaceProps {
  host: DesignHost;
}

export function DesignSurface({ host }: DesignSurfaceProps) {
  const storedDocument = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.document : null,
  );
  const [document, setDocument] = useState<DesignDocument | null>(storedDocument);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const controller = new AbortController();

    const session = useAppStore.getState().designSession;
    if (session.host !== host) {
      useAppStore.getState().setDesignHost(host);
    } else if (session.document !== null) {
      return () => {
        active = false;
        controller.abort();
      };
    }

    void host
      .loadDocument(controller.signal)
      .then((loadedDocument) => {
        if (!active) return;
        const messages = cloneMessages(loadedDocument);
        useAppStore.getState().setDesignDocument(host, loadedDocument, messages);
        setDocument(loadedDocument);
      })
      .catch((error: unknown) => {
        if (!active) return;
        setLoadError(
          error instanceof Error ? error.message : "The design document could not load.",
        );
      });

    return () => {
      active = false;
      controller.abort();
    };
  }, [host]);

  if (loadError !== null) {
    return (
      <section className="surface-card design-surface" data-screen-label="Design">
        <div role="alert">Unable to load the design document: {loadError}</div>
      </section>
    );
  }

  if (document === null) {
    return (
      <section className="surface-card design-surface" data-screen-label="Design">
        <div role="status">Loading…</div>
      </section>
    );
  }

  return <DesignSurfaceContent host={host} document={document} />;
}

interface DesignSurfaceContentProps {
  host: DesignHost;
  document: DesignDocument;
}

function DesignSurfaceContent({ host, document }: DesignSurfaceContentProps) {
  const daemon = useWorkspaceDaemon();
  const [lastKnownDaemonCapabilities, setLastKnownDaemonCapabilities] = useState<
    readonly string[] | null
  >(null);
  useEffect(() => {
    if (daemon.state === "connected" && daemon.capabilities.includes("typed_permissions")) {
      setLastKnownDaemonCapabilities((previous) => previous ?? daemon.capabilities);
    }
  }, [daemon]);
  // A failed poll reports an empty capability list, but that means "unknown", not "absent".
  // A permission event itself proves this session negotiated typed permissions, so keep the
  // card visible even before the first successful status poll; thereafter use the last connected
  // capability snapshot while the daemon is reconnecting.
  const permissionCapabilities =
    daemon.state === "connected"
      ? (lastKnownDaemonCapabilities ?? daemon.capabilities)
      : (lastKnownDaemonCapabilities ?? ["typed_permissions"]);
  const daemonGone = daemon.state !== "connected";
  const messages = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.messages : EMPTY_DESIGN_MESSAGES,
  );
  // Anchored agent notes: document field mirrored in the store, like messages.
  const sectionNotes = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.sectionNotes : EMPTY_SECTION_NOTES,
  );
  const generation = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.generation : null,
  );
  const skillIndex = useMemo(() => builtInSkillIndex(), []);
  const knownSkillSlugs = useMemo(() => skillIndex.map((entry) => entry.slug), [skillIndex]);
  const initialSnapshot: DesignSnapshot = {
    hiddenLayerIds: document.initialState.hiddenLayerIds,
    layers: cloneLayers(document),
  };
  const initialViewState: DesignViewState = {
    pan: { x: 0, y: 0 },
    selectedLayerId: document.selectedLayerId,
    zoom: clampViewportZoom(document.initialState.zoom),
  };
  const [history, setHistory] = useState<DesignHistory>(() => ({
    present: initialSnapshot,
    past: [],
    future: [],
    saved: document.initialState.saved,
  }));
  const [viewState, setViewState] = useState<DesignViewState>(initialViewState);
  // Adaptive frame height for the generated page: width stays 1280, height
  // follows the live canvas aspect (see artifactViewport). Seeded at the
  // 800 baseline so mount and tests without a measured canvas keep the
  // canonical sheet until a real canvas size arrives with an artifact.
  const [artifactPageHeight, setArtifactPageHeight] = useState(ARTIFACT_PAGE_HEIGHT);
  const [composerContextLayerId, setComposerContextLayerId] = useState<string | null>(
    initialViewState.selectedLayerId,
  );
  const [grounded, setGrounded] = useState(document.grounded);
  // The output shape is surface state like the skill selection, not document
  // state like grounding: it says what the next run must produce, and it is
  // remembered in the surface settings beside the craft selection.
  const [outputMode, setOutputModeState] = useState<DesignOutputMode>("page");
  const outputModeInteractedRef = useRef(false);
  const [draft, setDraft] = useState(document.initialState.draft);
  /**
   * Files imported as starting points for the run the user is about to start.
   * Deliberately not part of the document and never persisted: an attachment
   * belongs to the request it was attached to, and `startGeneration` clears both
   * this and the draft at once so the composer never shows a file that the run it
   * is describing did not carry.
   */
  const [attachments, setAttachments] = useState<readonly DesignAttachment[]>([]);
  const [attachmentMessages, setAttachmentMessages] = useState<readonly AttachmentMessage[]>([]);
  /**
   * The same list the state holds, and the one imports are measured against. The
   * importer's ceilings are computed from what is already attached, so an import
   * has to see the list as it stands when it runs, not as it stood when its handler
   * was created; and two imports must not measure against the same baseline and
   * together pass a ceiling neither would pass alone. Hence the queue: one import
   * at a time, each reading the ref the previous one finished writing.
   */
  const attachmentsRef = useRef<readonly DesignAttachment[]>([]);
  const attachQueueRef = useRef<Promise<void>>(Promise.resolve());
  /**
   * Bumped by every run that consumes the composer. An import reads a file
   * asynchronously against the list as it stood when the read began; if a run
   * empties that list while the read is in flight, the import's result describes
   * a composer that no longer exists. Without this, the resolved import writes
   * its files back after `startGeneration` cleared them, so a consumed file
   * reappears in the composer under a run that did not carry it. The epoch is
   * captured when the import starts and checked before it writes: an import
   * started before the consumption cannot write after it.
   */
  const attachmentEpochRef = useRef(0);
  /**
   * The import in flight, if any.
   *
   * It exists so that work which is no longer wanted can be stopped rather than
   * finished: a run that consumes the composer aborts it (the epoch above would
   * discard the result anyway, and a render is not a thing to spend on a
   * composer that has moved on), and unmounting aborts it too. The renderer
   * checks the signal between pages and inside a page's own render loop, so an
   * abort costs the page in flight, not the call.
   */
  const attachControllerRef = useRef<AbortController | null>(null);
  /**
   * What the import is doing right now, as a page count (`deck.pdf: page 2 of
   * 3.`). Live state rather than an import message: it describes work in
   * progress and has to leave when the work does.
   */
  const [attachmentProgress, setAttachmentProgress] = useState<string | null>(null);

  /**
   * The one place the two are written together. `setAttachments` alone would leave
   * the ref behind, and the ref is what the next import measures against.
   */
  const commitAttachments = useCallback((next: readonly DesignAttachment[]) => {
    attachmentsRef.current = next;
    setAttachments(next);
  }, []);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [persistenceNotice, setPersistenceNotice] = useState<string | null>(null);
  const [skillSelection, setSkillSelectionState] = useState<DesignSkillSelection>(
    DEFAULT_DESIGN_SKILL_SELECTION,
  );
  const [appliedSkillSlugs, setAppliedSkillSlugs] = useState<readonly string[] | null>(null);
  const [skillResultNotice, setSkillResultNotice] = useState<string | null>(null);
  const [craftSheetMode, setCraftSheetMode] = useState<"manual" | "readonly" | null>(null);
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [providersLoading, setProvidersLoading] = useState(true);
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(null);
  const [unavailableProviderId, setUnavailableProviderId] = useState<string | null>(null);
  const [workspaceProjects, setWorkspaceProjects] = useState<WorkspaceProject[]>([]);
  const [workspacesLoading, setWorkspacesLoading] = useState(true);
  const [workspacesRefreshing, setWorkspacesRefreshing] = useState(false);
  const [workspacesError, setWorkspacesError] = useState<ErrorSentence | null>(null);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null);
  const [workspaceSelectionNotice, setWorkspaceSelectionNotice] = useState<string | null>(null);
  const [workspaceSelectionUnresolved, setWorkspaceSelectionUnresolved] = useState(false);
  const [agentSession, setAgentSession] = useState<DesignAgentSession | null>(
    () => host.getAgentSession?.() ?? null,
  );
  const [agentState, setAgentState] = useState<AgentSessionState | null>(
    () => host.getAgentSession?.()?.getState() ?? null,
  );
  const [agentSessionRecord, setAgentSessionRecord] = useState<Session | null>(
    () => host.getAgentSessionRecord?.() ?? null,
  );
  const [pendingPermission, setPendingPermission] = useState<PendingPermission | null>(
    () => host.getPendingPermission?.() ?? null,
  );
  const [permissionNotice, setPermissionNotice] = useState<string | null>(
    () => host.getPermissionNotice?.() ?? null,
  );
  const [historyOpenResult, setHistoryOpenResult] = useState<DesignHistoryOpenResult | null>(null);
  const [historyRefreshKey, setHistoryRefreshKey] = useState(0);

  const savingRef = useRef(false);
  const mountedRef = useRef(true);
  const messagesRef = useRef(messages);
  const documentRevisionRef = useRef(0);
  const skillSelectionInteractedRef = useRef(false);
  const skillSelectionRef = useRef(skillSelection);
  const providerSelectionInteractedRef = useRef(false);
  const workspaceSelectionInteractedRef = useRef(false);
  const workspaceSelectionIdRef = useRef<string | null>(null);
  const workspaceSelectionUnresolvedRef = useRef(false);
  const workspaceRequestTokenRef = useRef(0);
  // Which kind the currently shown persistence notice belongs to, so a later successful save
  // clears only its own kind's warning and leaves the others untouched.
  const persistenceNoticeKindRef = useRef<PersistenceNoticeKind | null>(null);
  const historyOpenRef = useRef<DesignHistoryOpenHandle | null>(null);
  const historyOpenInFlightRef = useRef(false);
  const historyOpenGenerationRef = useRef(0);
  const historyOpenMessageCounterRef = useRef(0);
  const generationInFlightRef = useRef(generation !== null);
  const liveSessionIdRef = useRef<string | null>(agentSessionRecord?.id ?? null);
  const assistantRef = useRef<HTMLDivElement>(null);
  const designSurfaceRef = useRef<HTMLElement>(null);
  const setMessages = useCallback(
    (
      update:
        | readonly DesignMessage[]
        | ((messages: readonly DesignMessage[]) => readonly DesignMessage[]),
    ) => {
      useAppStore.getState().setDesignMessages(host, update);
    },
    [host],
  );

  // `saved === false` is the only definitive failure; `true` clears the same kind's notice, and
  // anything else (a rejected call is "we do not know") changes nothing. Only the notice is
  // mount-guarded here — the persistence calls themselves must still run after unmount.
  const reportPersistence = useCallback((kind: PersistenceNoticeKind, saved: boolean): void => {
    if (!mountedRef.current) return;
    if (saved === false) {
      persistenceNoticeKindRef.current = kind;
      setPersistenceNotice(PERSISTENCE_NOTICE_TEXT[kind]);
      return;
    }
    if (saved === true && persistenceNoticeKindRef.current === kind) {
      // A later successful save of the same kind clears the warning; other kinds stay shown.
      persistenceNoticeKindRef.current = null;
      setPersistenceNotice(null);
    }
  }, []);

  const updateWorkspaceSelection = useCallback(
    (workspaceId: string | null, unresolved: boolean, notice: string | null): void => {
      workspaceSelectionIdRef.current = workspaceId;
      workspaceSelectionUnresolvedRef.current = unresolved;
      setSelectedWorkspaceId(workspaceId);
      setWorkspaceSelectionUnresolved(unresolved);
      setWorkspaceSelectionNotice(notice);
    },
    [],
  );

  useEffect(() => {
    const updateAgentSession = (): void => {
      const next = host.getAgentSession?.() ?? null;
      const nextRecord = host.getAgentSessionRecord?.() ?? null;
      // An absent record means this host has no live session to protect from a history attach.
      liveSessionIdRef.current = nextRecord?.id ?? null;
      setAgentSession(next);
      setAgentState(next?.getState() ?? null);
      setAgentSessionRecord(nextRecord);
      setPendingPermission(host.getPendingPermission?.() ?? null);
      setPermissionNotice(host.getPermissionNotice?.() ?? null);
    };
    const unsubscribe = host.subscribeAgentSession?.(updateAgentSession);
    updateAgentSession();
    return () => unsubscribe?.();
  }, [host]);

  const [streamingTranscript, setStreamingTranscript] =
    useState<readonly DesignTranscriptItem[]>(EMPTY_TRANSCRIPT);
  // Read by startGeneration's failure path and the stop/retry actions below.
  // Those callbacks must see the rows live at the moment they run, not the
  // rows live at the moment they were created, so they read this ref instead
  // of taking streamingTranscript as a dependency (which would recreate them
  // on every stream chunk).
  const streamingTranscriptRef = useRef(streamingTranscript);

  useEffect(() => {
    if (agentSession === null) return;
    // This subscription is the surface's single live view of the session, and it also keeps the
    // transcript rows. The boundary comes from the host because it is recorded after the
    // craft-selection pre-flight, whose items are the host's own question, not the agent's
    // answer. Stop and failure run outside render, so they read the same rows the working card
    // renders instead of taking a dependency on every chunk.
    const update = (): void => {
      const state = agentSession.getState();
      setAgentState(state);
      const start = host.getRunTranscriptStart?.() ?? null;
      const next = start === null ? EMPTY_TRANSCRIPT : transcriptItems(state.items, start);
      streamingTranscriptRef.current = next;
      setStreamingTranscript(next);
    };
    update();
    return agentSession.subscribe(update);
  }, [agentSession, host]);

  useEffect(() => {
    let active = true;
    void providersList()
      .then((catalog) => {
        if (!active) return;
        const available = chatCapableProviders(catalog.providers);
        setProviders(available);
        return loadDesignProviderId(available.map((provider) => provider.id)).then((storedId) => {
          if (!active) return;
          if (providerSelectionInteractedRef.current) return;
          setSelectedProviderId(storedId);
          setUnavailableProviderId(null);
          if (storedId !== null) {
            const storedProvider = available.find((provider) => provider.id === storedId);
            if (storedProvider !== undefined) {
              // Mount restores the preference only; the first generation owns session creation.
              (host.setProviderPreference ?? host.selectProvider)?.(storedProvider);
            }
            return;
          }
          return loadStoredDesignProviderId().then((rawStoredId) => {
            if (!active || providerSelectionInteractedRef.current) return;
            if (
              rawStoredId !== null &&
              !available.some((provider) => provider.id === rawStoredId)
            ) {
              setUnavailableProviderId(rawStoredId);
            }
          });
        });
      })
      .catch(() => {
        if (active) {
          setProviders([]);
          setSelectedProviderId(null);
          setUnavailableProviderId(null);
        }
      })
      .finally(() => {
        if (active) setProvidersLoading(false);
      });
    return () => {
      active = false;
    };
  }, [host]);

  const refreshWorkspaceProjects = useCallback(
    async (initialLoad: boolean, isActive: () => boolean = () => true): Promise<void> => {
      const requestToken = ++workspaceRequestTokenRef.current;
      if (initialLoad) setWorkspacesLoading(true);
      else setWorkspacesRefreshing(true);
      setWorkspacesError(null);

      // active only rejects updates after unmount; opening twice can leave an older response alive
      // while a newer request is current, so the token also orders concurrent refreshes.
      const isCurrent = (): boolean =>
        isActive() && mountedRef.current && requestToken === workspaceRequestTokenRef.current;

      try {
        const projects = await projectsList();
        const records = await Promise.all(
          projects.map(async (project): Promise<WorkspaceProject> => {
            try {
              return { ...project, workspaces: await workspacesList(project.id) };
            } catch {
              return {
                ...project,
                workspaces: [],
                workspaceError: "Workspaces could not be loaded.",
              };
            }
          }),
        );
        if (!isCurrent()) return;

        setWorkspaceProjects(records);
        const workspaceIds = records.flatMap((project) =>
          project.workspaces.map((workspace) => workspace.id),
        );
        const failedProjectExists = records.some((project) => project.workspaceError !== undefined);
        const existingSession = host.getAgentSessionRecord?.() ?? null;
        const wasUnresolved = workspaceSelectionUnresolvedRef.current;
        let storedSelection = false;
        let candidateId: string | null;

        if (existingSession !== null) {
          candidateId = existingSession.workspaceId;
        } else if (!initialLoad || workspaceSelectionInteractedRef.current) {
          candidateId = workspaceSelectionIdRef.current;
        } else {
          storedSelection = true;
          candidateId = failedProjectExists
            ? await loadStoredDesignWorkspaceId()
            : await loadDesignWorkspaceId(workspaceIds);
          if (!isCurrent()) return;
          if (workspaceSelectionInteractedRef.current) {
            candidateId = workspaceSelectionIdRef.current;
            storedSelection = false;
          }
        }

        if (!isCurrent()) return;
        const selectedWorkspace =
          candidateId === null
            ? undefined
            : records
                .flatMap((project) => project.workspaces)
                .find((workspace) => workspace.id === candidateId);

        if (candidateId !== null && selectedWorkspace !== undefined) {
          updateWorkspaceSelection(candidateId, false, null);
          if (existingSession === null && (storedSelection || (!initialLoad && wasUnresolved))) {
            (host.setWorkspacePreference ?? host.selectWorkspace)?.(selectedWorkspace);
          }
        } else if (candidateId !== null && failedProjectExists) {
          updateWorkspaceSelection(candidateId, true, WORKSPACE_UNCONFIRMED_NOTICE);
          if (!initialLoad && !wasUnresolved) {
            (host.setWorkspacePreference ?? host.selectWorkspace)?.(null);
          }
        } else if (!initialLoad && candidateId !== null) {
          updateWorkspaceSelection(null, false, WORKSPACE_NOT_REGISTERED_NOTICE);
          (host.setWorkspacePreference ?? host.selectWorkspace)?.(null);
          void saveDesignWorkspaceId(null).then((saved) => reportPersistence("workspace", saved));
        } else if (candidateId !== null || initialLoad) {
          updateWorkspaceSelection(null, false, null);
        }
      } catch (cause: unknown) {
        if (!isCurrent()) return;
        const mapped = errorSentence(cause);
        setWorkspacesError({
          sentence: `Could not load workspaces: ${mapped.sentence}`,
          detail: mapped.detail,
        });
      }
      if (!isCurrent()) return;
      if (initialLoad) setWorkspacesLoading(false);
      else {
        setWorkspacesRefreshing(false);
        setWorkspacesLoading(false);
      }
    },
    [host, reportPersistence, updateWorkspaceSelection],
  );

  useEffect(() => {
    let active = true;
    void refreshWorkspaceProjects(true, () => active);
    return () => {
      active = false;
    };
  }, [refreshWorkspaceProjects]);

  const snapshot = history.present;
  const layers = snapshot.layers;
  const saved = history.saved;
  const busy = generation !== null;
  const { pan, selectedLayerId, zoom } = viewState;

  const disposeHistoryOpen = useCallback(() => {
    historyOpenGenerationRef.current += 1;
    historyOpenInFlightRef.current = false;
    const current = historyOpenRef.current;
    historyOpenRef.current = null;
    current?.dispose();
  }, []);

  useEffect(() => disposeHistoryOpen, [disposeHistoryOpen]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      // An import outliving the surface is a render drawing pages for nobody:
      // the abort stops it at the page boundary (see `attachControllerRef`).
      attachControllerRef.current?.abort();
    };
  }, []);

  useEffect(() => {
    messagesRef.current = messages;
    if (assistantRef.current && messages.length > 0) {
      assistantRef.current.scrollTop = assistantRef.current.scrollHeight;
    }
    // The transcript also grows while a run streams, and following those rows is the same
    // behaviour the Workspace chat has: text that arrives below the fold is not "shown".
  }, [messages, busy, streamingTranscript]);

  useEffect(() => {
    let active = true;
    void loadDesignSkillSelection(knownSkillSlugs).then((selection) => {
      if (active && !skillSelectionInteractedRef.current) setSkillSelectionState(selection);
    });
    return () => {
      active = false;
    };
  }, [knownSkillSlugs]);

  useEffect(() => {
    let active = true;
    void loadDesignOutputMode().then((mode) => {
      if (active && !outputModeInteractedRef.current) setOutputModeState(mode);
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    skillSelectionRef.current = skillSelection;
  }, [skillSelection]);

  const artifact = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.latestArtifact : null,
  );
  const artifactHtml = artifact?.html;
  const artifactError = artifact?.error;
  const artifactOutputMode = artifact?.outputMode;
  const artifactFencedBlockCount = artifact?.fencedHtmlBlockCount;
  const artifactMissingTokens = useMemo(
    () =>
      artifactHtml !== undefined && artifactError === undefined
        ? findUndefinedCustomProperties(artifactHtml)
        : [],
    [artifactError, artifactHtml],
  );
  // No recorded mode means no slides contract: the notice stays silent rather
  // than assuming one. The gate sits before the parse: a page-mode artifact is
  // allowed to contain <section> landmarks, and reporting on it would state a
  // contract that never applied (see `artifactSlides.ts`). Keyed on the markup
  // and the recorded mode, so the parse reruns when either changes — not once
  // per canvas event.
  const artifactSlideShapeNotice = useMemo(
    () =>
      artifactOutputMode === "slides" && artifactHtml !== undefined
        ? artifactSlideNotice(readArtifactSlideShape(artifactHtml))
        : "",
    [artifactHtml, artifactOutputMode],
  );
  // How many fenced blocks the reply carried is a fact the producing run
  // recorded on the artifact, like the mode above, not something re-derived
  // from the markup: the reply itself is already gone from here, and re-parsing
  // it would silently mean "one" for an artifact that records no count.
  // `fencedBlockNotice` returns "" for one block, which is the ordinary case.
  const artifactFencedBlockNotice = fencedBlockNotice(artifactFencedBlockCount);
  // The export title is the title of the run that produced the artifact on
  // screen, matched by markup identity so a stale message cannot lend its
  // name. Absent when the run is unknown; the exporter then falls back.
  const artifactSourceTitle = useMemo(() => {
    if (artifactHtml === undefined) return undefined;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (
        message?.role === "assistant" &&
        message.status === "done" &&
        message.artifactHtml === artifactHtml
      ) {
        return message.title;
      }
    }
    return undefined;
  }, [artifactHtml, messages]);
  // The export paginates by the mode recorded on the artifact, not the switch (next run);
  // a reopened artifact with no recorded mode stays undefined and print falls back to shape.
  const artifactExportControls = useMemo(
    () =>
      artifactHtml !== undefined ? (
        <ArtifactExportControls
          html={artifactHtml}
          title={artifactSourceTitle}
          outputMode={artifactOutputMode}
        />
      ) : null,
    [artifactHtml, artifactOutputMode, artifactSourceTitle],
  );
  const artifactRect = useMemo(
    () =>
      artifactHtml !== undefined || artifactError !== undefined
        ? artifactNodeRect(layers, artifactPageHeight)
        : null,
    [artifactError, artifactHtml, artifactPageHeight, layers],
  );

  // Measured page structure for the current artifact. The critic feeds the
  // module cache once per new artifact (same pass, no second measurement);
  // this state only re-renders the surface when that result lands. A remount
  // reads straight from the cache, so navigating back keeps the layers and the
  // measured page height.
  const [measuredArtifact, setMeasuredArtifact] = useState<{
    html: string;
    sections: readonly ArtifactSection[];
    contentHeight?: number;
  } | null>(null);
  const handleArtifactMeasured = useCallback((html: string, result: ArtifactRenderCriticResult) => {
    const sections = result.structure ?? EMPTY_SECTIONS;
    const contentHeight = result.contentHeight;
    setCachedArtifactStructure(html, {
      sections,
      ...(contentHeight === undefined ? {} : { contentHeight }),
    });
    setMeasuredArtifact({
      html,
      sections,
      ...(contentHeight === undefined ? {} : { contentHeight }),
    });
  }, []);
  const artifactStructure: ArtifactStructure = useMemo(() => {
    if (artifactHtml === undefined) return EMPTY_ARTIFACT_STRUCTURE;
    if (measuredArtifact !== null && measuredArtifact.html === artifactHtml) {
      return measuredArtifact;
    }
    return getCachedArtifactStructure(artifactHtml) ?? EMPTY_ARTIFACT_STRUCTURE;
  }, [artifactHtml, measuredArtifact]);
  const artifactSections = artifactStructure.sections;
  const artifactContentHeight = artifactStructure.contentHeight;
  const sectionAnchors = useMemo(
    () => new Set(artifactSections.map((section) => section.anchor)),
    [artifactSections],
  );
  const sectionLayers = useMemo(
    () =>
      artifactRect === null
        ? []
        : sectionsToLayers(artifactSections, { x: artifactRect.x, y: artifactRect.y }),
    [artifactSections, artifactRect],
  );
  // Displayed layers: canvas nodes first, measured page sections after.
  // Undo snapshots, saves, and fit math keep using `layers` (canvas nodes
  // only); sections are derived from the artifact and never enter history.
  const displayLayers = useMemo(() => [...layers, ...sectionLayers], [layers, sectionLayers]);

  const selectedLayer = useMemo(
    () => displayLayers.find((layer) => layer.id === selectedLayerId) ?? null,
    [displayLayers, selectedLayerId],
  );

  // The same resolved list drives both the composer summary and each generation.
  const selectedSkillSlugs = useMemo(
    () => selectedSlugs(skillSelection, knownSkillSlugs),
    [knownSkillSlugs, skillSelection],
  );
  const updateSkillSelection = useCallback(
    (selection: DesignSkillSelection) => {
      skillSelectionInteractedRef.current = true;
      skillSelectionRef.current = selection;
      setSkillSelectionState(selection);
      void saveDesignSkillSelection(selection).then((saved) => reportPersistence("skill", saved));
    },
    [reportPersistence],
  );
  const handleSkillModeChange = useCallback(
    (mode: DesignSkillSelection["mode"]) => {
      if (skillSelection.mode === mode) return;
      setSkillResultNotice(null);
      setAppliedSkillSlugs(null);
      updateSkillSelection({
        ...skillSelection,
        mode,
        enabledSlugs:
          mode === "manual"
            ? skillSelection.enabledSlugs.slice(0, MAX_AUTOMATIC_SKILL_SECTIONS)
            : skillSelection.enabledSlugs,
      });
    },
    [skillSelection, updateSkillSelection],
  );
  const handleSkillToggle = useCallback(
    (slug: string) => {
      if (skillSelection.mode !== "manual") return;
      const enabled = new Set(skillSelection.enabledSlugs);
      if (enabled.has(slug)) enabled.delete(slug);
      else {
        if (enabled.size >= MAX_AUTOMATIC_SKILL_SECTIONS) return;
        enabled.add(slug);
      }
      updateSkillSelection({
        ...skillSelection,
        enabledSlugs: [...enabled],
      });
      setSkillResultNotice(null);
      setAppliedSkillSlugs(null);
    },
    [skillSelection, updateSkillSelection],
  );
  const openManualCraftSheet = useCallback(() => setCraftSheetMode("manual"), []);
  const openCraftReadOnlySheet = useCallback(() => setCraftSheetMode("readonly"), []);
  const closeCraftSheet = useCallback(() => setCraftSheetMode(null), []);

  const resolvedSkillSlugs =
    appliedSkillSlugs !== null
      ? appliedSkillSlugs
      : skillSelection.mode === "auto"
        ? null
        : selectedSkillSlugs;
  // Manual belongs here too: `resolvedSkillSlugs` is the user's own ticks, so the composition
  // is as resolved as it is in `all`. Leaving it out meant a manual selection that overflowed
  // the budget kept every box ticked and said nothing, which is the same lie this row status
  // exists to prevent — and the larger the corpus grows, the easier it is to tick past the
  // ceiling.
  const hasResolvedComposition =
    skillSelection.mode === "all" ||
    skillSelection.mode === "manual" ||
    (skillSelection.mode === "auto" && appliedSkillSlugs !== null);
  const skillBlock = useMemo(
    () => buildSkillBlock(builtInSkillSources(), resolvedSkillSlugs ?? []),
    [resolvedSkillSlugs],
  );
  const resolvedSkillSlugSet = useMemo(
    () => new Set(resolvedSkillSlugs ?? []),
    [resolvedSkillSlugs],
  );
  const automaticBaselineSlugSet = useMemo(
    () => new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS),
    [],
  );
  const droppedSkillSlugSet = useMemo(() => new Set(skillBlock.dropped), [skillBlock]);

  const layerRows = useMemo(
    () =>
      displayLayers.map((layer) => ({
        ...layer,
        selected: layer.id === selectedLayerId,
        hidden: isHidden(snapshot.hiddenLayerIds, layer.id),
        hasNote:
          layer.kind === "SECTION" && layer.section !== undefined
            ? sectionNotes.some((note) => note.anchor === layer.section?.anchor)
            : false,
      })),
    [displayLayers, selectedLayerId, snapshot.hiddenLayerIds, sectionNotes],
  );

  // The tree is rebuilt from layer ids, so it survives filters and reorders of
  // `displayLayers`; the navigator is its root level and the keyboard walks it.
  const layerTree = useMemo(() => buildLayerTree(displayLayers), [displayLayers]);
  const layerRowById = useMemo(
    () => new Map<string, LayerViewModel>(layerRows.map((row) => [row.id, row])),
    [layerRows],
  );
  const navigatorRows = useMemo(
    () =>
      layerTree.roots
        .map((layer) => layerRowById.get(layer.id))
        .filter((row): row is LayerViewModel => row !== undefined),
    [layerTree, layerRowById],
  );
  const selectedRow = selectedLayer === null ? null : (layerRowById.get(selectedLayer.id) ?? null);
  const selectedAncestors = useMemo<readonly LayerChainStep[]>(() => {
    if (selectedRow === null || selectedRow.section === undefined) return [];
    return layerAncestorChain(layerTree, selectedRow.id).map((layer) => ({
      id: layer.id,
      name: layer.name,
    }));
  }, [layerTree, selectedRow]);

  const fitRects = useMemo<NodeRect[]>(() => {
    const rects = layerRectsFor(layers).filter(
      (layer) => !snapshot.hiddenLayerIds.includes(layer.id),
    );
    return artifactRect === null ? rects : [...rects, artifactRect];
  }, [artifactRect, layers, snapshot.hiddenLayerIds]);
  const fitRectsRef = useRef<NodeRect[]>([]);
  useEffect(() => {
    fitRectsRef.current = fitRects;
  }, [fitRects]);

  const saveDocument = host.saveDocument;
  const generate = host.generate;
  const canSave = saveDocument !== undefined;
  const canGenerate = generate !== undefined;
  const selectProvider = useCallback(
    (provider: ProviderInfo) => {
      if (busy) return;
      providerSelectionInteractedRef.current = true;
      setSelectedProviderId(provider.id);
      setUnavailableProviderId(null);
      (host.setProviderPreference ?? host.selectProvider)?.(provider);
      void saveDesignProviderId(provider.id).then((saved) => reportPersistence("provider", saved));
    },
    [busy, host, reportPersistence],
  );
  const selectWorkspace = useCallback(
    (workspace: Workspace | null) => {
      if (busy) return;
      workspaceSelectionInteractedRef.current = true;
      updateWorkspaceSelection(workspace?.id ?? null, false, null);
      (host.setWorkspacePreference ?? host.selectWorkspace)?.(workspace);
      const workspaceId = workspace?.id ?? null;
      void saveDesignWorkspaceId(workspaceId).then((saved) =>
        reportPersistence("workspace", saved),
      );
    },
    [busy, host, reportPersistence, updateWorkspaceSelection],
  );
  const openWorkspacePicker = useCallback(() => {
    if (busy) return;
    void refreshWorkspaceProjects(false);
  }, [busy, refreshWorkspaceProjects]);
  const [folderAttachBusy, setFolderAttachBusy] = useState(false);
  const [folderAttachError, setFolderAttachError] = useState<ErrorSentence | null>(null);

  /**
   * The local checkout a folder is worked in. A folder registered here but never
   * used has none yet, and a session needs one, so attaching a folder also creates
   * its first local checkout — the same step the Workspace surface takes when it
   * starts an agent in a project. An existing local checkout is reused rather than
   * duplicated.
   */
  const ensureFolderCheckout = useCallback(async (folder: Project): Promise<Workspace> => {
    const existing = await workspacesList(folder.id);
    const checkout = existing.find((workspace) => workspace.isolation === "local");
    return checkout ?? workspaceCreate(folder.id, "local");
  }, []);

  const attachFolder = useCallback(async (): Promise<boolean> => {
    if (busy || folderAttachBusy) return false;
    setFolderAttachBusy(true);
    setFolderAttachError(null);
    try {
      const selected = await openFolderDialog({ directory: true, title: "Attach a folder" });
      if (typeof selected !== "string") return false;
      // project_add canonicalizes the path and re-registers an already known folder
      // instead of duplicating it, so the returned id is the one to use.
      const folder = await projectAdd(selected);
      const checkout = await ensureFolderCheckout(folder);
      await refreshWorkspaceProjects(false);
      selectWorkspace(checkout);
      return true;
    } catch (cause: unknown) {
      setFolderAttachError(errorSentence(cause));
      return false;
    } finally {
      setFolderAttachBusy(false);
    }
  }, [busy, ensureFolderCheckout, folderAttachBusy, refreshWorkspaceProjects, selectWorkspace]);

  const useRegisteredFolder = useCallback(
    async (folderId: string): Promise<boolean> => {
      if (busy || folderAttachBusy) return false;
      const folder = workspaceProjects.find((candidate) => candidate.id === folderId);
      if (folder === undefined) return false;
      setFolderAttachBusy(true);
      setFolderAttachError(null);
      try {
        const checkout = await ensureFolderCheckout(folder);
        await refreshWorkspaceProjects(false);
        selectWorkspace(checkout);
        return true;
      } catch (cause: unknown) {
        setFolderAttachError(errorSentence(cause));
        return false;
      } finally {
        setFolderAttachBusy(false);
      }
    },
    [
      busy,
      ensureFolderCheckout,
      folderAttachBusy,
      refreshWorkspaceProjects,
      selectWorkspace,
      workspaceProjects,
    ],
  );
  const selectModel = useCallback(
    (modelId: string) => {
      void agentSession?.setModel(modelId);
    },
    [agentSession],
  );
  const selectEffort = useCallback(
    (effort: string) => {
      void agentSession?.setModel(undefined, effort);
    },
    [agentSession],
  );
  const respondPermission = useCallback(
    // The card's answer object travels whole to the host: this layer names
    // no fields, so a new answer carrier cannot be dropped here.
    (response: PermissionAnswer): Promise<void> =>
      host.respondPermission?.(response) ?? Promise.resolve(),
    [host],
  );
  const endSession = useCallback(() => {
    if (busy || agentSession === null) return;
    // Ending the session closes the reading too: the pin goes with it, so
    // the next delegation may mirror again.
    clearDelegatedMirrorPin();
    void host.closeAgentSession?.();
  }, [agentSession, busy, host]);

  const generationCount = useMemo(
    () =>
      messages.filter(
        (message): message is DesignAssistantMessage =>
          message.role === "assistant" &&
          message.status === "done" &&
          !isHistoryOpenMessage(message) &&
          // Delegated cards are readings, not generations the human started.
          !message.id.startsWith(DELEGATED_DESIGN_MESSAGE_PREFIX),
      ).length,
    [messages],
  );

  const generationLabel = `${generationCount} ${generationCount === 1 ? "generation" : "generations"}`;
  const composerContextTarget = useMemo(() => {
    const artifactPresent = artifactHtml !== undefined || artifactError !== undefined;
    if (composerContextLayerId === ARTIFACT_NODE_ID && artifactPresent) {
      const orphanBlock = formatSectionNotesScope(null, sectionNotes, sectionAnchors, true);
      return {
        label: ARTIFACT_CONTEXT_NAME,
        scope:
          `${document.contextPrefix} ${ARTIFACT_CONTEXT_NAME}; the user is refining the artifact the agent just produced.` +
          (orphanBlock.length > 0 ? `\n${orphanBlock}` : ""),
      };
    }

    const layer = displayLayers.find((candidate) => candidate.id === composerContextLayerId);
    if (!layer) return null;
    if (layer.kind === "SECTION" && layer.section !== undefined) {
      const anchor = layer.section.anchor;
      const base =
        `${document.contextPrefix} ${layer.name} (page section <${layer.section.tag}>); ` +
        `anchor: "${anchor}"; the user is pointing at the section named "${layer.name}".`;
      const notesBlock = formatSectionNotesScope(anchor, sectionNotes, sectionAnchors, true);
      return {
        label: layer.name,
        scope: notesBlock.length > 0 ? `${base}\n${notesBlock}` : base,
      };
    }
    const sourcePath = layer.source ? `; source file: ${layer.source.path}` : "";
    return {
      label: layer.name,
      scope: `${document.contextPrefix} ${layer.name} (${layer.kind})${sourcePath}; the user is pointing at the layer named "${layer.name}".`,
    };
  }, [
    artifactError,
    artifactHtml,
    composerContextLayerId,
    displayLayers,
    document.contextPrefix,
    sectionAnchors,
    sectionNotes,
  ]);
  const composerContextLayerName = composerContextTarget?.label ?? null;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const commitSnapshot = useCallback((change: SnapshotChange) => {
    documentRevisionRef.current += 1;
    setHistory((current) => {
      const next = change(current.present);
      if (next === null) return current;
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: [],
        saved: false,
      };
    });
  }, []);

  const markDocumentDirty = useCallback(() => {
    documentRevisionRef.current += 1;
    setHistory((current) => (current.saved ? { ...current, saved: false } : current));
  }, []);

  const selectLayer = useCallback(
    (layerId: string) => {
      const selectionChanged = selectedLayerId !== layerId || composerContextLayerId !== layerId;
      if (selectionChanged) markDocumentDirty();
      setViewState((current) =>
        current.selectedLayerId === layerId ? current : { ...current, selectedLayerId: layerId },
      );
      setComposerContextLayerId((current) => (current === layerId ? current : layerId));
    },
    [composerContextLayerId, markDocumentDirty, selectedLayerId],
  );
  // The one deselect path: the expanded row's close control, the row's Escape
  // handler below, and an empty-canvas click all land here, and the canvas
  // takes focus back so keyboard users stay oriented.
  const deselectLayer = useCallback(() => {
    selectLayer("");
    queueMicrotask(() => {
      designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas")?.focus();
    });
  }, [selectLayer]);

  useEffect(() => {
    if (selectedLayer === null) return;
    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (isImeComposition(event)) return;
      if (event.key !== "Escape") return;
      const surface = designSurfaceRef.current;
      if (!surface) return;
      const eventTarget = event.target;
      const focusTarget =
        eventTarget instanceof Element && eventTarget.isConnected
          ? eventTarget
          : globalThis.document.activeElement instanceof Element &&
              globalThis.document.activeElement.isConnected
            ? globalThis.document.activeElement
            : null;
      const escapeOwner = focusTarget?.closest<HTMLElement>(
        '[role="dialog"], [role="alertdialog"], [role="listbox"], [role="group"][aria-label]',
      );
      if (escapeOwner) return;
      event.preventDefault();
      deselectLayer();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [deselectLayer, selectedLayer]);

  // Tree movement while a layer is selected. The listener lives on the surface
  // element, not on the window: the shell's ArrowLeft/ArrowRight paging only
  // runs while the crescent nav holds focus, and a key from inside the surface
  // never reaches it. The note field keeps its caret keys; every other arrow
  // scroll default is left alone when the move has no target.
  useEffect(() => {
    const surface = designSurfaceRef.current;
    if (surface === null || selectedLayer === null) return;
    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }
      const move = LAYER_MOVE_BY_ARROW.get(event.key);
      if (move === undefined) return;
      const next = layerMoveTarget(layerTree, selectedLayer.id, move);
      if (next === null) return;
      event.preventDefault();
      event.stopPropagation();
      selectLayer(next);
    };
    surface.addEventListener("keydown", handleKeyDown);
    return () => surface.removeEventListener("keydown", handleKeyDown);
  }, [layerTree, selectLayer, selectedLayer]);

  const addSectionNote = useCallback(
    (anchor: string, text: string) => {
      const trimmed = text.trim().slice(0, MAX_SECTION_NOTE_CHARS);
      if (trimmed.length === 0) return;
      markDocumentDirty();
      useAppStore
        .getState()
        .setSectionNotes(host, (current) =>
          current.length >= MAX_SECTION_NOTES ? current : [...current, { anchor, text: trimmed }],
        );
    },
    [host, markDocumentDirty],
  );
  const deleteSectionNote = useCallback(
    (index: number) => {
      markDocumentDirty();
      useAppStore
        .getState()
        .setSectionNotes(host, (current) => current.filter((_, noteIndex) => noteIndex !== index));
    },
    [host, markDocumentDirty],
  );

  const selectedSectionAnchor =
    selectedLayer !== null && selectedLayer.kind === "SECTION"
      ? (selectedLayer.section?.anchor ?? null)
      : null;
  const selectedSectionNotes = useMemo(() => {
    if (selectedSectionAnchor === null) return EMPTY_RESOLVED_NOTES;
    const entries: ResolvedSectionNote[] = [];
    sectionNotes.forEach((note, index) => {
      if (note.anchor === selectedSectionAnchor) entries.push({ note, index });
    });
    return entries;
  }, [sectionNotes, selectedSectionAnchor]);
  // Stable like the other LayerPanel callbacks below: an inline arrow here
  // would hand memo(LayerPanel) a new prop on every parent render and
  // re-render every row for nothing.
  const handleAddSectionNote = useCallback(
    (text: string) => {
      if (selectedSectionAnchor !== null) addSectionNote(selectedSectionAnchor, text);
    },
    [addSectionNote, selectedSectionAnchor],
  );
  const orphanNotes = useMemo(
    () => resolveSectionNotes(sectionNotes, sectionAnchors, artifactHtml !== undefined).orphans,
    [sectionNotes, sectionAnchors, artifactHtml],
  );
  const sectionHighlight = useMemo<NodeRect | null>(() => {
    if (selectedLayer === null || selectedLayer.kind !== "SECTION") return null;
    return {
      id: selectedLayer.id,
      x: selectedLayer.transform.x,
      y: selectedLayer.transform.y,
      w: selectedLayer.transform.width,
      h: selectedLayer.transform.height,
      z: 0,
    };
  }, [selectedLayer]);
  const noteMarks = useMemo<NodeRect[]>(() => {
    const marks: NodeRect[] = [];
    const seen = new Set<string>();
    for (const section of sectionLayers) {
      const anchor = section.section?.anchor;
      if (anchor === undefined || seen.has(anchor)) continue;
      if (snapshot.hiddenLayerIds.includes(section.id)) continue;
      if (!sectionNotes.some((note) => note.anchor === anchor)) continue;
      seen.add(anchor);
      marks.push({
        id: section.id,
        x: section.transform.x,
        y: section.transform.y,
        w: 0,
        h: 0,
        z: 0,
      });
    }
    return marks;
  }, [sectionLayers, sectionNotes, snapshot.hiddenLayerIds]);

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      commitSnapshot((current) => ({
        ...current,
        hiddenLayerIds: current.hiddenLayerIds.includes(layerId)
          ? current.hiddenLayerIds.filter((id) => id !== layerId)
          : [...current.hiddenLayerIds, layerId],
      }));
    },
    [commitSnapshot],
  );

  const setViewport = useCallback((nextViewport: DesignViewport) => {
    setViewState((current) => {
      if (
        current.zoom === nextViewport.zoom &&
        current.pan.x === nextViewport.pan.x &&
        current.pan.y === nextViewport.pan.y
      ) {
        return current;
      }
      return { ...current, ...nextViewport };
    });
  }, []);
  // True once the user pans, zooms, or wheels after the last fit, so a later
  // reframe never tears the viewport out from under their hands. fitCanvas
  // clears it; every manual viewport path sets it.
  const viewportTouchedRef = useRef(false);
  const setZoom = useCallback((nextZoom: number | ((currentZoom: number) => number)) => {
    viewportTouchedRef.current = true;
    setViewState((current) => {
      const requested = typeof nextZoom === "function" ? nextZoom(current.zoom) : nextZoom;
      const next = clampViewportZoom(requested);
      return current.zoom === next ? current : { ...current, zoom: next };
    });
  }, []);
  const handleCanvasViewportChange = useCallback(
    (nextViewport: DesignViewport) => {
      viewportTouchedRef.current = true;
      setViewport(nextViewport);
    },
    [setViewport],
  );
  const zoomIn = useCallback(
    () => setZoom((currentZoom) => Number((currentZoom + 0.1).toFixed(1))),
    [setZoom],
  );
  const zoomOut = useCallback(
    () => setZoom((currentZoom) => Number((currentZoom - 0.1).toFixed(1))),
    [setZoom],
  );
  const zoomReset = useCallback(() => setZoom(1), [setZoom]);
  const fitCanvas = useCallback(() => {
    const canvas = designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
    if (!canvas) return;
    const bounds = canvas.getBoundingClientRect();
    const { pan: fittedPan, zoom: fittedZoom } = fitViewport(
      nodesBounds(fitRectsRef.current),
      bounds.width,
      bounds.height,
      DESIGN_FIT_MARGIN,
    );
    viewportTouchedRef.current = false;
    setViewport({ pan: fittedPan, zoom: fittedZoom });
  }, [setViewport]);

  // The artifact frame follows the live canvas aspect (width stays 1280, height
  // adapts), but its height re-renders the iframe, so it must not chase every
  // pixel. The ratio gate in shouldAdaptArtifactHeight and the new-artifact
  // trigger below are the only two reframe paths.
  const lastCanvasSizeRef = useRef<{ width: number; height: number } | null>(null);
  useEffect(() => {
    const canvas = designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
    if (!canvas || typeof ResizeObserver === "undefined") return;
    const seed = canvas.getBoundingClientRect();
    lastCanvasSizeRef.current = { width: seed.width, height: seed.height };
    const observer = new ResizeObserver(() => {
      const rect = canvas.getBoundingClientRect();
      const prev = lastCanvasSizeRef.current;
      lastCanvasSizeRef.current = { width: rect.width, height: rect.height };
      if (prev === null) return;
      if (!shouldAdaptArtifactHeight(prev.width, prev.height, rect.width, rect.height)) return;
      const desired = artifactPageHeightForCanvas(rect.width, rect.height);
      setArtifactPageHeight((current) => (current === desired ? current : desired));
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  // A new artifact is a full page, not a thumbnail: fit it into view the moment
  // it lands, so the whole generated page is visible without a manual Fit. The
  // ref is seeded with the artifact already on screen at mount, so reopening a
  // document keeps the saved viewport instead of snapping the camera. A reframe
  // (new artifact or adapted height) refits only while the viewport is still
  // pristine after the last fit; a manual pan/zoom owns the camera from then on.
  const fittedArtifactRef = useRef<string | undefined>(artifactHtml ?? artifactError);
  const fittedHeightRef = useRef(artifactPageHeight);
  useEffect(() => {
    const artifact = artifactHtml ?? artifactError;
    if (artifact === undefined) return;
    const isNewArtifact = fittedArtifactRef.current !== artifact;
    if (isNewArtifact) {
      fittedArtifactRef.current = artifact;
      const canvas = designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
      if (canvas) {
        const rect = canvas.getBoundingClientRect();
        lastCanvasSizeRef.current = { width: rect.width, height: rect.height };
        const desired = artifactPageHeightForCanvas(rect.width, rect.height);
        if (desired !== artifactPageHeight) {
          // Defer the fit until the reframed height commits, so the camera
          // fits the sheet the user will actually see instead of the old one.
          setArtifactPageHeight(desired);
          return;
        }
      }
    }
    const heightChanged = fittedHeightRef.current !== artifactPageHeight;
    if (!isNewArtifact && !heightChanged) return;
    fittedHeightRef.current = artifactPageHeight;
    if (!viewportTouchedRef.current) fitCanvas();
  }, [artifactError, artifactHtml, artifactPageHeight, fitCanvas]);

  const undo = useCallback(() => {
    if (!canUndo) return;
    documentRevisionRef.current += 1;
    setHistory((current) => {
      if (current.past.length === 0) return current;
      const previous = current.past[current.past.length - 1];
      return {
        ...current,
        present: previous,
        past: current.past.slice(0, -1),
        future: [current.present, ...current.future],
        saved: false,
      };
    });
  }, [canUndo]);

  const redo = useCallback(() => {
    if (!canRedo) return;
    documentRevisionRef.current += 1;
    setHistory((current) => {
      if (current.future.length === 0) return current;
      const next = current.future[0];
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: current.future.slice(1),
        saved: false,
      };
    });
  }, [canRedo]);

  useEffect(() => {
    const surface = designSurfaceRef.current;
    if (!surface) return;

    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.key.toLowerCase() !== "z") return;

      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }

      if (event.shiftKey) {
        if (!canRedo) return;
        event.preventDefault();
        redo();
        return;
      }

      if (!canUndo) return;
      event.preventDefault();
      undo();
    };

    surface.addEventListener("keydown", handleKeyDown);
    return () => surface.removeEventListener("keydown", handleKeyDown);
  }, [canRedo, canUndo, redo, undo]);

  const save = useCallback(async () => {
    if (saveDocument === undefined || savingRef.current) return;

    savingRef.current = true;
    const revisionAtSave = documentRevisionRef.current;
    const hasWorkingMessage = messages.some(
      (message) => message.role === "assistant" && message.status === "working",
    );
    const documentToSave: DesignDocument = {
      ...document,
      initialState: {
        ...document.initialState,
        hiddenLayerIds: [...history.present.hiddenLayerIds],
      },
      selectedLayerId,
      grounded,
      layers: cloneLayerList(layers),
      sectionNotes: sectionNotes.map((note) => ({ ...note })),
      messages: terminalMessagesForSave(messages),
    };

    setSaveError(null);
    setSaving(true);
    try {
      await saveDocument(documentToSave);
      if (documentRevisionRef.current === revisionAtSave && !hasWorkingMessage) {
        if (mountedRef.current) {
          setHistory((current) => (current.saved ? current : { ...current, saved: true }));
        }
      }
    } catch (error: unknown) {
      if (mountedRef.current) {
        setHistory((current) => (current.saved ? { ...current, saved: false } : current));
        setSaveError(
          error instanceof Error ? error.message : "The design document could not save.",
        );
      }
    } finally {
      savingRef.current = false;
      if (mountedRef.current) setSaving(false);
    }
  }, [
    document,
    grounded,
    history.present,
    layers,
    messages,
    saveDocument,
    sectionNotes,
    selectedLayerId,
  ]);
  const toggleGrounding = useCallback(() => {
    markDocumentDirty();
    setGrounded((value) => !value);
  }, [markDocumentDirty]);

  // The toggle is disabled while busy (see the toolbar), so this guard only
  // covers callers that bypass the button; the visible mode never disagrees
  // with the running generation.
  const updateOutputMode = useCallback(
    (mode: DesignOutputMode) => {
      if (busy) return;
      outputModeInteractedRef.current = true;
      setOutputModeState(mode);
      void saveDesignOutputMode(mode).then((saved) => reportPersistence("output", saved));
    },
    [busy, reportPersistence],
  );

  /**
   * Attaches a history entry to the canvas. Returns true when the attach was
   * started and false when the guard refused the pick, so the caller knows
   * whether an action really happened. Every guard condition is synchronous, so
   * the answer is settled before the first await of the attach.
   */
  const openHistoryEntry = useCallback(
    (entry: DesignHistoryEntry) => {
      if (
        busy ||
        generationInFlightRef.current ||
        historyOpenInFlightRef.current ||
        entry.sessionId === liveSessionIdRef.current
      ) {
        return false;
      }

      // Provenance belongs to the generation that produced it. History entries do not persist
      // that metadata, so clear the live result before showing a different artifact.
      setAppliedSkillSlugs(null);
      setSkillResultNotice(null);
      disposeHistoryOpen();
      historyOpenInFlightRef.current = true;
      const openGeneration = historyOpenGenerationRef.current;
      setHistoryOpenResult({ status: "loading" });
      const handle = openDesignHistoryEntry(entry.sessionId, {
        onResult: (result) => {
          if (openGeneration !== historyOpenGenerationRef.current) return;
          if (result.status !== "loading") historyOpenInFlightRef.current = false;
          const isOversizedArtifact =
            result.status === "failed" && result.message === ARTIFACT_TOO_LARGE_MESSAGE;
          // The oversized error is rendered in the canvas message below, so a banner would duplicate it.
          setHistoryOpenResult(isOversizedArtifact ? null : result);
          if (result.status === "artifact" || isOversizedArtifact) {
            // A deliberate open that landed pins the panel: later delegated
            // arrivals replay nothing and write nothing until the human
            // moves on. A failed open pins nothing — there is no card being
            // read, so there is nothing to protect from being yanked away.
            noteHumanOpenedHistory(entry.sessionId);
            const messageId = `${HISTORY_OPEN_MESSAGE_PREFIX}${++historyOpenMessageCounterRef.current}`;
            // A pointer is not a prompt: a `child` entry's title is the commissioned
            // agent's display name, so it is left off the card as an instruction and
            // the card offers no action that would generate from it.
            const instruction = historyEntryInstruction(entry);
            markDocumentDirty();
            setMessages((current) => [
              ...current,
              {
                id: messageId,
                role: "assistant",
                status: "done",
                title: entry.title || "Untitled design",
                desc:
                  result.status === "artifact"
                    ? "Reopened from design history."
                    : "The reopened artifact could not be displayed.",
                sources: [],
                nodeIds: [],
                ...(instruction === null ? {} : { instruction }),
                // No outputMode or fencedHtmlBlockCount: the history entry records
                // neither the shape the run asked for nor how many blocks its reply
                // carried, so a reopened artifact has no contract to read back and
                // no dropped selection to report.
                ...(result.status === "artifact"
                  ? { artifactHtml: result.html }
                  : { artifactError: result.message }),
              },
            ]);
          }
        },
      });
      historyOpenRef.current = handle;
      return true;
    },
    [busy, disposeHistoryOpen, markDocumentDirty, setMessages],
  );

  /**
   * The directory the agent is actually given, when a session has been opened.
   * The daemon echoes it back; it is the truth about where the agent is working,
   * so it wins over the attachment the user picked. Before the first generation
   * there is no session and the attachment is all that is known. Defined before
   * startGeneration so the generation options can name the folder Oracle must
   * search: grounding is about this folder, never the global index.
   */
  const attachedFolder = workspaceProjects
    .flatMap((project) => project.workspaces)
    .find((workspace) => workspace.id === selectedWorkspaceId);
  const attachedFolderPath = agentSessionRecord?.cwd ?? attachedFolder?.path ?? null;

  /**
   * The composer's feedback about a run's attachments, from the host that
   * stores them.
   *
   * The pages of an attached document are deposited after this surface has
   * handed them over — the run clears the composer the moment it starts — so
   * this callback is the only route a progress count, or the sentence naming the
   * pages that did not make it, has back to the row they belong to. Progress
   * replaces the one transient line the import also uses; a note or an error is
   * kept, exactly as an import's own feedback is.
   */
  const handleAttachmentFeedback = useCallback((message: DesignAttachmentFeedback): void => {
    const { kind, text, detail } = message;
    if (kind === "progress") {
      setAttachmentProgress(text);
      return;
    }
    setAttachmentProgress(null);
    setAttachmentMessages((current) => [
      ...current,
      { kind, text, ...(detail === undefined ? {} : { detail }) },
    ]);
  }, []);

  const startGeneration = useCallback(
    (prompt: string) => {
      if (
        busy ||
        daemonGone ||
        generate === undefined ||
        generationInFlightRef.current ||
        historyOpenInFlightRef.current
      ) {
        return;
      }
      // A new run is the human's own work: release the history pin so later
      // delegations may mirror again.
      clearDelegatedMirrorPin();
      generationInFlightRef.current = true;
      disposeHistoryOpen();
      setHistoryOpenResult(null);
      const scopedPrompt = composerContextTarget
        ? `${prompt}\n\nScope: ${composerContextTarget.scope}`
        : prompt;
      const controller = new AbortController();
      const userId = crypto.randomUUID();
      const assistantId = crypto.randomUUID();
      const userMessage: DesignMessage = {
        id: userId,
        role: "user",
        text: prompt,
        ctx: composerContextLayerName
          ? `${document.contextPrefix} ${composerContextLayerName}`
          : undefined,
      };
      const assistantMessage: DesignAssistantMessage = {
        id: assistantId,
        role: "assistant",
        status: "working",
        title: document.workingMessage.title,
        desc: document.workingMessage.desc,
        sources: [],
        nodeIds: [],
        instruction: prompt,
      };

      documentRevisionRef.current += 1;
      setMessages((current) => [...current, userMessage, assistantMessage]);
      useAppStore.getState().setDesignGeneration(host, { assistantId, controller });
      setDraft("");
      // The run consumed the starting points; a second run must not silently
      // resend files the user attached for the first one. The epoch change is
      // what makes that clear final: an import already reading a file finishes
      // into this emptied composer and is abandoned rather than re-adding it.
      attachmentEpochRef.current += 1;
      // The import this abandons is stopped rather than allowed to finish: its
      // result is dropped either way, and a render is seconds of work the
      // composer no longer has a use for.
      attachControllerRef.current?.abort();
      commitAttachments([]);
      setAttachmentMessages([]);
      setPermissionNotice(null);
      setSkillResultNotice(null);
      setAppliedSkillSlugs(null);
      const generationSkillSelection = skillSelectionRef.current;
      // The wire mode repeats the persisted selection id: the caller knows
      // which mode is active and states it, the host never infers intention
      // from the list shape. `all` carries no list — the host ranks the
      // corpus itself. folderPath names the folder Oracle must search, or
      // null when nothing is attached (no grounding, no notice).
      const folderPath = attachedFolderPath ?? null;
      const generationOptions =
        skillSelection.mode === "auto"
          ? {
              skillMode: "auto" as const,
              grounded,
              folderPath,
              outputMode,
              attachments,
              onAttachmentFeedback: handleAttachmentFeedback,
            }
          : skillSelection.mode === "manual"
            ? {
                skillMode: "manual" as const,
                skills: selectedSkillSlugs,
                grounded,
                folderPath,
                outputMode,
                attachments,
                onAttachmentFeedback: handleAttachmentFeedback,
              }
            : {
                skillMode: "all" as const,
                grounded,
                folderPath,
                outputMode,
                attachments,
                onAttachmentFeedback: handleAttachmentFeedback,
              };
      void generate(scopedPrompt, controller.signal, generationOptions)
        .then((result) => {
          const currentGeneration = useAppStore.getState().designSession.generation;
          if (
            controller.signal.aborted ||
            currentGeneration === null ||
            currentGeneration.assistantId !== assistantId
          ) {
            return;
          }
          generationInFlightRef.current = false;
          documentRevisionRef.current += 1;
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantId && message.role === "assistant"
                ? {
                    ...message,
                    status: "done",
                    title: result.title,
                    desc: result.desc,
                    sources: [...result.sources],
                    nodeIds: [...result.nodeIds],
                    transcript:
                      result.transcript === undefined ? message.transcript : [...result.transcript],
                    artifactHtml: result.artifactHtml,
                    artifactError: result.artifactError,
                    // The run's own mode travels with its artifact, so a later
                    // flip of the output switch cannot re-label this result.
                    outputMode: result.outputMode,
                    // The number of blocks the reply carried travels with its
                    // artifact, so the notice states what that run actually sent
                    // and not what a later reply happened to contain.
                    fencedHtmlBlockCount: result.fencedHtmlBlockCount,
                    groundingNotice: result.groundingNotice ?? null,
                    groundingNoticeDetail: result.groundingNoticeDetail ?? null,
                    instruction: prompt,
                  }
                : message,
            ),
          );
          const historyWrite =
            result.artifactHtml !== undefined && useAppStore.getState().designSession.host === host
              ? recordDesignHistoryEntry({
                  sessionId: result.sessionId,
                  peerSessionId: result.peerSessionId,
                  createdAtMs: result.createdAtMs,
                  title: prompt.trim(),
                  savedAtMs: Date.now(),
                  origin: "design",
                })
              : null;
          if (historyWrite !== null) {
            void historyWrite.then(
              (saved) => {
                reportPersistence("history", saved);
                if (mountedRef.current) setHistoryRefreshKey((current) => current + 1);
              },
              // A rejected write is "we do not know": no notice, but the list still refreshes.
              () => {
                if (mountedRef.current) setHistoryRefreshKey((current) => current + 1);
              },
            );
          }
          if (
            skillSelectionRef.current === generationSkillSelection &&
            result.appliedSkillSlugs !== undefined
          ) {
            const composedSkillBlock = buildSkillBlock(
              builtInSkillSources(),
              result.appliedSkillSlugs,
            );
            const droppedSkillSlugs = new Set(composedSkillBlock.dropped);
            const appliedSlugs = result.appliedSkillSlugs.filter(
              (slug) => !droppedSkillSlugs.has(slug),
            );
            const omittedSlugs = result.appliedSkillSlugs.filter((slug) =>
              droppedSkillSlugs.has(slug),
            );
            const modeCopy = SKILL_MODE_LABELS[generationSkillSelection.mode];
            const appliedSummary = appliedSlugs.length > 0 ? appliedSlugs.join(", ") : "none";
            const omittedSummary =
              omittedSlugs.length > 0
                ? ` Omitted: ${omittedSlugs.join(", ")} did not fit within the ${composedSkillBlock.ceiling.toLocaleString()}-character budget.`
                : "";
            const fallbackSummary =
              modeCopy.fallbackNotice ??
              `${modeCopy.name} choice did not happen; the most important sections that fit were used, and the rest were omitted.`;
            setAppliedSkillSlugs([...result.appliedSkillSlugs]);
            setSkillResultNotice(
              result.skillSelectionFallback
                ? `${fallbackSummary} Applied: ${appliedSummary}.${omittedSummary}`
                : `${modeCopy.name} craft: ${appliedSummary}.${omittedSummary}`,
            );
          }
          // The run's deposits are over, so its progress line goes with it. The
          // sentences about pages that did not make it stay in the composer:
          // they are the user's record of what the agent was handed.
          setAttachmentProgress(null);
          useAppStore.getState().setDesignGeneration(host, null);
          if (mountedRef.current) {
            setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          }
        })
        .catch((error: unknown) => {
          const currentGeneration = useAppStore.getState().designSession.generation;
          if (
            controller.signal.aborted ||
            currentGeneration === null ||
            currentGeneration.assistantId !== assistantId
          ) {
            return;
          }
          generationInFlightRef.current = false;
          documentRevisionRef.current += 1;
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantId && message.role === "assistant"
                ? {
                    ...message,
                    status: "error",
                    title: "Generation failed",
                    desc: error instanceof Error ? error.message : "The design generation failed.",
                    errorDetail: isSessionError(error) ? error.detail : null,
                    transcript: streamingTranscriptRef.current,
                  }
                : message,
            ),
          );
          setAttachmentProgress(null);
          useAppStore.getState().setDesignGeneration(host, null);
          if (mountedRef.current) {
            setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          }
        });
    },
    [
      attachedFolderPath,
      attachments,
      busy,
      commitAttachments,
      composerContextLayerName,
      composerContextTarget,
      daemonGone,
      document.contextPrefix,
      disposeHistoryOpen,
      document.workingMessage,
      generate,
      grounded,
      handleAttachmentFeedback,
      host,
      outputMode,
      reportPersistence,
      skillSelection.mode,
      selectedSkillSlugs,
      setMessages,
    ],
  );

  const send = useCallback(() => {
    const text = draft.trim();
    if (!text || busy || daemonGone) return;
    startGeneration(text);
  }, [busy, draft, daemonGone, startGeneration]);

  const visualCheck = useCallback(() => {
    startGeneration("Run a visual check on the canvas.");
  }, [startGeneration]);

  const handleDraftChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setDraft(event.target.value),
    [],
  );

  const handleAttachFiles = useCallback(
    (files: readonly File[], problem: string | null) => {
      attachQueueRef.current = attachQueueRef.current.then(async () => {
        // Captured at the start of the read, checked before the write: a run
        // that consumed the composer in between has moved the epoch, and this
        // import's result — both the files and the feedback about them —
        // belongs to the composer it was measured against, not the one the run
        // left behind.
        const epoch = attachmentEpochRef.current;
        // The controller makes the import stoppable rather than only cancelable
        // in theory: a run that consumes the composer aborts it instead of
        // waiting for pages that run will discard, and unmounting aborts it too.
        // The progress line is the import's own page count and leaves with it.
        const controller = new AbortController();
        attachControllerRef.current = controller;
        setAttachmentProgress(null);
        // The whole body is guarded because this promise IS the queue: if it
        // rejects, `attachQueueRef.current` becomes a rejected promise and
        // every later `.then` on it silently skips its callback. One throw
        // would kill attaching for the rest of the session — the drop zone
        // would still light up and nothing would ever happen again. The import
        // reads files and lazily loads the PDF renderer, so throwing is not
        // hypothetical: a file moved between the picker and the read, or a
        // chunk that fails to load, both land here.
        let result: Awaited<ReturnType<typeof importDesignAttachments>>;
        try {
          result = await importDesignAttachments(files, attachmentsRef.current, {
            signal: controller.signal,
            onProgress: setAttachmentProgress,
          });
        } catch (cause) {
          if (attachmentEpochRef.current !== epoch) return;
          setAttachmentMessages([
            {
              kind: "error",
              text: attachmentReadFailureMessage(files, cause),
            },
          ]);
          return;
        } finally {
          if (attachControllerRef.current === controller) attachControllerRef.current = null;
          setAttachmentProgress(null);
        }
        if (attachmentEpochRef.current !== epoch) return;
        if (result.attachments.length > 0) {
          commitAttachments([...attachmentsRef.current, ...result.attachments]);
        }
        // Replaced wholesale, including by an empty list: the feedback describes
        // the last import, and an import that had nothing to say clears what the
        // one before it said.
        setAttachmentMessages([
          ...(problem === null ? [] : [{ kind: "error" as const, text: problem }]),
          ...result.rejections.map((rejection) => ({
            kind: "error" as const,
            text: rejection.reason,
          })),
          ...result.notices.map((notice) => ({ kind: "note" as const, text: notice })),
        ]);
      });
    },
    [commitAttachments],
  );

  const handleAttachmentProblem = useCallback(
    (message: string) => setAttachmentMessages([{ kind: "error", text: message }]),
    [],
  );

  const handleRemoveAttachment = useCallback(
    // The key belongs to a pill, and a document's pill key is the document: one
    // press takes every page of it, so a deck cannot be left with a page
    // missing. A page's own id is nobody's pill key, so asking to remove one
    // removes nothing rather than half a document.
    (key: string) =>
      commitAttachments(attachmentsRef.current.filter((item) => attachmentPillKey(item) !== key)),
    [commitAttachments],
  );

  const handleComposerKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (isImeComposition(event.nativeEvent)) return;
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        send();
      }
    },
    [send],
  );

  const handleMessageAction = useCallback(
    (action: MessageAction, message: DesignMessage) => {
      if (action === "stop" && message.role === "assistant") {
        const activeGeneration = useAppStore.getState().designSession.generation;
        if (activeGeneration === null || activeGeneration.assistantId !== message.id) return;

        activeGeneration.controller.abort();
        generationInFlightRef.current = false;
        documentRevisionRef.current += 1;
        setAttachmentProgress(null);
        useAppStore.getState().setDesignGeneration(host, null);
        setMessages((current) =>
          current.map((item) =>
            item.id === message.id && item.role === "assistant"
              ? {
                  ...item,
                  status: "done",
                  title: "Stopped",
                  desc: "Cancelled before the host returned a result.",
                  transcript: streamingTranscriptRef.current,
                }
              : item,
          ),
        );
        return;
      }

      if (action === "select") {
        const nodeId = message.role === "assistant" ? message.nodeIds[0] : null;
        if (nodeId) selectLayer(nodeId);
        return;
      }

      if (action === "retry") {
        const prompt = promptForMessage(messagesRef.current, message);
        if (prompt !== null) startGeneration(prompt);
        return;
      }

      const prompt = promptForMessage(messagesRef.current, message);
      if (prompt !== null) startGeneration(prompt);
    },
    [host, selectLayer, setMessages, startGeneration],
  );

  const clearComposerContext = useCallback(() => setComposerContextLayerId(null), []);

  return (
    <section
      ref={designSurfaceRef}
      className="surface-card design-surface"
      data-screen-label="Design"
      aria-labelledby="design-surface-title"
    >
      <h1 className="design-sr-only" id="design-surface-title">
        Design
      </h1>
      <DesignToolbar
        folderControl={
          <DesignFolderControl
            folders={workspaceProjects}
            loading={workspacesLoading}
            refreshing={workspacesRefreshing}
            foldersError={workspacesError}
            selectionNotice={workspaceSelectionNotice}
            selectedWorkspaceId={selectedWorkspaceId}
            selectionUnresolved={workspaceSelectionUnresolved}
            attachedPath={attachedFolderPath}
            disabled={busy}
            attachBusy={folderAttachBusy}
            attachError={folderAttachError}
            onOpen={openWorkspacePicker}
            onSelect={selectWorkspace}
            onAttach={attachFolder}
            onUseFolder={useRegisteredFolder}
          />
        }
        grounded={grounded}
        outputMode={outputMode}
        busy={busy}
        onOutputModeChange={updateOutputMode}
        canSave={canSave}
        saved={saved}
        saving={saving}
        saveError={saveError}
        canUndo={canUndo}
        canRedo={canRedo}
        historyRefreshKey={historyRefreshKey}
        liveSessionId={agentSessionRecord?.id ?? null}
        onGroundingToggle={toggleGrounding}
        onSave={save}
        onUndo={undo}
        onRedo={redo}
        onHistoryOpen={openHistoryEntry}
      />
      {persistenceNotice ? (
        <div className="design-history-open-status" role="status">
          {persistenceNotice}
        </div>
      ) : null}

      {historyOpenResult?.status === "loading" ? (
        <div className="design-history-open-status" role="status">
          Opening design history…
        </div>
      ) : historyOpenResult?.status === "timeout" ? (
        <div className="design-history-open-status" role="status">
          {historyOpenResult.message}
        </div>
      ) : historyOpenResult?.status === "failed" ? (
        <div className="design-history-open-status" role="alert">
          <ErrorText
            sentence={historyOpenResult.message}
            detail={historyOpenResult.detail}
            id="design-history-open-failed"
          />
        </div>
      ) : null}

      {/* Always mounted: the registration must follow the boolean, not the
          mount — a conditional mount would let the signal leak. */}
      <DesignCraftSheet
        open={craftSheetMode !== null}
        skillIndex={skillIndex}
        skillSelection={skillSelection}
        selectedSkillSlugs={selectedSkillSlugs}
        resolvedSkillSlugs={resolvedSkillSlugs}
        appliedSkillSlugs={appliedSkillSlugs}
        hasResolvedComposition={hasResolvedComposition}
        skillBlock={skillBlock}
        resolvedSkillSlugSet={resolvedSkillSlugSet}
        automaticBaselineSlugSet={automaticBaselineSlugSet}
        droppedSkillSlugSet={droppedSkillSlugSet}
        readOnly={craftSheetMode === "readonly"}
        onClose={closeCraftSheet}
        onSkillToggle={handleSkillToggle}
      />

      <div className="design-main">
        <div className="design-workspace">
          <DesignCanvas
            layers={layers}
            sectionLayers={sectionLayers}
            hiddenLayerIds={snapshot.hiddenLayerIds}
            pan={pan}
            selectedLayerId={selectedLayerId}
            zoom={zoom}
            layerNotice={document.layerNotice}
            artifactHtml={artifactHtml}
            artifactError={artifactError}
            artifactMissingTokens={artifactMissingTokens}
            artifactSlideShapeNotice={artifactSlideShapeNotice}
            artifactFencedBlockNotice={artifactFencedBlockNotice}
            artifactHeight={artifactPageHeight}
            artifactContentHeight={artifactContentHeight}
            sectionHighlight={sectionHighlight}
            noteMarks={noteMarks}
            onSelectLayer={selectLayer}
            onViewportChange={handleCanvasViewportChange}
            onArtifactMeasured={handleArtifactMeasured}
          />
          {navigatorRows.length > 1 || selectedRow !== null || orphanNotes.length > 0 ? (
            <LayerPanel
              navigator={navigatorRows}
              selected={selectedRow}
              ancestors={selectedAncestors}
              onSelect={selectLayer}
              onDeselect={deselectLayer}
              onToggleVisibility={toggleLayerVisibility}
              orphanNotes={orphanNotes}
              onDeleteNote={deleteSectionNote}
              selectedSectionNotes={selectedSectionNotes}
              onAddNote={handleAddSectionNote}
            />
          ) : null}
          <ZoomControls
            zoom={zoom}
            canZoomIn={zoom < DESIGN_MAX_ZOOM}
            canZoomOut={zoom > DESIGN_MIN_ZOOM}
            onZoomIn={zoomIn}
            onZoomOut={zoomOut}
            onZoomReset={zoomReset}
            onFit={fitCanvas}
          >
            {/* The export acts on the artifact on screen, so it lives with the
                canvas controls, not the session: the 365px assistant header held
                four items already and cut the fifth ("Copy HTM", live 2026-09-11).
                The pill sizes to its content and is anchored right, so it cannot
                overflow its box toward the layer panel in the opposite corner. */}
            {artifactExportControls}
          </ZoomControls>
        </div>

        <DesignAssistant
          canGenerate={canGenerate}
          contextPrefix={document.contextPrefix}
          generationLabel={generationLabel}
          contextLayerName={composerContextLayerName}
          providers={providers}
          providersLoading={providersLoading}
          selectedProviderId={selectedProviderId}
          unavailableProviderId={unavailableProviderId}
          agentSession={agentSession}
          agentState={agentState}
          liveTranscript={streamingTranscript}
          pendingPermission={pendingPermission}
          permissionNotice={permissionNotice}
          capabilities={permissionCapabilities}
          daemonState={daemon.state}
          draft={draft}
          draftPlaceholder={
            // The document default's placeholder names a fixture layer. The context that
            // can actually be selected is the generated artifact, so the placeholder
            // is derived from what is selected instead of from the defaults.
            composerContextLayerName
              ? `Describe a change to ${composerContextLayerName}…`
              : document.noContextPlaceholder
          }
          sendLabel={busy ? "Working…" : "Generate"}
          busy={busy}
          attachments={attachments}
          attachmentMessages={attachmentMessages}
          attachmentProgress={attachmentProgress}
          messages={messages}
          assistantRef={assistantRef}
          onDraftChange={handleDraftChange}
          onComposerKeyDown={handleComposerKeyDown}
          onSend={send}
          onAttachFiles={handleAttachFiles}
          onAttachmentProblem={handleAttachmentProblem}
          onRemoveAttachment={handleRemoveAttachment}
          onVisualCheck={visualCheck}
          onClearContext={clearComposerContext}
          onMessageAction={handleMessageAction}
          onProviderSelect={selectProvider}
          onModelSelect={selectModel}
          onEffortSelect={selectEffort}
          onPermissionRespond={respondPermission}
          onEndSession={endSession}
          skillIndex={skillIndex}
          skillSelection={skillSelection}
          selectedSkillSlugs={selectedSkillSlugs}
          resolvedSkillSlugs={resolvedSkillSlugs}
          appliedSkillSlugs={appliedSkillSlugs}
          hasResolvedComposition={hasResolvedComposition}
          skillBlock={skillBlock}
          resolvedSkillSlugSet={resolvedSkillSlugSet}
          automaticBaselineSlugSet={automaticBaselineSlugSet}
          droppedSkillSlugSet={droppedSkillSlugSet}
          skillResultNotice={skillResultNotice}
          onSkillModeChange={handleSkillModeChange}
          onCraftOpen={openManualCraftSheet}
          onCraftReadMore={openCraftReadOnlySheet}
        />
      </div>
    </section>
  );
}
