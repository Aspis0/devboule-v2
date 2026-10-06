// The arrow-key step, the alternate send action, the highlight reset and the
// submit-button words are translated from Paseo's app (files listed in NOTICE).
import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { KeyboardEvent as ReactKeyboardEvent, DragEvent as ReactDragEvent } from "react";
import { composerActionLabel } from "../../lib/sendBehavior";
import { COMMAND_MENU_KEY, composerChordLabel, composerKeyAction } from "../../lib/keymap";
import { isImeComposition } from "../../lib/imeComposition";
import type { AttachmentReference, PromptAttachment } from "../../types/ipc";
import { rankCommandMatches } from "./commandMatch";
import {
  acceptedImageTypes,
  ComposerAttachControl,
  MAX_COMPOSER_IMAGES,
  routePickedFiles,
} from "./ComposerAttachControl";
import type { AttachedFile } from "./useFileAttachments";
import { WorkspaceCommandMenu, type WorkspaceCommand } from "./WorkspaceCommandMenu";

/** Height cap of the growing textarea: eight 20px lines. */
const TEXTAREA_MAX_HEIGHT_PX = 160;

const COMPOSER_PLACEHOLDER = `Message the agent, or type ${COMMAND_MENU_KEY} for commands`;

/** The draft the queue hands back: applied once, then dropped. `focus` is
 * false for an Edit (the row rule owns that focus) and true for a refused
 * steer, whose text the user must look at. */
interface RestoredDraft {
  text: string;
  /** Images a failed send handed back with the text, restored as previews. */
  images?: readonly PromptAttachment[];
  focus: boolean;
  nonce: number;
}

interface WorkspaceComposerProps {
  streaming: boolean;
  turnActive: boolean;
  queueAllowed?: boolean;
  /** The connected daemon does not keep a queue for this session: the action
   * stays on screen, disabled, and says why it cannot queue. */
  queueUnsupportedReason?: string | null;
  /** The daemon agreed `attachments.gif_webp`: the picker offers GIF and WebP. */
  gifWebpSupported?: boolean;
  /** The composer's attached files, owned by the parent: they upload as they
   * arrive and clear once a send has settled. */
  files?: readonly AttachedFile[];
  onAddFiles?: (files: readonly File[]) => void;
  onRemoveFile?: (id: string) => void;
  disabled?: boolean;
  disabledReason: string | null;
  availableCommands?: readonly WorkspaceCommand[];
  onSend: (
    text: string,
    attachments: readonly PromptAttachment[],
    fileReferences?: readonly AttachmentReference[],
  ) => Promise<boolean>;
  /** Queue the composer's text while the turn runs; absent, Enter always sends.
   * Settles when the daemon has answered, which is what holds a second
   * activation of the same intent off the wire. */
  onQueue?: (
    text: string,
    attachments: readonly PromptAttachment[],
    fileReferences?: readonly AttachmentReference[],
  ) => void | Promise<unknown>;
  /** The resolved setting: Enter queues while the turn runs (the permission rule flips it to steer). */
  enterQueues?: boolean;
  onStop?: () => void;
  /** Rows above the composer, first: the agent's plan checklist. */
  taskPill?: ReactNode;
  /** Rows above the composer: the session's queued follow-ups. */
  queuedTrack?: ReactNode;
  /** Draft handed back by the queue, applied once per nonce. */
  restoreDraft?: RestoredDraft | null;
  /** Handed the textarea element so the parent can put the focus back here. */
  captureTextarea?: (element: HTMLTextAreaElement | null) => void;
  /** Pickers rendered on the left of the control bar, below the textarea. */
  controls?: ReactNode;
}

function commandQuery(input: string): string | null {
  const trimmed = input.trimStart();
  if (!trimmed.startsWith(COMMAND_MENU_KEY)) return null;
  const query = trimmed.slice(COMMAND_MENU_KEY.length);
  if (/\s/.test(query)) return null;
  return query.toLowerCase();
}

