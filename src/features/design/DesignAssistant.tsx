import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ChangeEvent,
  DragEvent,
  ClipboardEvent as ReactClipboardEvent,
  KeyboardEvent,
  RefObject,
} from "react";
import { ErrorText } from "../../components/ErrorText";
import { useMenuOpen } from "../../lib/menuOpen";
import { isImeComposition } from "../../lib/imeComposition";
import type {
  DesignAgentSession,
  DesignAttachment,
  DesignAttachmentDocument,
  DesignMessage,
  DesignTranscriptItem,
  PendingPermission,
} from "./designHost";
import type { AgentSessionState } from "../../lib/agentSession";
import type { DaemonConnectionState, ProviderInfo } from "../../types/ipc";
import { PermissionCard, type PermissionAnswer } from "../../components/PermissionCard";
import { PickerChip, modeDotClass } from "../../components/PickerChip";
import {
  ATTACHMENT_INPUT_ACCEPT,
  attachmentPillKey,
  collectAttachmentFiles,
  formatAttachmentSize,
  transferCarriesFiles,
  unreadableNotice,
  type TransferLike,
} from "./designAttachments";
import { useProviderConsent } from "../workspace/useProviderConsent";
import { journalLossCopy } from "../workspace/journalLoss";
import { requiresConsent } from "../workspace/workspaceSessions";
import { DesignSkillModeControl, type DesignSkillViewProps } from "./DesignSkillControls";
import { DesignMessageCard } from "./DesignTranscript";
import { confirmedEffort, manifestModel, promptForMessage } from "./designMessageModel";
import type { AttachmentMessage, MessageAction } from "./designSurfaceTypes";
import type { DesignSkillSelection } from "./designSettings";

/**
 * What ending the session costs, in one sentence. It is a tooltip rather than a
 * visible line: at 337px this sentence consumed a whole composer row on its own,
 * so the controls it explains were pushed to a third row. Both the tooltip and
 * the accessible name carry it, so hiding it from the layout does not hide it
 * from a screen reader.
 */
export const END_SESSION_EXPLANATION =
  "Ends this session and drops the agent's context for this surface.";

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
  daemonState: DaemonConnectionState;
  draft: string;
  draftPlaceholder: string;
  sendLabel: string;
  busy: boolean;
  /** Files imported as starting points for this run, in the order shown. */
  attachments: readonly DesignAttachment[];
  /**
   * What the last import had to say: a rejection, or something the user should
   * know about a file that was attached anyway. Empty renders nothing.
   */
  attachmentMessages: readonly AttachmentMessage[];
  /**
   * The import in flight, as a page count (`deck.pdf: page 2 of 3.`), or null
   * when nothing is importing. A count rather than a spinner: the work is
   * countable, and a spinner would say less than the truth.
   */
  attachmentProgress: string | null;
  messages: readonly DesignMessage[];
  assistantRef: RefObject<HTMLDivElement | null>;
  onDraftChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onComposerKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  /**
   * Hand over files to import. `problem` is a sentence to show alongside whatever
   * the import itself has to say — a drop that also carried a folder, which the
   * importer never sees because it is handed files only.
   */
  onAttachFiles: (files: readonly File[], problem: string | null) => void;
  onAttachmentProblem: (message: string) => void;
  /**
   * Remove the pill this key belongs to: a file's own id, or — for a file that
   * arrived as several pictures — the id of the document they came from, which
   * takes every page of it away at once. Never a page id: see
   * `attachmentGroupKey`.
   */
  onRemoveAttachment: (key: string) => void;
  onVisualCheck: () => void;
  onClearContext: () => void;
  onMessageAction: (action: MessageAction, message: DesignMessage) => void;
  onProviderSelect: (provider: ProviderInfo) => void;
  onModelSelect: (modelId: string) => void;
  onEffortSelect: (effort: string) => void;
  onPermissionRespond: (response: PermissionAnswer) => Promise<void>;
  onEndSession: () => void;
  skillResultNotice: string | null;
  onSkillModeChange: (mode: DesignSkillSelection["mode"]) => void;
  onCraftOpen: () => void;
  onCraftReadMore: () => void;
}

