import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ChangeEvent,
  KeyboardEvent,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  ReactNode,
  RefObject,
} from "react";
import type {
  DesignAssistantMessage,
  DesignDocument,
  DesignAgentSession,
  DesignHost,
  DesignLayer,
  DesignMessage,
  DesignTranscriptItem,
  PendingPermission,
  DesignRadiusOption,
  SectionNote,
} from "./designHost";
import { findUndefinedCustomProperties } from "./artifactTokenLint";
import { ArtifactRenderCritic, type ArtifactRenderCriticResult } from "./artifactRenderCritic";
import {
  getCachedArtifactSections,
  sectionsToLayers,
  setCachedArtifactSections,
  type ArtifactSection,
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
  ARTIFACT_PAGE_WIDTH,
  artifactPageHeightForCanvas,
  shouldAdaptArtifactHeight,
} from "./artifactViewport";
import {
  ARTIFACT_TOO_LARGE_MESSAGE,
  AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS,
  stripFencedHtml,
  transcriptItems,
} from "./agentHost";
import {
  MAX_AUTOMATIC_SKILL_SECTIONS,
  builtInSkillIndex,
  builtInSkillSources,
  type BuiltInSkillIndexEntry,
} from "./builtInSkills";
import {
  DEFAULT_DESIGN_SKILL_SELECTION,
  loadDesignProviderId,
  loadDesignSkillSelection,
  loadDesignWorkspaceId,
  loadStoredDesignProviderId,
  loadStoredDesignWorkspaceId,
  saveDesignProviderId,
  saveDesignSkillSelection,
  saveDesignWorkspaceId,
  selectedSlugs,
  SKILL_MODE_LABELS,
  type DesignSkillSelection,
} from "./designSettings";
import { DesignFolderControl } from "./DesignFolderControl";
import { DesignHistoryList } from "./DesignHistoryList";
import { recordDesignHistoryEntry, type DesignHistoryEntry } from "./designHistory";
import {
  openDesignHistoryEntry,
  type DesignHistoryOpenHandle,
  type DesignHistoryOpenResult,
} from "./designHistoryOpen";
import { buildSkillBlock } from "./skillLoader";
import { useProviderConsent } from "../workspace/useProviderConsent";
import { useWorkspaceDaemon } from "../workspace/workspaceDaemon";
import { chatCapableProviders, requiresConsent } from "../workspace/workspaceSessions";
import { PermissionCard } from "../../components/PermissionCard";
import {
  projectAdd,
  projectsList,
  providersList,
  reasonFromCause,
  workspaceCreate,
  workspacesList,
} from "../../lib/tauri";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { hitTest } from "../../lib/canvas/hitTest";
import { nodesBounds, type Pan } from "../../lib/canvas/viewportMath";
import { useAppStore } from "../../store/appStore";
import type { AgentSessionState } from "../../lib/agentSession";
import type {
  Project,
  ProviderInfo,
  Session,
  SessionManifest,
  SessionModel,
  Workspace,
} from "../../types/ipc";
import type { NodeRect, Point } from "../../types/geometry";
import {
  clampViewportZoom,
  createViewport,
  createViewportCommitScheduler,
  DESIGN_MAX_ZOOM,
  DESIGN_MIN_ZOOM,
  fitViewport,
  panViewport,
  pointerToWorld,
  viewportTransform,
  zoomViewport,
  type DesignViewport,
} from "./designViewport";
import "./design.css";
import "./designSession.css";

export type { DesignDocument, DesignHost } from "./designHost";

type MessageAction = "stop" | "retry" | "select" | "regenerate";

interface DesignSnapshot {
  hiddenLayerIds: readonly string[];
  layers: readonly DesignLayer[];
  radius: number;
  flat: boolean;
}

interface DesignViewState {
  // Viewport state is deliberately outside DesignHistory, so undo never moves the camera.
  pan: Pan;
  selectedLayerId: string;
  zoom: number;
}

interface DesignHistory {
  present: DesignSnapshot;
  past: DesignSnapshot[];
  future: DesignSnapshot[];
  saved: boolean;
}

interface LayerViewModel extends DesignLayer {
  selected: boolean;
  hidden: boolean;
  /** A section layer carries at least one agent note. Canvas nodes never do. */
  hasNote: boolean;
}

interface RadiusViewModel extends DesignRadiusOption {
  selected: boolean;
}

interface DesignToolbarProps {
  /**
   * The folder control that owns the toolbar's left slot. It is passed in as an
   * element rather than as a dozen props because the toolbar only positions it:
   * the attachment itself is the surface's state, not the toolbar's.
   */
  folderControl: ReactNode;
  grounded: boolean;
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
  onHistoryOpen: (entry: DesignHistoryEntry) => void;
}

interface LayerPanelProps {
  layers: readonly LayerViewModel[];
  onSelect: (layerId: string) => void;
  onToggleVisibility: (layerId: string) => void;
  /** Notes whose anchor is gone from the current page; shown, not dropped. */
  orphanNotes: readonly ResolvedSectionNote[];
  onDeleteNote: (index: number) => void;
}

interface CanvasProps {
  layers: readonly DesignLayer[];
  /** Measured page sections living inside the artifact frame. */
  sectionLayers: readonly DesignLayer[];
  hiddenLayerIds: readonly string[];
  pan: Pan;
  selectedLayerId: string;
  zoom: number;
  layerNotice?: string;
  artifactHtml?: string;
  artifactError?: string;
  artifactMissingTokens: readonly string[];
  artifactHeight: number;
  /** World-space highlight for the selected page section, if it is one. */
  sectionHighlight: NodeRect | null;
  /** World-space marks for sections carrying an agent note. */
  noteMarks: readonly NodeRect[];
  onSelectLayer: (layerId: string) => void;
  onViewportChange: (viewport: DesignViewport) => void;
  onArtifactMeasured: (html: string, result: ArtifactRenderCriticResult) => void;
}

interface CanvasNodeProps {
  layer: DesignLayer;
  hidden: boolean;
  selected: boolean;
}

interface ZoomControlsProps {
  zoom: number;
  canZoomIn: boolean;
  canZoomOut: boolean;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
  onFit: () => void;
}

interface InspectorProps {
  layer: DesignLayer;
  tokenFooter: string;
  radiusOptions: readonly RadiusViewModel[];
  flat: boolean;
  onRadiusChange: (radius: number) => void;
  onElevationChange: (flat: boolean) => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onClose: () => void;
  canDuplicate: boolean;
  canDelete: boolean;
  /** Notes on the inspected section; empty for canvas layers. */
  sectionNotes: readonly ResolvedSectionNote[];
  onAddNote: (text: string) => void;
  onDeleteNote: (index: number) => void;
}

interface WorkspaceProject extends Project {
  workspaces: readonly Workspace[];
  workspaceError?: string;
}

/**
 * What ending the session costs, in one sentence. It is a tooltip rather than a
 * visible line: at 337px this sentence consumed a whole composer row on its own,
 * so the controls it explains were pushed to a third row. Both the tooltip and
 * the accessible name carry it, so hiding it from the layout does not hide it
 * from a screen reader.
 */
export const END_SESSION_EXPLANATION =
  "Ends this session and drops the agent's context for this surface.";

const WORKSPACE_NOT_REGISTERED_NOTICE = "The attached folder is no longer registered.";
const WORKSPACE_UNCONFIRMED_NOTICE =
  "The attached folder could not be confirmed because its record failed to load.";

// The persistence calls report a boolean: false means the value never reached disk and will
// revert on reload. One notice region serves all four callers because they fail the same way,
// but each message names what was lost, because the four mean different things to the user.
const PERSISTENCE_NOTICE_TEXT = {
  provider: "Your agent choice was not saved.",
  workspace: "Your folder choice was not saved.",
  skill: "Your craft selection was not saved.",
  history: "This design was not added to your history.",
} as const;

type PersistenceNoticeKind = keyof typeof PERSISTENCE_NOTICE_TEXT;

interface DesignSkillViewProps {
  skillIndex: readonly BuiltInSkillIndexEntry[];
  skillSelection: DesignSkillSelection;
  selectedSkillSlugs: readonly string[];
  resolvedSkillSlugs: readonly string[] | null;
  appliedSkillSlugs: readonly string[] | null;
  hasResolvedComposition: boolean;
  skillBlock: ReturnType<typeof buildSkillBlock>;
  resolvedSkillSlugSet: ReadonlySet<string>;
  automaticBaselineSlugSet: ReadonlySet<string>;
  droppedSkillSlugSet: ReadonlySet<string>;
}

interface AssistantProps extends DesignSkillViewProps {
  canGenerate: boolean;
  contextPrefix: string;
  generationLabel: string;
  contextLayerName: string | null;
  providers: readonly ProviderInfo[];
  providersLoading: boolean;
  selectedProviderId: string | null;
  unavailableProviderId: string | null;
  agentSession: DesignAgentSession | null;
  agentState: AgentSessionState | null;
  /** Rows streamed for the run in progress; empty when none is in progress. */
  liveTranscript: readonly DesignTranscriptItem[];
  pendingPermission: PendingPermission | null;
  permissionNotice: string | null;
  capabilities: readonly string[];
  daemonConnected: boolean;
  draft: string;
  draftPlaceholder: string;
  sendLabel: string;
  busy: boolean;
  messages: readonly DesignMessage[];
  assistantRef: RefObject<HTMLDivElement | null>;
  onDraftChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onComposerKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  onVisualCheck: () => void;
  onClearContext: () => void;
  onMessageAction: (action: MessageAction, message: DesignMessage) => void;
  onProviderSelect: (provider: ProviderInfo) => void;
  onModelSelect: (modelId: string) => void;
  onEffortSelect: (effort: string) => void;
  onPermissionRespond: (outcome: "allow_once" | "deny") => Promise<void>;
  onEndSession: () => void;
  skillResultNotice: string | null;
  onSkillModeChange: (mode: DesignSkillSelection["mode"]) => void;
  onCraftOpen: () => void;
  onCraftReadMore: () => void;
}

interface DesignCraftSheetProps extends DesignSkillViewProps {
  readOnly: boolean;
  onClose: () => void;
  onSkillToggle: (slug: string) => void;
}

const DESIGN_SKILL_MODES: readonly DesignSkillSelection["mode"][] = ["all", "manual", "auto"];

function renderCraftInline(text: string) {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((part, index) => {
    if (part.startsWith("**") && part.endsWith("**")) {
      return <strong key={index}>{part.slice(2, -2)}</strong>;
    }
    if (part.startsWith("`") && part.endsWith("`")) {
      return <code key={index}>{part.slice(1, -1)}</code>;
    }
    return part;
  });
}