/** One hand-back's merge: refused text stacks above what is already there.
 * An empty refusal is the identity, so the merge, the park fold, the pending
 * fold and a racing keystroke that empties the field all answer the same
 * question the same way. */
function combine(refused: string, existing: string): string {
  if (refused === "") return existing;
  return existing === "" ? refused : `${refused}\n\n${existing}`;
}

/** One step from the highlighted row for an arrow key, wrapping at both ends;
 * with no rows there is no row to move to. */
function nextCommandIndex(current: number, count: number, key: "ArrowUp" | "ArrowDown"): number {
  if (count <= 0) return current;
  const step = key === "ArrowDown" ? 1 : -1;
  return (current + step + count) % count;
}

export const WorkspaceComposer = memo(function WorkspaceComposer({
  streaming,
  turnActive,
  queueAllowed = true,
  queueUnsupportedReason = null,
  gifWebpSupported = false,
  files = [],
  onAddFiles,
  onRemoveFile,
  disabled = false,
  disabledReason,
  availableCommands = [],
  onSend,
  onQueue,
  enterQueues = false,
  onStop,
  taskPill = null,
  queuedTrack = null,
  restoreDraft = null,
  captureTextarea,
  controls = null,
}: WorkspaceComposerProps) {
  const [input, setInput] = useState("");
  const [attachedImages, setAttachedImages] = useState<readonly PromptAttachment[]>([]);
  // A send with images stays in flight until the sender answers: the picked
  // images stay put (shown as sending, picker disabled) and clear only on
  // success, so a failure keeps the submission whole without a hand-back.
  const [sendingImages, setSendingImages] = useState(false);
  // Restored images that did not fit beside the current picks; named once
  // in the picker's refusal line, cleared by the next pick or removal.
  const [restoreOverflow, setRestoreOverflow] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState(false);
  const [menuDismissed, setMenuDismissed] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const composingRef = useRef(false);
  // True from a queue press until the daemon has answered it.
  const queuePendingRef = useRef(false);
  const parkedRestoreRef = useRef<{ text: string; focus: boolean } | null>(null);
  const pendingPrefixRef = useRef<string | null>(null);
  const menuId = useId();
  // A file that is not ready — an upload in flight, or a refusal the user has
  // to remove — holds the send off exactly as an in-flight image send does.
  const filesBlocked = files.some((file) => file.state !== "ready");
  const fileReferences = useMemo(
    () =>
      files
        .filter((file) => file.state === "ready" && file.reference !== undefined)
        .map((file) => file.reference as AttachmentReference),
    [files],
  );

  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea === null) return;
    textarea.style.height = "auto";
    const overflowing = textarea.scrollHeight > TEXTAREA_MAX_HEIGHT_PX;
    textarea.style.height = `${Math.min(textarea.scrollHeight, TEXTAREA_MAX_HEIGHT_PX)}px`;
    textarea.style.overflowY = overflowing ? "auto" : "hidden";
  }, [input]);

  // A hand-back merges refused text above whatever the composer holds, via a
  // functional update: the freshest state is read at process time, so a
  // keystroke queued before the merge is folded in rather than overwritten.
  // The pending marker rides along so the keystroke that races the merge —
  // onChange is the only place a keystroke lands — re-applies it instead of
  // replacing it; a second hand-back before the first is observable folds
  // above it, the same order the park uses.
  const applyHandBack = useCallback(
    (draft: { text: string; focus: boolean }) => {
      const pending = pendingPrefixRef.current;
      pendingPrefixRef.current = combine(draft.text, pending ?? "");
      setInput((current) => combine(draft.text, current));
      if (draft.focus) textareaRef.current?.focus();
    },
    [setInput],
  );

  // Primitive views of the hand-back: the effect keys on these, never on
  // `restoreDraft`'s identity and never on `input`, so an inline-literal
  // caller cannot re-fire it and no keystroke runs it either.
  const restoreNonce = restoreDraft?.nonce ?? null;
  const restoreText = restoreDraft?.text ?? null;
  const restoreFocus = restoreDraft?.focus ?? false;
  const restoreImages = restoreDraft?.images ?? null;
  // A taken queue row lands its images here, beside whatever the composer
  // already holds — capped at the picker's bound, with the overflow named
  // rather than silently dropped. Guarded by the nonce (not array
  // identity), so a re-fire is a no-op and mid-composition hand-backs
  // apply at once: unlike text this never touches the textarea.
  const appliedImagesNonceRef = useRef<number | null>(null);
  useEffect(() => {
    if (restoreNonce === null) return;
    if (appliedImagesNonceRef.current === restoreNonce) return;
    appliedImagesNonceRef.current = restoreNonce;
    if (restoreImages === null || restoreImages.length === 0) return;
    // `attachedImages` rides along so the merge measures the current picks;
    // the nonce guard above makes the extra firings no-ops.
    const room = Math.max(0, MAX_COMPOSER_IMAGES - attachedImages.length);
    const fitting = restoreImages.slice(0, room);
    if (fitting.length > 0) {
      setAttachedImages((current) => [...current, ...fitting]);
    }
    const dropped = restoreImages.length - fitting.length;
    setRestoreOverflow(
      dropped === 0 ? null : `Only ${fitting.length} restored images fit; ${dropped} omitted.`,
    );
  }, [restoreNonce, restoreImages, attachedImages]);

  // A queue hand-back (an Edit's text, a refused steer's text) is applied
  // once, keyed by its nonce — primitive deps, so a fresh object with the
  // same nonce does not re-fire it and no keystroke does either; only a
  // refused steer also takes the focus, because an Edit's focus follows the
  // row rule in the track. A late hand-back must not eat what was typed
  // meanwhile: refused text first, the newer text after, so both survive.
  // One that lands mid-composition is parked instead — writing the value now
  // would cancel the composition and drop the preedit — and applies on
  // compositionend.
  useEffect(() => {
    if (restoreNonce === null || restoreText === null) return;
    if (composingRef.current) {
      const parked = parkedRestoreRef.current;
      // Two refusals in one composition stack instead of the later one
      // replacing the earlier: nothing handed back is ever dropped.
      parkedRestoreRef.current =
        parked === null
          ? { text: restoreText, focus: restoreFocus }
          : {
              text: combine(restoreText, parked.text),
              focus: restoreFocus || parked.focus,
            };
      return;
    }
    applyHandBack({ text: restoreText, focus: restoreFocus });
  }, [restoreNonce, restoreText, restoreFocus, applyHandBack]);

  // Only an input commit runs this — an unrelated re-render leaves [input]
  // untouched. While a hand-back is pending the writers of `input` are the
  // merge itself and onChange's own combine (both always start with the
  // pending text) or send/queue/command's raw write (which never does) — so
  // startsWith clears exactly when the merge is observable and keeps the
  // marker across a clear-out.
  useLayoutEffect(() => {
    const refused = pendingPrefixRef.current;
    if (refused === null) return;
    if (input.startsWith(refused)) pendingPrefixRef.current = null;
  }, [input]);

  // The image route's landing: capped at the composer's bound, the overflow
  // named once in the refusal line instead of silently dropped.
  const addPickedImages = useCallback((picked: readonly PromptAttachment[]) => {
    setRestoreOverflow(null);
    setAttachedImages((current) => {
      const room = Math.max(0, MAX_COMPOSER_IMAGES - current.length);
      const fitting = picked.slice(0, room);
      const dropped = picked.length - fitting.length;
      if (dropped > 0) {
        setRestoreOverflow(
          `${dropped} picked image${dropped === 1 ? "" : "s"} omitted: the composer carries at most ${MAX_COMPOSER_IMAGES} images.`,
        );
      }
      return [...current, ...fitting];
    });
  }, []);

  // A drop is the picker's own partition, run from the composer's root: the
  // files that fit the image route are read here, and the rest go to the file
  // route without their bytes being touched.
  const handleDrop = useCallback(
    (event: ReactDragEvent<HTMLDivElement>) => {
      // Only a drag that carries files is ours: a text or URL drop keeps the
      // browser's own behaviour and must not be swallowed by `preventDefault`.
      if (!Array.from(event.dataTransfer?.types ?? []).includes("Files")) return;
      event.preventDefault();
      setDropTarget(false);
      const dropped = Array.from(event.dataTransfer?.files ?? []);
      if (dropped.length === 0 || disabled) return;
      const room = Math.max(0, MAX_COMPOSER_IMAGES - attachedImages.length);
      void routePickedFiles(dropped, acceptedImageTypes(gifWebpSupported), room).then((route) => {
        if (route.images.length > 0) addPickedImages(route.images);
        if (route.files.length > 0) onAddFiles?.(route.files);
      });
    },
    [addPickedImages, attachedImages.length, disabled, gifWebpSupported, onAddFiles],
  );

  // Only a drag that is carrying files is ours: a text selection dragged over
  // the composer keeps its own browser behaviour.
  const dragCarriesFiles = (event: ReactDragEvent<HTMLDivElement>): boolean =>
    Array.from(event.dataTransfer?.types ?? []).includes("Files");

  const sendInput = useCallback(() => {
    const text = input.trim();
    if (!text || disabled || sendingImages || filesBlocked) return;
    // Imageless sends keep the fire-and-forget they always had. A send that
    // carries anything else takes a snapshot and waits for the answer: the
    // picks stay put as sending and clear only on success, so a failure keeps
    // the submission whole without a hand-back. The parent clears its files
    // once its own send settles.
    if (attachedImages.length === 0 && fileReferences.length === 0) {
      onSend(text, attachedImages);
      setInput("");
      return;
    }
    const snapshot = attachedImages;
    setSendingImages(true);
    setRestoreOverflow(null);
    setInput("");
    void Promise.resolve()
      .then(() =>
        fileReferences.length === 0
          ? onSend(text, snapshot)
          : onSend(text, snapshot, fileReferences),
      )
      .then(
        (sent) => {
          setSendingImages(false);
          if (sent) {
            setAttachedImages((current) => current.filter((image) => !snapshot.includes(image)));
          }
        },
        () => setSendingImages(false),
      );
  }, [
    attachedImages,
    disabled,
    fileReferences,
    filesBlocked,
    input,
    onSend,
    sendingImages,
    setInput,
  ]);

  const queueInput = useCallback(() => {
    const text = input.trim();
    // Blocked while an image send is in flight, like the buttons: the
    // in-flight picks still belong to that send, and queueing them again
    // would send them twice.
    if (!text || disabled || sendingImages || filesBlocked || onQueue === undefined) return;
    // One intent, one frame. A second activation inside this render still reads
    // the text the first press is about to clear, so the daemon would see two
    // adds of the same words under two ids.
    if (queuePendingRef.current) return;
    queuePendingRef.current = true;
    void Promise.resolve(
      fileReferences.length === 0
        ? onQueue(text, attachedImages)
        : onQueue(text, attachedImages, fileReferences),
    )
      .catch(() => undefined)
      .finally(() => {
        queuePendingRef.current = false;
      });
    setRestoreOverflow(null);
    setAttachedImages([]);
    setInput("");
  }, [
    attachedImages,
    disabled,
    fileReferences,
    filesBlocked,
    input,
    onQueue,
    sendingImages,
    setInput,
  ]);

  const queueAvailable = turnActive && queueAllowed && !disabled && onQueue !== undefined;
  const defaultActionQueues = enterQueues && queueAvailable;

  const runDefaultAction = useCallback(() => {
    if (defaultActionQueues) queueInput();
    else sendInput();
  }, [defaultActionQueues, queueInput, sendInput]);

  // With the queue default the alternate key sends; with the steer default it
  // queues onto a running turn, and does nothing when there is no turn to
  // queue onto.
  const runAlternateAction = useCallback(() => {
    if (enterQueues) {
      sendInput();
      return;
    }
    if (queueAvailable) queueInput();
  }, [enterQueues, queueAvailable, queueInput, sendInput]);

  const query = commandQuery(input);
  const commandMatches = useMemo(
    () => (query === null ? [] : rankCommandMatches(availableCommands, query)),
    [availableCommands, query],
  );
  const matchCount = commandMatches.length;
  const commandMenuVisible = !disabled && query !== null && !menuDismissed;
  // The row the keys act on: the highlight, or the first match while the
  // highlight is out of range of the list that is on screen.
  const activeRow =
    matchCount === 0 ? -1 : activeIndex >= 0 && activeIndex < matchCount ? activeIndex : 0;
  const activeOptionId =
    commandMenuVisible && activeRow >= 0 ? `${menuId}-option-${activeRow}` : null;

  // A query change resets the highlight and clamps a row that fell out of
  // range; the Escape's dismissal rides the same line.
  const lastQueryRef = useRef(query);
  useEffect(() => {
    const queryChanged = lastQueryRef.current !== query;
    lastQueryRef.current = query;
    if (queryChanged) setMenuDismissed(false);
    setActiveIndex((current) =>
      queryChanged || current < 0 || current >= matchCount ? 0 : current,
    );
  }, [query, matchCount]);

  const selectCommand = useCallback(
    (command: WorkspaceCommand) => {
      setInput(`/${command.name} `);
      textareaRef.current?.focus();
    },
    [setInput],
  );

  const handleComposerKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
      if (isImeComposition(event.nativeEvent)) return;
      if (event.key === "Escape" && commandMenuVisible) {
        event.preventDefault();
        setMenuDismissed(true);
        return;
      }
      // The menu's own keys, and only unmodified ones: Shift keeps editing, and
      // Ctrl/Alt keep their jumps and the Q2b chord, which falls through here.
      if (
        commandMenuVisible &&
        !event.shiftKey &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey
      ) {
        if (matchCount > 0) {
          if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            const key = event.key;
            event.preventDefault();
            setActiveIndex((current) => nextCommandIndex(current, matchCount, key));
            return;
          }
          if (event.key === "Enter" || event.key === "Tab") {
            event.preventDefault();
            selectCommand(commandMatches[activeRow]);
            return;
          }
        }
      }
      const action = composerKeyAction(event.nativeEvent);
      if (action !== "submit" && action !== "alternate") return;
      event.preventDefault();
      if (action === "alternate") runAlternateAction();
      else runDefaultAction();
    },
    [
      activeRow,
      commandMatches,
      commandMenuVisible,
      matchCount,
      runAlternateAction,
      runDefaultAction,
      selectCommand,
    ],
  );

  // The submit button's words name what Enter does.
  const actionLabel = composerActionLabel(defaultActionQueues);

  return (
    <div className="workspace-composer-wrap">
      <div className="workspace-composer-track">
        {taskPill}
        {queuedTrack}
      </div>
      <WorkspaceCommandMenu
        open={commandMenuVisible}
        onClose={() => setMenuDismissed(true)}
        listId={menuId}
        commands={commandMatches}
        activeIndex={activeRow}
        activeOptionId={activeOptionId}
        onSelect={selectCommand}
      />
      <div
        className={`workspace-composer${dropTarget ? " is-drop-target" : ""}`}
        onDragEnter={(event) => {
          if (!dragCarriesFiles(event)) return;
          event.preventDefault();
          setDropTarget(true);
        }}
        onDragOver={(event) => {
          if (!dragCarriesFiles(event)) return;
          event.preventDefault();
          setDropTarget(true);
        }}
        onDragLeave={() => setDropTarget(false)}
        onDrop={handleDrop}
      >
        <textarea
          ref={(element) => {
            textareaRef.current = element;
            captureTextarea?.(element);
          }}
          value={input}
          onChange={(event) => {
            const value = event.target.value;
            const refused = pendingPrefixRef.current;
            // The DOM value cannot carry the pending refusal — a commit that
            // put it there would already have cleared the marker — so the
            // refused text is stacked back on here, where every keystroke
            // lands, instead of in a second guessing effect.
            setInput(refused === null ? value : combine(refused, value));
          }}
          onKeyDown={handleComposerKeyDown}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            const parked = parkedRestoreRef.current;
            parkedRestoreRef.current = null;
            if (parked !== null) applyHandBack(parked);
          }}
          placeholder={COMPOSER_PLACEHOLDER}
          rows={1}
          aria-label="Message the agent"
          role="combobox"
          aria-expanded={commandMenuVisible}
          aria-controls={commandMenuVisible ? menuId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={activeOptionId ?? undefined}
          disabled={disabled}
        />
        <div className="workspace-composer-bar">
          <div className="workspace-composer-controls">
            <ComposerAttachControl
              images={attachedImages}
              files={files}
              disabled={disabled}
              sending={sendingImages}
              overflowNotice={restoreOverflow}
              gifWebpSupported={gifWebpSupported}
              onAdd={addPickedImages}
              onRemove={(position) => {
                setRestoreOverflow(null);
                setAttachedImages((current) => current.filter((_, at) => at !== position));
              }}
              onAddFiles={onAddFiles}
              onRemoveFile={(id) => onRemoveFile?.(id)}
            />
            {controls}
            {disabled && disabledReason !== null ? (
              <span className="workspace-composer-hint">{disabledReason}</span>
            ) : null}
          </div>
          {queueAvailable || queueUnsupportedReason != null ? (
            <button
              type="button"
              className="workspace-queue-action"
              data-testid="composer-queue-action"
              // The reason stands in for the label while there is nothing to
              // do: an action that cannot act must not invite the click.
              title={queueUnsupportedReason ?? actionLabel}
              aria-label={queueUnsupportedReason ?? actionLabel}
              onClick={runDefaultAction}
              disabled={
                queueUnsupportedReason != null ||
                disabled ||
                sendingImages ||
                filesBlocked ||
                !input.trim()
              }
            >
              {/* A clock while the action queues: it sends later. The
                  interrupt-and-send default steers, which sends, so it wears
                  the send arrow. */}
              <svg
                width={14}
                height={14}
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={1.75}
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
                focusable="false"
              >
                {defaultActionQueues ? (
                  <>
                    <circle cx={12} cy={12} r={8} />
                    <path d="M12 12V7" />
                    <path d="M12 12H17" />
                  </>
                ) : (
                  <>
                    <path d="M12 19V5" />
                    <path d="m5 12 7-7 7 7" />
                  </>
                )}
              </svg>
            </button>
          ) : null}
          {streaming && onStop ? (
            <button
              type="button"
              className="workspace-stop-action"
              aria-label="Stop the current turn"
              title="Stop"
              onClick={onStop}
              disabled={disabled}
            >
              <svg
                width={14}
                height={14}
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={1.75}
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
                focusable="false"
              >
                <rect x={7} y={7} width={10} height={10} rx={1} />
              </svg>
            </button>
          ) : (
            <button
              type="button"
              className="workspace-send-action"
              title={`Send · ${composerChordLabel("submit")} (${composerChordLabel("newline")} for a new line)`}
              aria-label="Send"
              onClick={sendInput}
              disabled={disabled || sendingImages || filesBlocked || !input.trim()}
            >
              <svg
                width={14}
                height={14}
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={1.75}
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
                focusable="false"
              >
                <path d="M12 19V5" />
                <path d="m5 12 7-7 7 7" />
              </svg>
            </button>
          )}
        </div>
      </div>
    </div>
  );
});