/**
 * The `data:` URL an attachment's preview is drawn from.
 *
 * A raster carries base64 already, so its URL is that string behind the prefix
 * its measured type declares. An SVG is percent-encoded instead, and the reason
 * is not taste: `btoa` accepts only Latin-1, while the sanitized source is
 * UTF-8, so an SVG with an accented character in its title would make `btoa`
 * throw and the preview would vanish on exactly the file that is fine.
 * `encodeURIComponent` encodes the string's UTF-8 bytes, which is what a `data:`
 * URL in an HTML document is read as.
 *
 * That an attached SVG may be drawn through an <img> at all is safe for two
 * independent reasons, either of which would be enough on its own:
 *
 * 1. An SVG loaded as an image is a document in secure static mode: the browser
 *    does not run its script and does not fetch what it references. That is a
 *    rule of the image element, not a precaution taken here.
 * 2. `source` has already been through `sanitizeSvgSource`, which removes
 *    scripts, `on*` handlers, `foreignObject`, doctypes, animations that
 *    rewrite an attribute, and every off-document reference — so the value is
 *    inert before this function is ever called with it.
 *
 * The app's CSP allows the data: URL: `img-src 'self' data:
 * http://plugin.localhost` in src-tauri/tauri.conf.json.
 */
function attachmentPreviewSrc(attachment: DesignAttachment): string {
  return attachment.kind === "raster"
    ? `data:${attachment.mimeType};base64,${attachment.base64}`
    : `data:image/svg+xml,${encodeURIComponent(attachment.source)}`;
}

/**
 * What fills the preview slot when the browser could not draw the file. One
 * string, used as both the tooltip and the accessible name: the slot is empty on
 * purpose, and that is the whole of what it has to say.
 */
const PREVIEW_UNAVAILABLE_LABEL = "Preview unavailable";

/**
 * What the composer says about a file it holds but could not draw.
 *
 * A notice, not an error. The file is attached — measured, sanitized, held in
 * the composer and stated by the pill beside this sentence — and the only thing
 * that did not happen is the drawing of it. What that means is the user's call:
 * a file whose pixels they still want is worth keeping, and one whose preview is
 * blank is worth removing before a run. The danger colour is reserved for a file
 * that was not attached at all.
 */
function attachmentPreviewNotice(name: string): string {
  return `${name} was attached, but its preview could not be drawn.`;
}

/** One pill: a file the user picked, or the document that file turned into. */
interface AttachmentGroup {
  /** `attachmentPillKey` of every member, and what removal is asked for. */
  readonly key: string;
  /** The document's name, or the file's own name when it is not a document. */
  readonly name: string;
  readonly attachments: readonly DesignAttachment[];
  /** The document every member is a page of, or null for a file of its own. */
  readonly document: DesignAttachmentDocument | null;
  /** Bytes the group carries, which is what the composer's caps hold. */
  readonly bytes: number;
}

/**
 * The pills, which are not the attachments: a document is one pill however many
 * pictures it arrived as.
 *
 * A PDF is one file the user chose once, and its pages are something the
 * composer derived from it. Forty pages of one deck are not forty things they
 * attached, and a row of forty pills is one nobody can read. The pills are also
 * where removal happens, so one pill per document is what makes taking a
 * document away take all of it: a page left behind is a deck with a hole in it,
 * handed to an agent that then answers confidently and wrongly.
 *
 * Order follows the attachments: each group sits where its first member sits,
 * and a file with no document is a group of one.
 */
function attachmentGroups(attachments: readonly DesignAttachment[]): readonly AttachmentGroup[] {
  const keys: string[] = [];
  const members = new Map<string, DesignAttachment[]>();
  const sources = new Map<string, DesignAttachmentDocument>();
  for (const attachment of attachments) {
    const key = attachmentPillKey(attachment);
    const source = attachment.kind === "raster" ? attachment.document : undefined;
    const bucket = members.get(key);
    if (bucket === undefined) {
      keys.push(key);
      members.set(key, [attachment]);
      if (source !== undefined) sources.set(key, source);
    } else {
      bucket.push(attachment);
    }
  }
  return keys.map((key) => {
    const group = members.get(key) ?? [];
    const source = sources.get(key) ?? null;
    return {
      key,
      name: source?.name ?? group[0].name,
      attachments: group,
      document: source,
      bytes: group.reduce((sum, attachment) => sum + attachment.bytes, 0),
    };
  });
}

/**
 * What a pill holds: the measured type of a picture, which is the one thing the
 * `kind` slot ever said. A document's pill answers with its page count instead —
 * see `attachmentDocumentLabel`.
 */
function attachmentKindLabel(attachment: DesignAttachment): string {
  if (attachment.kind === "svg") return "SVG";
  return attachment.mimeType === "image/png" ? "PNG" : "JPEG";
}