function renderCraftBody(body: string) {
  return body
    .split(/\n\s*\n/)
    .map((paragraph, index) => (
      <p key={index}>{renderCraftInline(paragraph.replace(/\n/g, " "))}</p>
    ));
}

const DesignSkillModeControl = memo(function DesignSkillModeControl({
  skillSelection,
  onSkillModeChange,
  onCraftOpen,
  onCraftReadMore,
}: {
  skillSelection: DesignSkillSelection;
  onSkillModeChange: (mode: DesignSkillSelection["mode"]) => void;
  onCraftOpen: () => void;
  onCraftReadMore: () => void;
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const selectedModeRef = useRef<HTMLButtonElement>(null);
  const activeCopy = SKILL_MODE_LABELS[skillSelection.mode];

  const closePopover = useCallback(() => {
    setOpen(false);
    queueMicrotask(() => triggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!open) return;
    selectedModeRef.current?.focus();

    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      closePopover();
    };
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (
        target instanceof Node &&
        !popoverRef.current?.contains(target) &&
        !triggerRef.current?.contains(target)
      ) {
        closePopover();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [closePopover, open]);

  const chooseMode = useCallback(
    (mode: DesignSkillSelection["mode"]) => {
      if (skillSelection.mode === mode) return;
      onSkillModeChange(mode);
      closePopover();
      if (mode === "manual") onCraftOpen();
    },
    [closePopover, onCraftOpen, onSkillModeChange, skillSelection.mode],
  );

  const openCraft = useCallback(() => {
    closePopover();
    if (skillSelection.mode === "manual") onCraftOpen();
    else onCraftReadMore();
  }, [closePopover, onCraftOpen, onCraftReadMore, skillSelection.mode]);

  return (
    <div className="design-skill-controls">
      <fieldset className="design-skill-mode-fieldset">
        <legend className="design-sr-only">Craft mode</legend>
        <button
          ref={triggerRef}
          className="design-skill-mode-control"
          type="button"
          data-design-skill-mode-trigger="true"
          aria-label={`Craft mode: ${activeCopy.name} · ${activeCopy.summary ?? activeCopy.blurb}`}
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls="design-skill-picker"
          onClick={() => setOpen((current) => !current)}
        >
          <span className="design-skill-mode-control-name">{activeCopy.name}</span>
          <span className="design-skill-mode-control-chevron" aria-hidden="true">
            ⌄
          </span>
        </button>
      </fieldset>
      {open ? (
        <div
          ref={popoverRef}
          id="design-skill-picker"
          className="design-agent-picker design-skill-picker"
          role="dialog"
          aria-labelledby="design-skill-picker-title"
          tabIndex={-1}
        >
          <div className="design-agent-picker-label" id="design-skill-picker-title">
            Craft mode
          </div>
          <p className="design-skill-picker-default">
            <strong>Default:</strong> {SKILL_MODE_LABELS.all.defaultNotice}
          </p>
          <div className="design-skill-mode-options" role="radiogroup" aria-label="Craft mode">
            {DESIGN_SKILL_MODES.map((mode) => {
              const copy = SKILL_MODE_LABELS[mode];
              const selected = skillSelection.mode === mode;
              return (
                <button
                  ref={selected ? selectedModeRef : undefined}
                  className={[
                    "design-skill-mode-option",
                    `design-skill-mode-option-${mode}`,
                    mode === "all" ? "design-skill-mode-option-default" : null,
                  ]
                    .filter((className): className is string => className !== null)
                    .join(" ")}
                  key={mode}
                  type="button"
                  role="radio"
                  data-design-skill-mode={mode}
                  aria-checked={selected}
                  onClick={() => chooseMode(mode)}
                >
                  <span className="design-skill-mode-option-name">
                    {copy.name}
                    <span className="design-skill-mode-option-badge">{copy.badge}</span>
                    {selected ? (
                      <span className="design-skill-mode-option-selected">Selected</span>
                    ) : null}
                  </span>
                  <span className="design-skill-mode-option-blurb">{copy.blurb}</span>
                </button>
              );
            })}
          </div>
          <button className="design-skill-picker-action" type="button" onClick={openCraft}>
            {skillSelection.mode === "manual" ? "Choose sections…" : "Read more"}
          </button>
        </div>
      ) : null}
    </div>
  );
});

const DesignCraftSheet = memo(function DesignCraftSheet({
  skillIndex,
  skillSelection,
  selectedSkillSlugs,
  resolvedSkillSlugs,
  appliedSkillSlugs,
  hasResolvedComposition,
  skillBlock,
  resolvedSkillSlugSet,
  automaticBaselineSlugSet,
  droppedSkillSlugSet,
  readOnly,
  onClose,
  onSkillToggle,
}: DesignCraftSheetProps) {
  const [expandedSlug, setExpandedSlug] = useState<string | null>(null);
  const modeCopy = SKILL_MODE_LABELS[skillSelection.mode];
  const manualLimitReached =
    skillSelection.mode === "manual" && selectedSkillSlugs.length >= MAX_AUTOMATIC_SKILL_SECTIONS;
  const includedSkillCount = hasResolvedComposition
    ? Math.max(0, (resolvedSkillSlugs?.length ?? 0) - skillBlock.dropped.length)
    : 0;
  const droppedEntries = skillIndex.filter((entry) => {
    const isRequested = resolvedSkillSlugSet.has(entry.slug);
    return hasResolvedComposition && isRequested && droppedSkillSlugSet.has(entry.slug);
  });
  const expandedEntry = skillIndex.find((entry) => entry.slug === expandedSlug) ?? null;
  const isWaitingForAutomaticChoice = skillSelection.mode === "auto" && appliedSkillSlugs === null;
  const budgetHeading = hasResolvedComposition
    ? `${includedSkillCount} sections included`
    : "Automatic selection";
  const budgetValue = hasResolvedComposition
    ? `${skillBlock.totalChars.toLocaleString()} / ${skillBlock.ceiling.toLocaleString()} characters`
    : `up to ${MAX_AUTOMATIC_SKILL_SECTIONS} sections · ${skillBlock.ceiling.toLocaleString()}-character budget`;

  return (
    <div className="design-craft-overlay">
      <section
        className={`design-craft-sheet${expandedEntry !== null ? " design-craft-sheet-expanded" : ""}`}
        role="dialog"
        aria-labelledby="design-craft-sheet-title"
        aria-describedby="design-craft-sheet-budget"
      >
        <header className="design-craft-sheet-header">
          <div className="design-craft-sheet-heading">
            <h2 id="design-craft-sheet-title">Craft</h2>
            <span>{modeCopy.name}</span>
          </div>
          <div className="design-craft-budget" id="design-craft-sheet-budget" role="status">
            <strong>{budgetHeading}</strong>
            <span>{budgetValue}</span>
          </div>
          <button
            className="design-craft-close"
            type="button"
            aria-label="Close Craft"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        <div className="design-craft-sheet-content">
          <div className="design-craft-index">
            <div className="design-craft-index-heading">
              <span>{readOnly ? "Sections" : "Choose sections"}</span>
              {!readOnly ? (
                <span className={manualLimitReached ? "design-craft-count-limit" : ""}>
                  {selectedSkillSlugs.length} / {MAX_AUTOMATIC_SKILL_SECTIONS}
                </span>
              ) : null}
            </div>
            {readOnly && droppedEntries.length > 0 ? (
              <p className="design-craft-budget-note">
                {droppedEntries.length} sections left out; the character budget is full.
              </p>
            ) : null}
            {readOnly && isWaitingForAutomaticChoice ? (
              <p className="design-craft-budget-note">
                The agent will choose sections for this request.
              </p>
            ) : null}
            {!readOnly && manualLimitReached ? (
              <p className="design-craft-budget-note design-craft-budget-note-limit">
                Maximum reached. Clear one to choose another.
              </p>
            ) : null}
            <ul className="design-craft-title-list">
              {skillIndex.map((entry) => {
                const isSelected = selectedSkillSlugs.includes(entry.slug);
                const isAutomaticBaseline =
                  skillSelection.mode === "auto" && automaticBaselineSlugSet.has(entry.slug);
                const isRequested = resolvedSkillSlugSet.has(entry.slug);
                const isDropped =
                  hasResolvedComposition && isRequested && droppedSkillSlugSet.has(entry.slug);
                const isIncluded =
                  !isDropped && ((hasResolvedComposition && isRequested) || isAutomaticBaseline);
                const isAutomaticallyUnselected =
                  skillSelection.mode === "auto" &&
                  appliedSkillSlugs !== null &&
                  !isRequested &&
                  !isAutomaticBaseline;
                const status = isDropped
                  ? "Left out"
                  : isAutomaticBaseline
                    ? "Always included"
                    : isWaitingForAutomaticChoice
                      ? "Chosen per request"
                      : isAutomaticallyUnselected
                        ? "Not chosen"
                        : isIncluded
                          ? "Included"
                          : "Not selected";
                const rowClass = [
                  "design-craft-title-row",
                  isSelected ? "design-craft-title-row-selected" : null,
                  isIncluded ? "design-craft-title-row-included" : null,
                  isDropped ? "design-craft-title-row-dropped" : null,
                  isAutomaticallyUnselected ? "design-craft-title-row-not-selected" : null,
                ]
                  .filter((className): className is string => className !== null)
                  .join(" ");
                const detailId = `design-craft-detail-${entry.slug}`;

                return (
                  <li className={rowClass} key={entry.slug}>
                    {readOnly ? (
                      <span
                        className={`design-craft-title-mark design-craft-title-mark-${
                          isDropped ? "dropped" : isIncluded ? "included" : "pending"
                        }`}
                        aria-hidden="true"
                      />
                    ) : (
                      <input
                        type="checkbox"
                        aria-label={`Apply ${entry.title}`}
                        checked={isSelected}
                        disabled={manualLimitReached && !isSelected}
                        onChange={() => {
                          if (!manualLimitReached || isSelected) onSkillToggle(entry.slug);
                        }}
                      />
                    )}
                    <button
                      className="design-craft-title-button"
                      type="button"
                      aria-expanded={expandedSlug === entry.slug}
                      aria-controls={detailId}
                      onClick={() =>
                        setExpandedSlug((current) => (current === entry.slug ? null : entry.slug))
                      }
                    >
                      <span>{entry.title}</span>
                      {readOnly ? (
                        <span className="design-craft-title-status">{status}</span>
                      ) : null}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>

          {expandedEntry !== null ? (
            <article
              className="design-craft-detail"
              id={`design-craft-detail-${expandedEntry.slug}`}
            >
              <h3>{expandedEntry.title}</h3>
              <p>{expandedEntry.description}</p>
              <div className="design-craft-detail-body">{renderCraftBody(expandedEntry.body)}</div>
            </article>
          ) : null}
        </div>
      </section>
    </div>
  );
});

type SnapshotChange = (current: DesignSnapshot) => DesignSnapshot | null;

const EMPTY_DESIGN_MESSAGES: readonly DesignMessage[] = [];
const EMPTY_TRANSCRIPT: readonly DesignTranscriptItem[] = [];
const EMPTY_SECTIONS: readonly ArtifactSection[] = [];
const EMPTY_SECTION_NOTES: readonly SectionNote[] = [];
const EMPTY_RESOLVED_NOTES: readonly ResolvedSectionNote[] = [];
const HISTORY_OPEN_MESSAGE_PREFIX = "design-history-open-";

function isHistoryOpenMessage(message: DesignMessage): boolean {
  return message.id.startsWith(HISTORY_OPEN_MESSAGE_PREFIX);
}

function cloneMessages(document: DesignDocument): DesignMessage[] {
  return cloneMessageList(document.messages).map((message) =>
    normalizeIncompleteMessage(message, "loaded"),
  );
}

function cloneMessageList(messages: readonly DesignMessage[]): DesignMessage[] {
  return messages.map((message) =>
    message.role === "user"
      ? { ...message }
      : {
          ...message,
          sources: [...message.sources],
          nodeIds: [...message.nodeIds],
          ...(message.transcript === undefined ? {} : { transcript: [...message.transcript] }),
        },
  );
}

function normalizeIncompleteMessage(
  message: DesignMessage,
  phase: "loaded" | "saved",
): DesignMessage {
  if (message.role !== "assistant" || message.status !== "working") return message;
  const boundary = phase === "saved" ? "saved" : "loaded";
  return {
    ...message,
    status: "error",
    title: "Generation incomplete",
    desc: `This generation did not complete before the document was ${boundary}.`,
  };
}

function terminalMessagesForSave(messages: readonly DesignMessage[]): DesignMessage[] {
  return cloneMessageList(messages).map((message) => normalizeIncompleteMessage(message, "saved"));
}

function messageActions(message: DesignMessage, canGenerate: boolean): readonly MessageAction[] {
  if (message.role === "user") return [];
  if (message.status === "working") return canGenerate ? ["stop"] : [];
  if (message.status === "error") return canGenerate ? ["retry"] : [];
  if (message.nodeIds.length === 0) return canGenerate ? ["regenerate"] : [];
  return canGenerate ? ["select", "regenerate"] : ["select"];
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

function isHidden(hiddenLayerIds: readonly string[], layerId: string): boolean {
  return hiddenLayerIds.includes(layerId);
}

function promptForMessage(
  messages: readonly DesignMessage[],
  message: DesignMessage,
): string | null {
  if (message.role !== "assistant") return null;
  const messageIndex = messages.findIndex((candidate) => candidate.id === message.id);
  const previousMessage = messageIndex > 0 ? messages[messageIndex - 1] : undefined;
  return previousMessage?.role === "user" ? previousMessage.text : (message.instruction ?? null);
}

const DesignToolbar = memo(function DesignToolbar({
  folderControl,
  grounded,
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

  const closeHistory = useCallback(() => {
    setHistoryOpen(false);
    queueMicrotask(() => historyTriggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!historyOpen) return;
    historyPopoverRef.current?.focus();

    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
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
            onOpen={onHistoryOpen}
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

const LayerPanel = memo(function LayerPanel({
  layers,
  onSelect,
  onToggleVisibility,
  orphanNotes,
  onDeleteNote,
}: LayerPanelProps) {
  return (
    <section className="design-layers-panel" aria-labelledby="design-layers-title">
      <div className="design-overlay-heading">
        <span id="design-layers-title">Layers</span>
        <span className="design-layer-count">{layers.length}</span>
      </div>
      <div className="design-layer-list">
        {layers.map((layer) => (
          <div
            className={`design-layer-row${layer.selected ? " design-layer-row-selected" : ""}`}
            key={layer.id}
          >
            <button
              className="design-layer-select"
              type="button"
              aria-pressed={layer.selected}
              aria-label={`Select ${layer.name}`}
              onClick={() => onSelect(layer.id)}
            >
              <span className="design-layer-kind">{layer.section?.tag ?? layer.kind}</span>
              <span
                className={`design-layer-name${layer.hidden ? " design-layer-name-hidden" : ""}`}
              >
                {layer.name}
              </span>
              {layer.hasNote ? (
                <span
                  className="design-layer-note-dot"
                  title="Has an agent note"
                  aria-label="Has an agent note"
                />
              ) : null}
            </button>
            <button
              className="design-layer-visibility"
              type="button"
              aria-pressed={!layer.hidden}
              aria-label={`${layer.hidden ? "Show" : "Hide"} ${layer.name}`}
              title="Hide / show"
              onClick={() => onToggleVisibility(layer.id)}
            >
              {layer.hidden ? "◌" : "◉"}
            </button>
          </div>
        ))}
      </div>
      {orphanNotes.length > 0 ? (
        <div className="design-layer-orphans">
          <div className="design-layer-orphans-heading">Detached notes ({orphanNotes.length})</div>
          <ul className="design-layer-orphans-list">
            {orphanNotes.map((entry) => (
              <li key={`${entry.note.anchor}:${entry.index}`}>
                <span className="design-layer-orphan-badge">orphan</span>
                <span className="design-layer-orphan-anchor">{entry.note.anchor}</span>
                <span className="design-layer-orphan-text">{entry.note.text}</span>
                <button
                  type="button"
                  aria-label={`Delete detached note on ${entry.note.anchor}`}
                  onClick={() => onDeleteNote(entry.index)}
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
  );
});

const CanvasNode = memo(function CanvasNode({ layer, hidden, selected }: CanvasNodeProps) {
  return (
    <button
      className={`design-canvas-node${selected ? " design-canvas-node-selected" : ""}${hidden ? " design-canvas-node-hidden" : ""}`}
      type="button"
      style={{
        left: layer.transform.x,
        top: layer.transform.y,
        width: layer.transform.width,
        height: layer.transform.height,
      }}
      data-canvas-layer-id={layer.id}
      aria-label={`Select ${layer.name}`}
      aria-pressed={selected}
      disabled={hidden}
    >
      <div className="design-canvas-node-body">
        <div className="design-node-heading">
          <span
            className={`design-node-mark ${layer.kind === "SVG" ? "design-node-mark-purple" : "design-node-mark-terracotta"}`}
            aria-hidden="true"
          />
          <span className="design-node-title">{layer.name}</span>
          <span className="design-node-badge">{layer.kind}</span>
        </div>
        {layer.source ? (
          <div className="design-node-actions">
            <span className="design-node-primary-action" title={layer.source.path}>
              {sourceDirectory(layer.source.path)}
            </span>
          </div>
        ) : null}
      </div>
    </button>
  );
});

const ZoomControls = memo(function ZoomControls({
  zoom,
  canZoomIn,
  canZoomOut,
  onZoomIn,
  onZoomOut,
  onZoomReset,
  onFit,
}: ZoomControlsProps) {
  const zoomLabel = `${Math.round(zoom * 100)}%`;

  return (
    <div className="design-zoom-controls" aria-label="Canvas zoom">
      <button
        type="button"
        title="Zoom out"
        aria-label="Zoom out"
        onClick={onZoomOut}
        disabled={!canZoomOut}
      >
        −
      </button>
      <button
        className="design-zoom-value"
        type="button"
        title="Reset to 100%"
        aria-label="Reset zoom to 100%"
        onClick={onZoomReset}
      >
        {zoomLabel}
      </button>
      <button
        type="button"
        title="Zoom in"
        aria-label="Zoom in"
        onClick={onZoomIn}
        disabled={!canZoomIn}
      >
        +
      </button>
      <button className="design-fit-button" type="button" title="Fit canvas" onClick={onFit}>
        Fit
      </button>
    </div>
  );
});

const DESIGN_GRID_ORIGIN_X = 60;
const DESIGN_GRID_ORIGIN_Y = 46;
// The generated page is a desktop page: it is authored against the canonical
// 1280px page width (see artifactViewport). Width stays fixed so media queries
// and columns do not move; height follows the live canvas aspect so the fitted
// page fills the canvas instead of letterboxing below it.
const ARTIFACT_NODE_WIDTH = ARTIFACT_PAGE_WIDTH;
const ARTIFACT_NODE_GAP = 32;
const ARTIFACT_NODE_ID = "generated-artifact";
const ARTIFACT_CONTEXT_NAME = "Generated artifact";
const ARTIFACT_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; manifest-src 'none'";
const ARTIFACT_CSP_META = `<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_CSP}" />`;
// A gutter, not a frame. The generated page is authored at 1280px and the
// canvas next to a 366px assistant column is under 900px, so every pixel of
// margin is a pixel the page does not get: at 80 per side the page fitted at
// 58% of its true size on a canvas that had room for 70%.
const DESIGN_FIT_MARGIN = 24;

function layerRectsFor(layers: readonly DesignLayer[]): NodeRect[] {
  return layers.map((layer, index) => ({
    id: layer.id,
    x: layer.transform.x,
    y: layer.transform.y,
    w: layer.transform.width,
    h: layer.transform.height,
    z: index,
  }));
}

export function artifactNodeRect(layers: readonly DesignLayer[], height: number): NodeRect {
  // The artifact owns its origin: only canvas layers (TSX/SVG) push it down.
  // Section layers live INSIDE its frame, so they are excluded here — feeding
  // them back in would make the frame depend on the sections that depend on
  // the frame. With no canvas layers the artifact sits at the grid origin.
  const bounds = nodesBounds(layerRectsFor(layers.filter((layer) => layer.kind !== "SECTION")));
  return {
    id: ARTIFACT_NODE_ID,
    x: bounds?.x ?? DESIGN_GRID_ORIGIN_X,
    y: bounds === null ? DESIGN_GRID_ORIGIN_Y : bounds.y + bounds.h + ARTIFACT_NODE_GAP,
    w: ARTIFACT_NODE_WIDTH,
    h: height,
    // Canvas nodes only: sections are measured inside the frame, not placed on it.
    z: layers.filter((layer) => layer.kind !== "SECTION").length,
  };
}

function sourceDirectory(path: string): string {
  const separator = path.lastIndexOf("/");
  return separator > 0 ? path.slice(0, separator) : ".";
}

/**
 * Direct-on-canvas section pick. Page sections nest (a `nav` inside a
 * `header` inside the body), so several rects contain the pointer at once.
 * Rule: the SMALLEST area containing the point wins — the deepest element is
 * the one the pointer is on. The overlay buttons below are painted
 * largest-first so the smallest is on top, and the canvas click path checks
 * sections with this same helper first: both paths pick the same id, and both
 * call the shared `onSelectLayer`, so canvas selection and panel selection
 * are one state, not two.
 */
export function smallestSectionAt(
  sections: readonly DesignLayer[],
  hiddenLayerIds: readonly string[],
  point: Point,
): DesignLayer | null {
  let best: DesignLayer | null = null;
  let bestArea = Number.POSITIVE_INFINITY;
  for (const section of sections) {
    if (hiddenLayerIds.includes(section.id)) continue;
    const box = section.transform;
    if (
      point.x < box.x ||
      point.x > box.x + box.width ||
      point.y < box.y ||
      point.y > box.y + box.height
    ) {
      continue;
    }
    const area = box.width * box.height;
    if (area < bestArea) {
      best = section;
      bestArea = area;
    }
  }
  return best;
}

function artifactSrcDoc(html: string): string {
  return `${ARTIFACT_CSP_META}\n${html}`;
}

const DesignCanvas = memo(function DesignCanvas({
  layers,
  sectionLayers,
  hiddenLayerIds,
  pan,
  selectedLayerId,
  zoom,
  layerNotice,
  artifactHtml,
  artifactError,
  artifactMissingTokens,
  artifactHeight,
  sectionHighlight,
  noteMarks,
  onSelectLayer,
  onViewportChange,
  onArtifactMeasured,
}: CanvasProps) {
  const canvasRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<DesignViewport>(createViewport(zoom, pan));
  const pointerDragRef = useRef<{
    button: number;
    moved: boolean;
    pointerId: number;
    lastX: number;
    lastY: number;
  } | null>(null);
  const suppressClickRef = useRef(false);

  const viewportCommitScheduler = useMemo(
    () =>
      createViewportCommitScheduler(
        onViewportChange,
        (callback) => window.requestAnimationFrame(callback),
        (frameId) => window.cancelAnimationFrame(frameId),
      ),
    [onViewportChange],
  );

  // Pointer moves update one stage transform imperatively; React records only settled viewport changes.
  const applyViewport = useCallback((next: DesignViewport) => {
    viewportRef.current = next;
    if (stageRef.current) stageRef.current.style.transform = viewportTransform(next);
  }, []);

  useEffect(() => {
    // While a drag is active, React may receive a zoom-button update before the
    // uncommitted pan does. Preserve the imperative pan so that update composes.
    const appliedPan = pointerDragRef.current ? viewportRef.current.pan : pan;
    applyViewport(createViewport(zoom, appliedPan));
  }, [applyViewport, pan, zoom]);

  const layerRects = useMemo<NodeRect[]>(
    () => layerRectsFor(layers).filter((layer) => !hiddenLayerIds.includes(layer.id)),
    [hiddenLayerIds, layers],
  );
  const artifactRect = useMemo(
    () =>
      artifactHtml !== undefined || artifactError !== undefined
        ? artifactNodeRect(layers, artifactHeight)
        : null,
    [artifactError, artifactHtml, artifactHeight, layers],
  );
  const hitRects = useMemo<NodeRect[]>(() => {
    const rects = artifactRect === null ? [...layerRects] : [...layerRects, artifactRect];
    // Sections sit above the artifact sheet so a click inside the frame
    // selects the section, not the whole page. Same z for all: last in
    // document order wins, which is the deepest element under the pointer.
    const sectionBase = artifactRect === null ? layerRects.length : artifactRect.z + 1;
    for (const section of sectionLayers) {
      if (hiddenLayerIds.includes(section.id)) continue;
      rects.push({
        id: section.id,
        x: section.transform.x,
        y: section.transform.y,
        w: section.transform.width,
        h: section.transform.height,
        z: sectionBase,
      });
    }
    return rects;
  }, [artifactRect, layerRects, sectionLayers, hiddenLayerIds]);

  const handleCanvasClick = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      if (suppressClickRef.current) {
        suppressClickRef.current = false;
        return;
      }
      const canvas = canvasRef.current;
      if (!canvas) return;
      const bounds = canvas.getBoundingClientRect();
      const point = pointerToWorld(
        event.clientX,
        event.clientY,
        { left: bounds.left, top: bounds.top },
        viewportRef.current,
      );
      // Sections first, smallest-wins (see smallestSectionAt): the overlay
      // buttons below already resolved the same way, so a click agrees with
      // a hover whatever path it arrived on.
      const sectionHit = smallestSectionAt(sectionLayers, hiddenLayerIds, point);
      if (sectionHit !== null) {
        onSelectLayer(sectionHit.id);
        return;
      }
      let target = hitTest(point, hitRects);
      if (!target && event.target instanceof Element) {
        const clickedNode = event.target.closest<HTMLElement>("[data-canvas-layer-id]");
        const clickedRect = hitRects.find(
          (layer) => layer.id === clickedNode?.dataset.canvasLayerId,
        );
        if (clickedRect) {
          onSelectLayer(clickedRect.id);
          return;
        }
      }
      onSelectLayer(target?.id ?? "");
    },
    [hitRects, hiddenLayerIds, onSelectLayer, sectionLayers],
  );

  // Direct-on-canvas hover: one id, cleared on leave. The highlight below
  // mirrors the selected-section highlight so hover and selection read as
  // the same affordance; the selected section keeps the solid style.
  const [hoveredSectionId, setHoveredSectionId] = useState<string | null>(null);
  const hoveredHighlight = useMemo(() => {
    if (hoveredSectionId === null || hoveredSectionId === selectedLayerId) return null;
    const hovered = sectionLayers.find((section) => section.id === hoveredSectionId);
    if (hovered === undefined || hiddenLayerIds.includes(hovered.id)) return null;
    return {
      x: hovered.transform.x,
      y: hovered.transform.y,
      w: hovered.transform.width,
      h: hovered.transform.height,
    };
  }, [hoveredSectionId, sectionLayers, selectedLayerId, hiddenLayerIds]);
  // Largest-first paint order: the smallest (deepest) overlay is on top and
  // receives the pointer, matching smallestSectionAt above.
  const sectionOverlays = useMemo(
    () =>
      [...sectionLayers]
        .filter((section) => !hiddenLayerIds.includes(section.id))
        .sort(
          (left, right) =>
            right.transform.width * right.transform.height -
            left.transform.width * left.transform.height,
        ),
    [hiddenLayerIds, sectionLayers],
  );

  const handleWheel = useCallback(
    (event: WheelEvent) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      event.preventDefault();
      const bounds = canvas.getBoundingClientRect();
      const next = zoomViewport(
        viewportRef.current,
        { deltaY: event.deltaY, deltaMode: event.deltaMode },
        event.clientX - bounds.left,
        event.clientY - bounds.top,
        bounds.height,
      );
      applyViewport(next);
      viewportCommitScheduler.schedule(next);
    },
    [applyViewport, viewportCommitScheduler],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  useEffect(() => () => viewportCommitScheduler.cancel(), [viewportCommitScheduler]);

  const releasePointer = useCallback((element: HTMLDivElement, pointerId: number) => {
    try {
      if (element.hasPointerCapture(pointerId)) element.releasePointerCapture(pointerId);
    } catch {
      // Pointer capture may already be gone when the browser cancels the gesture.
    }
  }, []);

  const finishPointerDrag = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>, followsClick: boolean) => {
      const active = pointerDragRef.current;
      if (!active || active.pointerId !== event.pointerId) return;
      pointerDragRef.current = null;
      releasePointer(event.currentTarget, event.pointerId);
      if (active.moved && active.button === 0 && followsClick) suppressClickRef.current = true;
      if (active.moved) viewportCommitScheduler.flush(viewportRef.current);
    },
    [releasePointer, viewportCommitScheduler],
  );

  const handlePointerUp = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, true),
    [finishPointerDrag],
  );
  const handlePointerCancel = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, false),
    [finishPointerDrag],
  );
  const handleLostPointerCapture = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, false),
    [finishPointerDrag],
  );

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const startedOnEmptyCanvas = event.button === 0 && event.target === event.currentTarget;
    if (!startedOnEmptyCanvas && event.button !== 1) return;
    event.preventDefault();

    pointerDragRef.current = {
      button: event.button,
      moved: false,
      pointerId: event.pointerId,
      lastX: event.clientX,
      lastY: event.clientY,
    };
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      pointerDragRef.current = null;
    }
  }, []);

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const active = pointerDragRef.current;
      if (!active || active.pointerId !== event.pointerId) return;
      try {
        const delta = { x: event.clientX - active.lastX, y: event.clientY - active.lastY };
        active.lastX = event.clientX;
        active.lastY = event.clientY;
        active.moved = active.moved || delta.x !== 0 || delta.y !== 0;
        applyViewport(panViewport(viewportRef.current, delta));
      } catch {
        finishPointerDrag(event, false);
      }
    },
    [applyViewport, finishPointerDrag],
  );

  const cleanupPointerDrag = useCallback(() => {
    const active = pointerDragRef.current;
    const stage = stageRef.current;
    pointerDragRef.current = null;
    if (active && stage) releasePointer(stage, active.pointerId);
  }, [releasePointer]);

  useEffect(() => cleanupPointerDrag, [cleanupPointerDrag]);

  return (
    <div
      ref={canvasRef}
      className="design-canvas"
      aria-label="Design canvas"
      tabIndex={-1}
      onClick={handleCanvasClick}
    >
      <div className="design-canvas-grid" aria-hidden="true" />
      {layerNotice && layers.length > 0 ? (
        <div
          className="design-canvas-notice"
          role="status"
          style={{ pointerEvents: "none", zIndex: 1 }}
        >
          {layerNotice}
        </div>
      ) : null}
      {layers.length === 0 && artifactRect === null ? (
        <div className="design-canvas-empty" role="status">
          {layerNotice === undefined ? (
            <>
              <p className="design-canvas-empty-title">The canvas is empty.</p>
              <p className="design-canvas-empty-copy">
                Describe the change you want in the composer, then choose Generate. The result
                appears here.
              </p>
            </>
          ) : (
            layerNotice
          )}
        </div>
      ) : null}
      <div
        ref={stageRef}
        className="design-canvas-stage"
        style={{ transform: viewportTransform({ pan, zoom }) }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
        onLostPointerCapture={handleLostPointerCapture}
      >
        {layers.map((layer) => (
          <CanvasNode
            key={layer.id}
            layer={layer}
            hidden={isHidden(hiddenLayerIds, layer.id)}
            selected={selectedLayerId === layer.id}
          />
        ))}
        {artifactRect !== null ? (
          <div
            className={`design-canvas-artifact${selectedLayerId === ARTIFACT_NODE_ID ? " design-canvas-artifact-selected" : ""}`}
            style={{
              left: artifactRect.x,
              top: artifactRect.y,
              width: artifactRect.w,
              height: artifactRect.h,
            }}
            data-canvas-layer-id={ARTIFACT_NODE_ID}
            role="button"
            tabIndex={0}
            aria-label="Select generated artifact"
            aria-pressed={selectedLayerId === ARTIFACT_NODE_ID}
            onClick={() => onSelectLayer(ARTIFACT_NODE_ID)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onSelectLayer(ARTIFACT_NODE_ID);
              }
            }}
          >
            {artifactError ? (
              <div className="design-canvas-artifact-error" role="status">
                {artifactError}
              </div>
            ) : (
              <div className="design-canvas-artifact-content" inert>
                {/*
                  WebView2 measurement on 2026-09-05: the parent CSP is not inherited by srcdoc.
                  This policy is therefore delivered inside the frame; the sandbox remains a
                  separate boundary, and later artifact policies cannot relax this one.
                */}
                <iframe
                  sandbox=""
                  srcDoc={artifactSrcDoc(artifactHtml ?? "")}
                  title="Generated artifact"
                  className="design-artifact-frame"
                  style={{ pointerEvents: "none" }}
                />
              </div>
            )}
            {artifactMissingTokens.length > 0 || artifactHtml !== undefined ? (
              <div className="design-canvas-artifact-notices">
                {artifactMissingTokens.length > 0 ? (
                  <div className="design-canvas-artifact-token-warning" role="status">
                    This artifact references{" "}
                    {artifactMissingTokens.length === 1 ? "a token" : "tokens"} it does not define:{" "}
                    {artifactMissingTokens.join(", ")}.
                  </div>
                ) : null}
                {artifactHtml !== undefined ? (
                  <ArtifactRenderCritic html={artifactHtml} onResult={onArtifactMeasured} />
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}
        {/*
          The section highlight is drawn by the parent OVER the closed iframe
          (like CanvasNode), never inside it: page rect + artifact origin, in
          world coordinates. The artifact box clips its own content but not
          this sibling, so a section below the fold still highlights at its
          true composed position under the sheet — declared, not hidden.
        */}
        {sectionHighlight !== null ? (
          <div
            className="design-canvas-section-highlight"
            style={{
              left: sectionHighlight.x,
              top: sectionHighlight.y,
              width: sectionHighlight.w,
              height: sectionHighlight.h,
            }}
            aria-hidden="true"
          />
        ) : null}
        {hoveredHighlight !== null ? (
          <div
            className="design-canvas-section-highlight design-canvas-section-hover"
            style={{
              left: hoveredHighlight.x,
              top: hoveredHighlight.y,
              width: hoveredHighlight.w,
              height: hoveredHighlight.h,
            }}
            aria-hidden="true"
          />
        ) : null}
        {/*
          Direct-on-canvas selection zones: one transparent parent-side button
          per measured section, painted largest-first (see sectionOverlays) so
          the smallest — the deepest — is on top and receives the pointer.
          The display iframe keeps pointer-events:none and inert and is never
          touched: these siblings over it are what the pointer hits. Hover
          highlights, click selects through the shared onSelectLayer, so the
          canvas and the Layers panel are one state, not two.
        */}
        {sectionOverlays.map((section) => (
          <button
            key={section.id}
            type="button"
            className="design-canvas-section-overlay"
            style={{
              left: section.transform.x,
              top: section.transform.y,
              width: section.transform.width,
              height: section.transform.height,
            }}
            aria-label={`Select ${section.name}`}
            aria-pressed={selectedLayerId === section.id}
            onClick={(event) => {
              // The canvas click handler below would hit-test the same point
              // and pick the same id, but stopping here keeps one path.
              event.stopPropagation();
              onSelectLayer(section.id);
            }}
            onMouseEnter={() => setHoveredSectionId(section.id)}
            onMouseLeave={() =>
              setHoveredSectionId((current) => (current === section.id ? null : current))
            }
            onFocus={() => setHoveredSectionId(section.id)}
            onBlur={() =>
              setHoveredSectionId((current) => (current === section.id ? null : current))
            }
          />
        ))}
        {noteMarks.map((mark) => {
          const marked = sectionLayers.find((section) => section.id === mark.id);
          return (
            <span
              key={mark.id}
              className="design-canvas-note-mark"
              style={{ left: mark.x, top: mark.y }}
              title={marked ? `Note on ${marked.name}` : "Section note"}
              aria-hidden="true"
            />
          );
        })}
      </div>
    </div>
  );
});

const InspectorPanel = memo(function InspectorPanel({
  layer,
  tokenFooter,
  radiusOptions,
  flat,
  onRadiusChange,
  onElevationChange,
  onDuplicate,
  onDelete,
  onClose,
  canDuplicate,
  canDelete,
  sectionNotes,
  onAddNote,
  onDeleteNote,
}: InspectorProps) {
  const [noteDraft, setNoteDraft] = useState("");
  // A measured page section is not a canvas node: no transform editing, no
  // corners, no elevation, no duplicate/delete. Tag, anchor, measured size,
  // text preview, and the agent notes are what exist for it.
  if (layer.kind === "SECTION" && layer.section !== undefined) {
    const submitNote = () => {
      const text = noteDraft.trim();
      if (text.length === 0) return;
      onAddNote(text);
      setNoteDraft("");
    };
    return (
      <section className="design-inspector-panel" aria-labelledby="design-inspector-title">
        <div className="design-inspector-heading">
          <span id="design-inspector-title" className="design-inspector-name">
            {layer.name}
          </span>
          <span className="design-inspector-kind">{layer.kind}</span>
          <button
            className="design-inspector-close"
            type="button"
            aria-label="Close inspector"
            onClick={onClose}
          >
            ×
          </button>
        </div>

        <div className="design-inspector-section">
          <div className="design-inspector-label">Page section</div>
          <dl className="design-section-meta">
            <div>
              <dt>Tag</dt>
              <dd className="design-mono-value">&lt;{layer.section.tag}&gt;</dd>
            </div>
            <div>
              <dt>Anchor</dt>
              <dd className="design-mono-value">{layer.section.anchor}</dd>
            </div>
            <div>
              <dt>Measured</dt>
              <dd className="design-mono-value">
                {layer.transform.width} × {layer.transform.height} px
              </dd>
            </div>
          </dl>
          <p className="design-section-preview">{layer.name}</p>
        </div>

        <div className="design-inspector-section">
          <div className="design-inspector-label">Agent notes ({sectionNotes.length})</div>
          {sectionNotes.length > 0 ? (
            <ul className="design-section-notes">
              {sectionNotes.map((entry) => (
                <li key={entry.index}>
                  <span className="design-section-note-text">{entry.note.text}</span>
                  <button
                    type="button"
                    aria-label="Delete note"
                    onClick={() => onDeleteNote(entry.index)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          <div className="design-section-note-compose">
            <input
              type="text"
              value={noteDraft}
              maxLength={2000}
              placeholder="Note for the agent on this section…"
              aria-label="Note for the agent on this section"
              onChange={(event) => setNoteDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  submitNote();
                }
              }}
            />
            <button type="button" onClick={submitNote} disabled={noteDraft.trim().length === 0}>
              Add
            </button>
          </div>
        </div>
        <div className="design-inspector-footer">{tokenFooter}</div>
      </section>
    );
  }

  const transformFields = [
    ["X", layer.transform.x],
    ["Y", layer.transform.y],
    ["W", layer.transform.width],
    ["H", layer.transform.height],
  ] as const;

  return (
    <section className="design-inspector-panel" aria-labelledby="design-inspector-title">
      <div className="design-inspector-heading">
        <span id="design-inspector-title" className="design-inspector-name">
          {layer.name}
        </span>
        <span className="design-inspector-kind">{layer.kind}</span>
        <button
          className="design-inspector-close"
          type="button"
          aria-label="Close inspector"
          onClick={onClose}
        >
          ×
        </button>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label">Transform</div>
        <div className="design-transform-grid">
          {transformFields.map(([label, value]) => (
            <span className="design-transform-field" key={label}>
              <span className="design-field-label">{label}</span>
              <span className="design-mono-value">{value}</span>
              {label === "H" && layer.transform.hug ? (
                <span className="design-hug-label">HUG</span>
              ) : null}
            </span>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Corners</span>
          <span className="design-token-value">
            radius.{radiusOptions.find((option) => option.selected)?.token ?? "custom"}
          </span>
        </div>
        <div className="design-radius-picker">
          {radiusOptions.map((option) => (
            <button
              className={`design-radius-option${option.selected ? " design-radius-option-selected" : ""}`}
              type="button"
              key={option.token}
              title={`radius.${option.token} · ${option.value}px`}
              aria-label={`Set radius.${option.token}, ${option.value} pixels`}
              aria-pressed={option.selected}
              onClick={() => onRadiusChange(option.value)}
            >
              <span
                className="design-radius-glyph"
                style={{ borderTopLeftRadius: `${Math.min(option.value, 12)}px` }}
                aria-hidden="true"
              />
            </button>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Elevation</span>
          <span className="design-token-value">{flat ? "shadow.none" : "shadow.soft"}</span>
        </div>
        <div className="design-segmented-control">
          <button
            type="button"
            aria-pressed={!flat}
            className={!flat ? "design-segment-selected" : ""}
            onClick={() => onElevationChange(false)}
          >
            Soft
          </button>
          <button
            type="button"
            aria-pressed={flat}
            className={flat ? "design-segment-selected" : ""}
            onClick={() => onElevationChange(true)}
          >
            Flat
          </button>
        </div>
      </div>

      <div className="design-inspector-actions">
        {canDuplicate ? (
          <button type="button" onClick={onDuplicate}>
            Duplicate
          </button>
        ) : null}
        <button
          type="button"
          className="design-delete-action"
          aria-label="Delete layer"
          onClick={onDelete}
          disabled={!canDelete}
        >
          Delete
        </button>
      </div>
      <div className="design-inspector-footer">{tokenFooter}</div>
    </section>
  );
});

/**
 * One row of the agent's conversation. The kind is carried in the class name
 * because the three kinds must stay visually distinct: prose is the answer,
 * reasoning is secondary and collapsible, and tool activity is a compact line
 * that opens only when the tool reported more than its own title. The Workspace
 * chat already renders these same agent items this way; this is that pattern
 * inside the Design panel's transcript.
 */
const DesignTranscriptRow = memo(function DesignTranscriptRow({
  item,
}: {
  item: DesignTranscriptItem;
}) {
  if (item.role === "thought") {
    return (
      <details className="design-transcript-row design-transcript-thought" open>
        <summary className="design-transcript-label">Thinking</summary>
        <div className="design-transcript-text">{item.text}</div>
      </details>
    );
  }

  if (item.role === "tool") {
    const [headline, ...detail] = item.text.split("\n");
    return (
      <details className="design-transcript-row design-transcript-tool">
        <summary className="design-transcript-label">
          <span className="design-transcript-tool-line">
            <span className="design-transcript-tool-name">{headline}</span>
            <span className="design-transcript-tool-status">{item.status}</span>
          </span>
        </summary>
        {detail.length > 0 ? (
          <div className="design-transcript-text">{detail.join("\n")}</div>
        ) : null}
      </details>
    );
  }

  // The page already lives on the canvas, so a fenced ```html block would print
  // dozens of tag lines into the column. Keep only the prose around it; when
  // nothing but a block remains, render no row instead of an empty bubble.
  const prose = stripFencedHtml(item.text);
  if (prose.length === 0) return null;
  return (
    <div className="design-transcript-row design-transcript-assistant">
      <div className="design-transcript-text">{prose}</div>
    </div>
  );
});

const DesignMessageCard = memo(function DesignMessageCard({
  canGenerate,
  liveTranscript,
  message,
  onAction,
}: {
  canGenerate: boolean;
  liveTranscript: readonly DesignTranscriptItem[];
  message: DesignMessage;
  onAction: (action: MessageAction, message: DesignMessage) => void;
}) {
  if (message.role === "user") {
    return (
      <div className="design-message design-user-message-wrap">
        {message.ctx ? <div className="design-message-context">{message.ctx}</div> : null}
        <div className="design-user-message">{message.text}</div>
      </div>
    );
  }

  // A working run streams the live slice; a settled one reads the transcript the
  // host reported with its result. Reading the live slice for a settled message
  // would attach a later run's words to this run's summary.
  const transcript =
    message.status === "working" ? liveTranscript : (message.transcript ?? EMPTY_TRANSCRIPT);

  return (
    <div className="design-message-group">
      {transcript.length > 0 ? (
        <div className="design-transcript">
          {transcript.map((item) => (
            <DesignTranscriptRow item={item} key={item.id} />
          ))}
        </div>
      ) : null}
      <div className="design-message-card">
        {message.status === "done" ? (
          // A settled run states one fact once: its status and the paths it
          // reported. The count heading, the tick, and a second copy of the
          // paths in the description were ceremony, not information.
          <div className="design-message-summary">
            <span className="design-message-summary-status">{message.title}</span>
            {message.sources.map((source) => (
              <span className="design-message-source" key={source}>
                {source}
              </span>
            ))}
          </div>
        ) : (
          <div className="design-message-card-heading">
            <span
              className={`design-message-icon design-message-icon-${message.status === "working" ? "working" : "error"}`}
              aria-hidden="true"
            >
              {message.status === "working" ? "◌" : "!"}
            </span>
            <span className="design-message-title">{message.title}</span>
          </div>
        )}
        <div className="design-message-description">{message.desc}</div>
        {message.groundingNotice ? (
          <div className="design-grounding-notice" role="status">
            {message.groundingNotice}
          </div>
        ) : null}
        <div className="design-message-actions">
          {messageActions(message, canGenerate).map((action) => (
            <button type="button" key={action} onClick={() => onAction(action, message)}>
              {action === "stop"
                ? "Stop"
                : action === "retry"
                  ? "Retry"
                  : action === "select"
                    ? "Select on canvas"
                    : "Regenerate"}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
});

function manifestModel(manifest: SessionManifest | null): SessionModel | null {
  if (manifest === null || manifest.currentModelId === undefined) return null;
  return manifest.models.find((model) => model.modelId === manifest.currentModelId) ?? null;
}

function confirmedEffort(model: SessionModel | null): string {
  if (
    model?.currentEffort !== undefined &&
    model.efforts?.some((entry) => entry.id === model.currentEffort)
  ) {
    return model.currentEffort;
  }
  return "";
}

const DesignAssistant = memo(function DesignAssistant({
  canGenerate,
  contextPrefix,
  generationLabel,
  contextLayerName,
  providers,
  providersLoading,
  selectedProviderId,
  unavailableProviderId,
  agentSession,
  agentState,
  liveTranscript,
  pendingPermission,
  permissionNotice,
  capabilities,
  daemonConnected,
  draft,
  draftPlaceholder,
  sendLabel,
  busy,
  messages,
  assistantRef,
  onDraftChange,
  onComposerKeyDown,
  onSend,
  onVisualCheck,
  onClearContext,
  onMessageAction,
  onProviderSelect,
  onModelSelect,
  onEffortSelect,
  onPermissionRespond,
  onEndSession,
  skillSelection,
  skillResultNotice,
  onSkillModeChange,
  onCraftOpen,
  onCraftReadMore,
}: AssistantProps) {
  const [providerPickerOpen, setProviderPickerOpen] = useState(false);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const providerButtonRef = useRef<HTMLButtonElement>(null);
  const providerPickerWrapRef = useRef<HTMLDivElement>(null);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  const consentRestoreProviderIdRef = useRef<string | null>(null);
  const manifest = agentState?.manifest ?? null;
  const currentModel = manifestModel(manifest);
  const sessionClosed = agentState?.status === "closed";
  const sessionErrored = agentState?.status === "error";
  const sessionUnavailable = sessionClosed || sessionErrored;
  // `fail()` in agentSession.ts records the message only as an error item in the
  // transcript; the state carries no dedicated error field, so read it back here.
  let sessionErrorText: string | null = null;
  if (agentState !== null) {
    for (let index = agentState.items.length - 1; index >= 0; index -= 1) {
      const item = agentState.items[index];
      if (item.role === "error") {
        sessionErrorText = item.text;
        break;
      }
    }
  }
  // The permission request itself carries only the tool's name, so the target it
  // asks about is read back from the transcript item with the same toolCallId —
  // the same id the daemon used to correlate them. A request that arrives before
  // its tool call simply has no wording yet and gains it on the next render.
  const permissionToolTitle = useMemo(() => {
    const toolCallId = pendingPermission?.request.toolCallId;
    if (toolCallId === undefined || agentState === null) return null;
    for (let index = agentState.items.length - 1; index >= 0; index -= 1) {
      const item = agentState.items[index];
      if (item.role === "tool" && item.toolCallId === toolCallId) return item.text;
    }
    return null;
  }, [agentState, pendingPermission]);

  const modelLabel = sessionClosed
    ? "Session closed"
    : sessionErrored
      ? "Session error"
      : (currentModel?.name ??
        manifest?.currentModelId ??
        (agentState === null ? "No agent running" : "No model selected"));
  const modelButtonLabel = modelLabel === "No model selected" ? "No model" : modelLabel;
  const selectedProvider = providers.find((provider) => provider.id === selectedProviderId) ?? null;
  const providerFallback =
    unavailableProviderId === null
      ? providersLoading
        ? "Loading agents…"
        : "Choose agent"
      : `Unavailable: ${unavailableProviderId}`;
  const providerLabel = selectedProvider?.id ?? manifest?.providerId ?? providerFallback;
  const efforts = currentModel?.efforts ?? [];
  const pendingSwitch =
    agentState?.pendingSwitch !== null && agentState?.pendingSwitch !== undefined;
  const providerButtonDisabled = busy;
  const modelButtonDisabled =
    agentSession === null ||
    sessionUnavailable ||
    manifest === null ||
    manifest.models.length === 0;
  const modelButtonUnavailableLabel =
    busy && agentSession === null
      ? "Starting the agent session; its models will appear shortly."
      : agentSession === null
        ? "Start a generation to see the models offered by this agent."
        : sessionClosed
          ? "The agent session has closed; start a generation to reconnect."
          : sessionErrored
            ? `Session error: ${sessionErrorText ?? "The agent reported an unknown error."} The session is still open; start a generation to continue.`
            : manifest === null
              ? "The agent is running; waiting for its model list."
              : // Interim reading until the wire carries the distinction (models as
                // Option<Vec<_>>, absent vs empty): an empty list WITH a current model is
                // self-contradictory — an agent offering no models cannot have a current
                // one — and matches the daemon's model-switch completion fallback, which
                // publishes a manifest naming the switched-to model with models: []. So
                // only an empty list with NO current model counts as evidence of absence.
                manifest.currentModelId !== undefined
                ? "The agent is running; its model list is not known yet."
                : "This agent offered no models.";

  // A picker whose button is disabled must not keep an open flag: a session can close and a
  // later one can open, and the stale flag would reopen the menu with no user action.
  useEffect(() => {
    if (providerButtonDisabled) setProviderPickerOpen(false);
  }, [providerButtonDisabled]);
  useEffect(() => {
    if (modelButtonDisabled) setModelPickerOpen(false);
  }, [modelButtonDisabled]);

  const handleConsentConfirmed = useCallback(
    (provider: ProviderInfo) => {
      onProviderSelect(provider);
      setProviderPickerOpen(false);
    },
    [onProviderSelect],
  );
  const {
    pending: consentProvider,
    request: requestConsent,
    confirm: confirmConsent,
    cancel: cancelConsent,
    inFlight: consentInFlight,
    commandLine: consentCommandLine,
  } = useProviderConsent({ onConfirmed: handleConsentConfirmed });

  useEffect(() => {
    if (!providerPickerOpen && consentProvider !== null) cancelConsent();
  }, [cancelConsent, consentProvider, providerPickerOpen]);

  const dismissProviderPicker = useCallback(() => {
    if (consentProvider !== null) {
      cancelConsent();
      return;
    }
    setProviderPickerOpen(false);
  }, [cancelConsent, consentProvider]);

  useEffect(() => {
    if (consentProvider !== null) {
      consentConfirmRef.current?.focus();
      return;
    }
    const trigger = consentRestoreRef.current;
    const providerId = consentRestoreProviderIdRef.current;
    const restoredOption =
      providerId === null
        ? null
        : [
            ...(providerPickerWrapRef.current?.querySelectorAll<HTMLButtonElement>(
              '[role="option"]',
            ) ?? []),
          ].find((option) => option.dataset.providerId === providerId);
    if (restoredOption !== undefined && restoredOption !== null) {
      restoredOption.focus();
    } else if (trigger?.isConnected) {
      trigger.focus();
    } else if (trigger !== null || providerId !== null) {
      providerButtonRef.current?.focus();
    }
    if (trigger !== null || providerId !== null) {
      consentRestoreRef.current = null;
      consentRestoreProviderIdRef.current = null;
    }
  }, [consentProvider]);

  useEffect(() => {
    if (!providerPickerOpen) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      dismissProviderPicker();
    };
    const onMouseDown = (event: globalThis.MouseEvent): void => {
      const root = providerPickerWrapRef.current;
      if (root !== null && event.target instanceof Node && !root.contains(event.target)) {
        dismissProviderPicker();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mousedown", onMouseDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mousedown", onMouseDown);
    };
  }, [dismissProviderPicker, providerPickerOpen]);

  return (
    <aside className="design-assistant" aria-labelledby="design-assistant-title">
      <div className="design-assistant-header">
        <span className="design-assistant-mark" aria-hidden="true" />
        <span id="design-assistant-title" className="design-assistant-title">
          Assistant
        </span>
        <span className="design-generation-label">{generationLabel}</span>
        {/* The session-level actions sit at the right end of the header, as one
            group, so the composer strip below keeps a single row of controls. */}
        <div className="design-assistant-actions">
          {agentSession !== null ? (
            <div className="design-session-end-control">
              {/* The explanation is a tooltip, not a line in a 337px column where it
                  claimed a row of its own. It is mirrored into aria-label because a
                  title alone is not announced; the visible label stays visible, since
                  a label that exists only in aria-label is invisible text. */}
              <button
                className="design-session-end-button"
                type="button"
                disabled={busy}
                title={END_SESSION_EXPLANATION}
                aria-label={`End session. ${END_SESSION_EXPLANATION}`}
                onClick={onEndSession}
              >
                End session
              </button>
            </div>
          ) : null}
          {canGenerate ? (
            <button
              className="design-visual-check"
              type="button"
              title="Visual check"
              aria-label="Run visual check"
              onClick={onVisualCheck}
            >
              ◉
            </button>
          ) : null}
        </div>
      </div>

      <div className="design-assistant-scroll design-scroll" ref={assistantRef}>
        {messages.map((message) => (
          <DesignMessageCard
            key={message.id}
            canGenerate={canGenerate}
            liveTranscript={liveTranscript}
            message={message}
            onAction={onMessageAction}
          />
        ))}
      </div>

      {canGenerate ? (
        <div className="design-composer-wrap">
          {busy && pendingPermission !== null ? (
            <PermissionCard
              sessionId={pendingPermission.sessionId}
              subscriptionId={pendingPermission.subscriptionId}
              request={pendingPermission.request}
              toolTitle={permissionToolTitle}
              capabilities={capabilities}
              daemonState={daemonConnected ? "connected" : "disconnected"}
              onRespond={onPermissionRespond}
            />
          ) : permissionNotice !== null ? (
            <div className="permission-card-notice" role="status">
              {permissionNotice}
            </div>
          ) : null}
          {unavailableProviderId !== null ? (
            <div className="design-provider-unavailable" role="status">
              <span className="design-message-icon design-message-icon-error" aria-hidden="true">
                !
              </span>
              Remembered agent &ldquo;{unavailableProviderId}&rdquo; is no longer available. Choose
              another agent.
            </div>
          ) : null}
          {contextLayerName ? (
            <div className="design-composer-meta">
              <div className="design-composer-context">
                <span>
                  {contextPrefix} {contextLayerName}
                </span>
                <button
                  type="button"
                  title="Clear context"
                  aria-label="Clear editing context"
                  onClick={onClearContext}
                >
                  ✕
                </button>
              </div>
            </div>
          ) : null}
          <div className="design-composer">
            <div className="design-composer-input">
              <textarea
                value={draft}
                onChange={onDraftChange}
                onKeyDown={onComposerKeyDown}
                placeholder={draftPlaceholder}
                aria-label="Describe a design change"
                rows={3}
              />
              {/*
                The primary action sits beside the text, not in a row of its own.
                The composer's content box is 313px wide (366 assistant − 1 border
                − 28 composer-wrap padding − 2 composer border − 22 composer padding),
                and the four controls with their gaps need 335px, so a labelled
                button cannot join the strip below. The three selectors keep that
                strip and Generate docks at the text's bottom-right.
              */}
              <button
                className="design-generate-button"
                type="button"
                onClick={onSend}
                disabled={busy || !draft.trim()}
              >
                {sendLabel}
              </button>
            </div>
            {skillResultNotice ? (
              <div className="design-skill-result" role="status">
                {skillResultNotice}
              </div>
            ) : null}
            <div className="design-composer-footer">
              <div className="design-composer-controls">
                <DesignSkillModeControl
                  skillSelection={skillSelection}
                  onSkillModeChange={onSkillModeChange}
                  onCraftOpen={onCraftOpen}
                  onCraftReadMore={onCraftReadMore}
                />
                <div className="design-agent-picker-wrap" ref={providerPickerWrapRef}>
                  <button
                    ref={providerButtonRef}
                    className="design-provider-button"
                    type="button"
                    // A session keeps the agent it was opened with. While a generation is idle,
                    // let the user choose a new provider; that choice closes this session and a
                    // later generation opens a fresh one for the selected agent.
                    aria-label={
                      providerButtonDisabled
                        ? `Choose provider: ${providerLabel}. A generation is running; wait for it to finish to change the agent.`
                        : `Choose provider: ${providerLabel}`
                    }
                    title={
                      providerButtonDisabled
                        ? "A generation is running; wait for it to finish to change the agent."
                        : undefined
                    }
                    aria-expanded={providerButtonDisabled ? undefined : providerPickerOpen}
                    aria-controls={providerButtonDisabled ? undefined : "design-provider-picker"}
                    disabled={providerButtonDisabled}
                    onClick={() => {
                      if (consentProvider !== null) return;
                      setProviderPickerOpen((open) => !open);
                      setModelPickerOpen(false);
                    }}
                  >
                    <span className="design-provider-dot" aria-hidden="true" />
                    {providerLabel}
                    {providerButtonDisabled ? null : " ▾"}
                  </button>
                  {providerPickerOpen && !providerButtonDisabled ? (
                    <div
                      id="design-provider-picker"
                      className={`design-agent-picker${pendingSwitch ? " design-agent-picker-pending" : ""}`}
                      role={consentProvider === null ? "listbox" : "group"}
                      aria-label={consentProvider === null ? "Choose provider" : "Confirm provider"}
                    >
                      {consentProvider !== null ? (
                        <>
                          <div className="design-agent-picker-label">Confirm provider</div>
                          {/*
                          Focus moves to Confirm as soon as this card appears, so a screen reader
                          announces that button and whatever describes it — and nothing else. The
                          description therefore has to carry the command itself: approving a
                          package download while hearing only the word "Confirm" is not consent.
                        */}
                          <p className="design-agent-picker-notice" id="design-consent-notice">
                            Approve this command to download and run third-party code:
                          </p>
                          <code className="design-agent-picker-command" id="design-consent-command">
                            {consentCommandLine}
                          </code>
                          <div className="design-agent-picker-actions">
                            <button
                              type="button"
                              className="design-agent-picker-secondary"
                              onClick={cancelConsent}
                            >
                              Cancel
                            </button>
                            <button
                              ref={consentConfirmRef}
                              type="button"
                              className="design-agent-picker-primary"
                              aria-describedby="design-consent-notice design-consent-command"
                              onClick={confirmConsent}
                              disabled={consentInFlight}
                            >
                              Confirm
                            </button>
                          </div>
                        </>
                      ) : providersLoading ? (
                        <>
                          <div className="design-agent-picker-label">Choose agent</div>
                          <div className="design-agent-picker-status">Loading agents…</div>
                        </>
                      ) : providers.length === 0 ? (
                        <>
                          <div className="design-agent-picker-label">Choose agent</div>
                          <div className="design-agent-picker-status">
                            No chat-capable agents found.
                          </div>
                        </>
                      ) : (
                        <>
                          <div className="design-agent-picker-label">Choose agent</div>
                          <div className="design-agent-picker-options">
                            {providers.map((providerOption) => (
                              <button
                                type="button"
                                role="option"
                                aria-selected={providerOption.id === selectedProviderId}
                                data-provider-id={providerOption.id}
                                className="design-agent-picker-option"
                                key={providerOption.id}
                                onClick={(event) => {
                                  if (requiresConsent(providerOption)) {
                                    consentRestoreRef.current = event.currentTarget;
                                    consentRestoreProviderIdRef.current = providerOption.id;
                                    requestConsent(providerOption);
                                    return;
                                  }
                                  onProviderSelect(providerOption);
                                  setProviderPickerOpen(false);
                                }}
                              >
                                {providerOption.id}
                              </button>
                            ))}
                          </div>
                        </>
                      )}
                    </div>
                  ) : null}
                </div>
                <div className="design-agent-picker-wrap">
                  <button
                    className="design-provider-button"
                    type="button"
                    aria-label={
                      modelButtonDisabled ? modelButtonUnavailableLabel : `Model: ${modelLabel}`
                    }
                    title={modelButtonDisabled ? modelButtonUnavailableLabel : undefined}
                    aria-expanded={modelButtonDisabled ? undefined : modelPickerOpen}
                    aria-controls={modelButtonDisabled ? undefined : "design-model-picker"}
                    disabled={modelButtonDisabled}
                    onClick={() => {
                      setModelPickerOpen((open) => !open);
                      setProviderPickerOpen(false);
                    }}
                  >
                    <span className="design-provider-dot" aria-hidden="true" />
                    {modelButtonLabel}
                    {modelButtonDisabled ? null : " ▾"}
                  </button>
                  {modelButtonDisabled ? null : modelPickerOpen ? (
                    <div
                      id="design-model-picker"
                      className={`design-agent-picker${pendingSwitch ? " design-agent-picker-pending" : ""}`}
                      role="group"
                      aria-label="Choose model"
                      aria-busy={pendingSwitch}
                    >
                      <>
                        <div className="design-agent-picker-label">
                          {manifest.providerId ?? providerLabel}
                        </div>
                        {manifest.models.length > 1 ? (
                          <select
                            aria-label="Model"
                            value={manifest.currentModelId ?? ""}
                            disabled={pendingSwitch}
                            onChange={(event) => onModelSelect(event.target.value)}
                          >
                            {manifest.models.map((model) => (
                              <option key={model.modelId} value={model.modelId}>
                                {model.name}
                              </option>
                            ))}
                          </select>
                        ) : (
                          <span className="design-agent-picker-model-name">
                            {manifest.models[0].name}
                          </span>
                        )}
                        {efforts.length > 0 ? (
                          <label className="design-agent-picker-effort">
                            <span>Thinking effort</span>
                            <select
                              aria-label="Thinking effort"
                              value={confirmedEffort(currentModel)}
                              disabled={pendingSwitch}
                              onChange={(event) => onEffortSelect(event.target.value)}
                            >
                              {efforts.map((effort) => (
                                <option key={effort.id} value={effort.id}>
                                  {effort.label}
                                </option>
                              ))}
                            </select>
                          </label>
                        ) : null}
                      </>
                    </div>
                  ) : null}
                </div>
              </div>
            </div>
          </div>
          <div className="design-composer-hint">
            <b>Enter</b> to send · <b>Shift+Enter</b> for a new line
          </div>
        </div>
      ) : null}
    </aside>
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
  const daemonConnected = daemon.state === "connected";
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
    radius: document.initialState.radius,
    flat: document.initialState.flat,
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
  const [draft, setDraft] = useState(document.initialState.draft);
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
  const [workspacesError, setWorkspacesError] = useState<string | null>(null);
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
  const layerCopyCounterRef = useRef(0);
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
        setWorkspacesError(`Could not load workspaces: ${reasonFromCause(cause)}`);
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
    skillSelectionRef.current = skillSelection;
  }, [skillSelection]);

  const artifact = useAppStore((state) =>
    state.designSession.host === host ? state.designSession.latestArtifact : null,
  );
  const artifactHtml = artifact?.html;
  const artifactError = artifact?.error;
  const artifactMissingTokens = useMemo(
    () =>
      artifactHtml !== undefined && artifactError === undefined
        ? findUndefinedCustomProperties(artifactHtml)
        : [],
    [artifactError, artifactHtml],
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
  // reads straight from the cache, so navigating back keeps the layers.
  const [measuredArtifact, setMeasuredArtifact] = useState<{
    html: string;
    sections: readonly ArtifactSection[];
  } | null>(null);
  const handleArtifactMeasured = useCallback((html: string, result: ArtifactRenderCriticResult) => {
    const sections = result.structure ?? EMPTY_SECTIONS;
    setCachedArtifactSections(html, sections);
    setMeasuredArtifact({ html, sections });
  }, []);
  const artifactSections: readonly ArtifactSection[] = useMemo(() => {
    if (artifactHtml === undefined) return EMPTY_SECTIONS;
    if (measuredArtifact !== null && measuredArtifact.html === artifactHtml) {
      return measuredArtifact.sections;
    }
    return getCachedArtifactSections(artifactHtml) ?? EMPTY_SECTIONS;
  }, [artifactHtml, measuredArtifact]);
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

  useEffect(() => {
    if (craftSheetMode === null) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeCraftSheet();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeCraftSheet, craftSheetMode]);

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

  const radiusOptions = useMemo(
    () =>
      document.radiusOptions.map((option) => ({
        ...option,
        selected: option.value === snapshot.radius,
      })),
    [document.radiusOptions, snapshot.radius],
  );

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
  const [folderAttachError, setFolderAttachError] = useState<string | null>(null);

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
      setFolderAttachError(reasonFromCause(cause));
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
        setFolderAttachError(reasonFromCause(cause));
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
    (outcome: "allow_once" | "deny"): Promise<void> =>
      host.respondPermission?.(outcome) ?? Promise.resolve(),
    [host],
  );
  const endSession = useCallback(() => {
    if (busy || agentSession === null) return;
    void host.closeAgentSession?.();
  }, [agentSession, busy, host]);

  const generationCount = useMemo(
    () =>
      messages.filter(
        (message): message is DesignAssistantMessage =>
          message.role === "assistant" &&
          message.status === "done" &&
          !isHistoryOpenMessage(message),
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
  const closeInspector = useCallback(() => {
    selectLayer("");
    queueMicrotask(() => {
      designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas")?.focus();
    });
  }, [selectLayer]);

  useEffect(() => {
    if (selectedLayer === null) return;
    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
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
        '[role="dialog"], [role="listbox"], [role="group"][aria-label]',
      );
      if (escapeOwner) return;
      event.preventDefault();
      closeInspector();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [closeInspector, selectedLayer]);

  const duplicateLayer = useCallback(() => {
    const source = layers.find((layer) => layer.id === selectedLayerId);
    // Measured page sections cannot be duplicated: they describe the generated
    // page, they are not editable nodes.
    if (!source || source.kind === "SECTION") return;

    layerCopyCounterRef.current += 1;
    const copy: DesignLayer = {
      ...source,
      id: `${source.id}-copy-${layerCopyCounterRef.current}`,
      name: `${source.name} copy`,
      transform: {
        ...source.transform,
        x: source.transform.x + 16,
        y: source.transform.y + 16,
      },
    };
    commitSnapshot((current) => ({ ...current, layers: [...current.layers, copy] }));
    selectLayer(copy.id);
  }, [commitSnapshot, layers, selectedLayerId, selectLayer]);

  const deleteLayer = useCallback(() => {
    // Measured page sections cannot be deleted, for the same reason.
    if (layers.length <= 1 || selectedLayer === null || selectedLayer.kind === "SECTION") return;

    const selectedIndex = layers.findIndex((layer) => layer.id === selectedLayerId);
    const nextLayer = layers[selectedIndex + 1] ?? layers[selectedIndex - 1];
    if (!nextLayer) return;

    selectLayer(nextLayer.id);
    commitSnapshot((current) => ({
      ...current,
      layers: current.layers.filter((layer) => layer.id !== selectedLayerId),
      hiddenLayerIds: current.hiddenLayerIds.filter((id) => id !== selectedLayerId),
    }));
  }, [commitSnapshot, layers, selectedLayer, selectedLayerId, selectLayer]);

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

  const setRadius = useCallback(
    (radius: number) => {
      commitSnapshot((current) => (current.radius === radius ? null : { ...current, radius }));
    },
    [commitSnapshot],
  );

  const setElevation = useCallback(
    (flat: boolean) => {
      commitSnapshot((current) => (current.flat === flat ? null : { ...current, flat }));
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
        radius: history.present.radius,
        flat: history.present.flat,
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

  const openHistoryEntry = useCallback(
    (entry: DesignHistoryEntry) => {
      if (
        busy ||
        generationInFlightRef.current ||
        historyOpenInFlightRef.current ||
        entry.sessionId === liveSessionIdRef.current
      ) {
        return;
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
            const messageId = `${HISTORY_OPEN_MESSAGE_PREFIX}${++historyOpenMessageCounterRef.current}`;
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
                instruction: entry.title,
                ...(result.status === "artifact"
                  ? { artifactHtml: result.html }
                  : { artifactError: result.message }),
              },
            ]);
          }
        },
      });
      historyOpenRef.current = handle;
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

  const startGeneration = useCallback(
    (prompt: string) => {
      if (
        busy ||
        generate === undefined ||
        generationInFlightRef.current ||
        historyOpenInFlightRef.current
      ) {
        return;
      }
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
          ? { skillMode: "auto" as const, grounded, folderPath }
          : skillSelection.mode === "manual"
            ? { skillMode: "manual" as const, skills: selectedSkillSlugs, grounded, folderPath }
            : { skillMode: "all" as const, grounded, folderPath };
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
                    groundingNotice: result.groundingNotice ?? null,
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
                    transcript: streamingTranscriptRef.current,
                  }
                : message,
            ),
          );
          useAppStore.getState().setDesignGeneration(host, null);
          if (mountedRef.current) {
            setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          }
        });
    },
    [
      attachedFolderPath,
      busy,
      composerContextLayerName,
      composerContextTarget,
      document.contextPrefix,
      disposeHistoryOpen,
      document.workingMessage,
      generate,
      grounded,
      host,
      reportPersistence,
      skillSelection.mode,
      selectedSkillSlugs,
      setMessages,
    ],
  );

  const send = useCallback(() => {
    const text = draft.trim();
    if (!text || busy) return;
    startGeneration(text);
  }, [busy, draft, startGeneration]);

  const visualCheck = useCallback(() => {
    startGeneration("Run a visual check on the canvas.");
  }, [startGeneration]);

  const handleDraftChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setDraft(event.target.value),
    [],
  );

  const handleComposerKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
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
          {historyOpenResult.message}
        </div>
      ) : null}

      {craftSheetMode !== null ? (
        <DesignCraftSheet
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
      ) : null}

      <div className="design-main">
        <div
          className={`design-workspace${selectedLayer ? " design-workspace-inspector-open" : ""}`}
        >
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
            artifactHeight={artifactPageHeight}
            sectionHighlight={sectionHighlight}
            noteMarks={noteMarks}
            onSelectLayer={selectLayer}
            onViewportChange={handleCanvasViewportChange}
            onArtifactMeasured={handleArtifactMeasured}
          />
          {layerRows.length > 0 || orphanNotes.length > 0 ? (
            <LayerPanel
              layers={layerRows}
              onSelect={selectLayer}
              onToggleVisibility={toggleLayerVisibility}
              orphanNotes={orphanNotes}
              onDeleteNote={deleteSectionNote}
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
          />
          {selectedLayer ? (
            <InspectorPanel
              layer={selectedLayer}
              tokenFooter={document.tokenFooter}
              radiusOptions={radiusOptions}
              flat={snapshot.flat}
              onRadiusChange={setRadius}
              onElevationChange={setElevation}
              onDuplicate={duplicateLayer}
              onDelete={deleteLayer}
              onClose={closeInspector}
              canDuplicate={selectedLayer.source === undefined && selectedLayer.kind !== "SECTION"}
              canDelete={layers.length > 1 && selectedLayer.kind !== "SECTION"}
              sectionNotes={selectedSectionNotes}
              onAddNote={(text) => {
                if (selectedSectionAnchor !== null) addSectionNote(selectedSectionAnchor, text);
              }}
              onDeleteNote={deleteSectionNote}
            />
          ) : null}
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
          daemonConnected={daemonConnected}
          draft={draft}
          draftPlaceholder={
            // The mock document's placeholder names a fixture layer. The context that
            // can actually be selected is the generated artifact, so the placeholder
            // is derived from what is selected instead of from the demo host.
            composerContextLayerName
              ? `Describe a change to ${composerContextLayerName}…`
              : document.noContextPlaceholder
          }
          sendLabel={busy ? "Working…" : "Generate"}
          busy={busy}
          messages={messages}
          assistantRef={assistantRef}
          onDraftChange={handleDraftChange}
          onComposerKeyDown={handleComposerKeyDown}
          onSend={send}
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