/**
 * How many pages of a document travelled: `2 pages`, or `2 of 40 pages` when the
 * composer's budget cut the document short.
 *
 * The count is the pages that came through, never the pages the document has: a
 * pill claiming forty on a run that carries two would be the silent truncation
 * this feature exists to prevent, and the import's notice names the pages that
 * stayed behind.
 */
function attachmentDocumentLabel(source: DesignAttachmentDocument): string {
  if (source.travelled !== source.pageCount) {
    return `${source.travelled} of ${source.pageCount} pages`;
  }
  return `${source.travelled} ${source.travelled === 1 ? "page" : "pages"}`;
}

export const DesignAssistant = memo(function DesignAssistant({
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
  daemonState,
  draft,
  draftPlaceholder,
  sendLabel,
  busy,
  attachments,
  attachmentMessages,
  attachmentProgress,
  messages,
  assistantRef,
  onDraftChange,
  onComposerKeyDown,
  onSend,
  onAttachFiles,
  onAttachmentProblem,
  onRemoveAttachment,
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
  const daemonGone = daemonState !== "connected";
  const [providerPickerOpen, setProviderPickerOpen] = useState(false);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  // Drag state is tracked with a depth counter, not a boolean: dragenter and
  // dragleave fire again for every child the pointer crosses, so a boolean turns
  // the highlight off when the pointer moves from the composer onto the textarea
  // inside it. The counter is back at zero when the last leave arrives.
  const [dropActive, setDropActive] = useState(false);
  /**
   * Ids of attachments whose preview the browser could not draw.
   *
   * A failure does not take the thumbnail away: hiding it would make a file the
   * renderer choked on look exactly like a healthy one, and telling those two
   * apart is the only reason the thumbnail is here. The pill keeps the slot, and
   * the composer names the file below.
   *
   * Ids rather than the files, so every sentence about a file is read through
   * `attachments` and leaves with it — removing the pill removes its sentence,
   * with nothing to clean up by hand.
   */
  const [undrawnPreviewIds, setUndrawnPreviewIds] = useState<readonly string[]>([]);
  const reportUndrawnPreview = useCallback((id: string) => {
    setUndrawnPreviewIds((current) => (current.includes(id) ? current : [...current, id]));
  }, []);
  const dragDepthRef = useRef(0);
  const attachmentInputRef = useRef<HTMLInputElement>(null);
  const providerButtonRef = useRef<HTMLButtonElement>(null);
  const providerPickerWrapRef = useRef<HTMLDivElement>(null);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  const consentRestoreProviderIdRef = useRef<string | null>(null);
  const manifest = agentState?.manifest ?? null;
  const currentModel = manifestModel(manifest);
  const modes = manifest?.modes;
  const currentModeId = agentState?.pendingModeId ?? modes?.currentModeId ?? null;
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
      if (item.role === "tool" && item.toolCallId === toolCallId) return item.title;
    }
    return null;
  }, [agentState, pendingPermission]);

  /**
   * Files arriving by drop or by paste. Both routes run the same collector and the
   * same importer as the picker: one pipeline behind three entries, so a rule
   * cannot hold on one route and not another. Returns whether the payload was
   * claimed, which is what tells the paste handler whether to consume the event.
   */
  const attachFromTransfer = useCallback(
    (transfer: TransferLike | null): boolean => {
      const collected = collectAttachmentFiles(transfer);
      if (collected.files.length === 0 && collected.unreadable === 0) return false;
      if (collected.files.length === 0) {
        onAttachmentProblem(unreadableNotice(collected.unreadable));
        return true;
      }
      // A drop can carry both: attaching the images and saying nothing about the
      // folder beside them would hide half of what the user handed over.
      onAttachFiles(
        collected.files,
        collected.unreadable > 0 ? unreadableNotice(collected.unreadable) : null,
      );
      return true;
    },
    [onAttachFiles, onAttachmentProblem],
  );

  /**
   * Three ways in — drop, paste, picker — over one importer.
   *
   * Drop is wired on the standard HTML5 path, and on this app that path cannot fire
   * yet. Tauri replaces WebView2's own drag-drop handler unless the window declares
   * `dragDropEnabled: false`, and src-tauri/tauri.conf.json declares nothing, so the
   * default (true) stands and the browser never hands the composer a DragEvent. Paste
   * and the picker do reach the importer today. The drop handlers are kept because
   * they are the standard path and the switch is one line in a config file this slice
   * does not own; the alternative, Tauri's own drag-drop event, yields file *paths*
   * and reading those needs a filesystem capability this app does not have (only
   * core:default and dialog:default are granted). Because all three routes share the
   * importer below, no rule is missing from the two that work.
   */
  const handleDragEnter = useCallback((event: DragEvent<HTMLDivElement>) => {
    if (!transferCarriesFiles(event.dataTransfer)) return;
    dragDepthRef.current += 1;
    setDropActive(true);
  }, []);

  const handleDragOver = useCallback((event: DragEvent<HTMLDivElement>) => {
    if (!transferCarriesFiles(event.dataTransfer)) return;
    // Without this the drop event never arrives: the platform's default action on
    // a dropped file is to have the window open it, and that default is only
    // cancelled by a listener that prevents it here.
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDragLeave = useCallback(() => {
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
    if (dragDepthRef.current === 0) setDropActive(false);
  }, []);

  const handleDrop = useCallback(
    (event: DragEvent<HTMLDivElement>) => {
      dragDepthRef.current = 0;
      setDropActive(false);
      if (!transferCarriesFiles(event.dataTransfer)) return;
      event.preventDefault();
      attachFromTransfer(event.dataTransfer);
    },
    [attachFromTransfer],
  );

  const handlePaste = useCallback(
    (event: ReactClipboardEvent<HTMLTextAreaElement>) => {
      // A text paste stays a text paste: only a payload that actually carries files
      // is consumed, so pasting a paragraph still lands in the textarea.
      if (!attachFromTransfer(event.clipboardData)) return;
      event.preventDefault();
    },
    [attachFromTransfer],
  );

  const handleAttachmentInput = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const files = Array.from(event.target.files ?? []);
      // Cleared before the hand-off so that picking the same file twice fires
      // change again: a file input only reports a value that differs from its own.
      event.target.value = "";
      if (files.length > 0) onAttachFiles(files, null);
    },
    [onAttachFiles],
  );

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

  // The provider choice (and its consent gate) dismisses when the band
  // opens, like every other menu: the band is the outside press.
  useMenuOpen(providerPickerOpen && !providerButtonDisabled, dismissProviderPicker);

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
      if (isImeComposition(event)) return;
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

  // The pills, and the feedback about them. Both are read off `attachments` as
  // it stands, so a removed file takes its sentence with it and no state has to
  // be pruned — and a document's sentence is said once, for the document, since
  // the pills are what the user is looking at.
  const attachmentPills = attachmentGroups(attachments);
  const attachmentFeedback: readonly AttachmentMessage[] = [
    // The work in flight first: it is the only line here that describes what is
    // happening now rather than what happened.
    ...(attachmentProgress === null ? [] : [{ kind: "note" as const, text: attachmentProgress }]),
    ...attachmentMessages,
    ...attachmentPills.flatMap((pill) =>
      pill.attachments.some((attachment) => undrawnPreviewIds.includes(attachment.id))
        ? [{ kind: "note" as const, text: attachmentPreviewNotice(pill.name) }]
        : [],
    ),
  ];

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
              title={daemonGone ? "The agent daemon is not connected." : "Visual check"}
              aria-label="Run visual check"
              onClick={onVisualCheck}
              disabled={daemonGone}
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
            // The card offers an action only when the action has a prompt to send,
            // asked of the same helper the action uses.
            canRegenerate={promptForMessage(messages, message) !== null}
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
              daemonState={daemonState}
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
          <div
            className="design-composer"
            data-drop-active={dropActive ? "true" : undefined}
            onDragEnter={handleDragEnter}
            onDragOver={handleDragOver}
            onDragLeave={handleDragLeave}
            onDrop={handleDrop}
          >
            {attachments.length > 0 ? (
              <>
                <div className="design-attachment-row">
                  {attachmentPills.map((pill) => {
                    // The picture the pill draws: the first page of a document,
                    // or the file itself. A document's preview is its cover.
                    const lead = pill.attachments[0];
                    return (
                      <span className="design-attachment-pill" key={pill.key}>
                        {undrawnPreviewIds.includes(lead.id) ? (
                          // The same slot, emptied. Not hidden: an absent preview
                          // and a preview that failed are the two things this
                          // element exists to tell apart.
                          <span
                            className="design-attachment-preview design-attachment-preview-empty"
                            role="img"
                            aria-label={PREVIEW_UNAVAILABLE_LABEL}
                            title={PREVIEW_UNAVAILABLE_LABEL}
                          />
                        ) : (
                          <img
                            className="design-attachment-preview"
                            src={attachmentPreviewSrc(lead)}
                            // The file name is the next thing in the pill and is
                            // already read aloud; naming the image would say it
                            // twice.
                            alt=""
                            // A data: URL has nothing to defer: the bytes are
                            // already here, so waiting to decode them would only
                            // delay the one signal this element carries.
                            loading="eager"
                            onError={() => reportUndrawnPreview(lead.id)}
                          />
                        )}
                        <span className="design-attachment-name" title={pill.name}>
                          {pill.name}
                        </span>
                        <span className="design-attachment-kind">
                          {pill.document === null
                            ? attachmentKindLabel(lead)
                            : attachmentDocumentLabel(pill.document)}
                        </span>
                        <span className="design-attachment-size">
                          {formatAttachmentSize(pill.bytes)}
                        </span>
                        <button
                          className="design-attachment-remove"
                          type="button"
                          // One control, and for a document it says what it
                          // takes: every page of it, not the one under the
                          // pointer.
                          aria-label={`Remove ${pill.name}`}
                          onClick={() => onRemoveAttachment(pill.key)}
                        >
                          ✕
                        </button>
                      </span>
                    );
                  })}
                </div>
              </>
            ) : null}
            {agentState?.journalLoss ? (
              <div
                className="design-journal-notice"
                role="status"
                data-testid="design-journal-notice"
              >
                {journalLossCopy(agentState.journalLoss)}
              </div>
            ) : null}
            <div className="design-composer-input">
              <textarea
                value={draft}
                onChange={onDraftChange}
                onKeyDown={onComposerKeyDown}
                onPaste={handlePaste}
                placeholder={draftPlaceholder}
                aria-label="Describe a design change"
                rows={3}
              />
              {/*
                The primary action sits beside the text, not in a row of its own.
                The composer's content box is 313px wide (366 assistant − 1 border
                − 28 composer-wrap padding − 2 composer border − 22 composer padding),
                and the four strip controls label at 13px, which needs more than
                that column, so a labelled button cannot join the strip below.
                The three selectors keep that strip and Generate docks at the
                text's bottom-right.
              */}
              <button
                className="design-generate-button"
                type="button"
                onClick={onSend}
                disabled={busy || daemonGone || !draft.trim()}
                title={daemonGone ? "The agent daemon is not connected." : undefined}
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
                <span className="design-attach-control">
                  <button
                    className="design-attach-button"
                    type="button"
                    // Reachable by keyboard because it is a real button in the
                    // composer's own control strip; the input behind it is hidden
                    // and out of the tab order so the picker has one way in.
                    aria-label="Attach an image or an SVG as a starting point"
                    onClick={() => attachmentInputRef.current?.click()}
                  >
                    Attach
                  </button>
                  <input
                    ref={attachmentInputRef}
                    className="design-attachment-input"
                    type="file"
                    accept={ATTACHMENT_INPUT_ACCEPT}
                    multiple
                    hidden
                    onChange={handleAttachmentInput}
                  />
                </span>
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
                {modes !== undefined ? (
                  <PickerChip
                    label="Session mode"
                    options={modes.availableModes.map((mode) => ({
                      id: mode.id,
                      name: mode.name,
                      description: mode.description,
                    }))}
                    currentId={currentModeId}
                    onSelect={(modeId) => void agentSession?.setMode(modeId)}
                    chipTestId="design-mode-chip"
                    optionTestId={(id) => `design-mode-option-${id}`}
                    dotFor={modeDotClass}
                  />
                ) : null}
              </div>
            </div>
          </div>
          {attachmentFeedback.length > 0 ? (
            // Every rejected file is named here, with the reason it was rejected,
            // and every attached file whose preview did not draw. A file the user
            // handed over that vanished without a word is the one outcome this
            // feature must never produce.
            <div className="design-attachment-feedback" role="status">
              {attachmentFeedback.map((message, index) => (
                // Keyed by position on purpose: the list is replaced wholesale and
                // never reordered, while two files can produce the same sentence
                // (the same name dropped twice), which would collide on text.
                <p
                  key={`${message.kind}-${index.toString()}`}
                  className={message.kind === "error" ? "design-attachment-error" : undefined}
                >
                  <ErrorText
                    sentence={message.text}
                    detail={message.detail ?? null}
                    id={`design-attachment-feedback-${index.toString()}`}
                  />
                </p>
              ))}
            </div>
          ) : null}
          <div className="design-composer-hint">
            <b>Enter</b> to send · <b>Shift+Enter</b> for a new line · drop or paste an image to
            start from it
          </div>
        </div>
      ) : null}
    </aside>
  );
});
