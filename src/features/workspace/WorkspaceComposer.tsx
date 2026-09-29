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
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { composerActionLabel } from "../../lib/sendBehavior";
import { isImeComposition } from "../../lib/imeComposition";
import { rankCommandMatches } from "./commandMatch";
import { WorkspaceCommandMenu, type WorkspaceCommand } from "./WorkspaceCommandMenu";

/** Height cap of the growing textarea: eight 20px lines. */
const TEXTAREA_MAX_HEIGHT_PX = 160;

const COMPOSER_PLACEHOLDER = "Message the agent, or type / for commands";

/** The draft the queue hands back: applied once, then dropped. `focus` is
 * false for an Edit (the row rule owns that focus) and true for a refused
 * steer, whose text the user must look at. */
interface RestoredDraft {
  text: string;
  focus: boolean;
  nonce: number;
}

interface WorkspaceComposerProps {
  streaming: boolean;
  turnActive: boolean;
  queueAllowed?: boolean;
  disabled?: boolean;
  disabledReason: string | null;
  availableCommands?: readonly WorkspaceCommand[];
  onSend: (text: string) => void;
  /** Queue the composer's text while the turn runs; absent, Enter always sends. */
  onQueue?: (text: string) => void;
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
  /** Context ring for the control bar, between the pickers and the
      actions. */
  contextMeter?: ReactNode;
}

function commandQuery(input: string): string | null {
  const trimmed = input.trimStart();
  if (!trimmed.startsWith("/")) return null;
  const query = trimmed.slice(1);
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

/** One step from the highlighted row for an arrow key, wrapping at both ends
 * (Paseo's `getNextActiveIndex`,
 * `packages/app/src/components/ui/combobox-keyboard.ts`); with no rows there
 * is no row to move to. */
function nextCommandIndex(current: number, count: number, key: "ArrowUp" | "ArrowDown"): number {
  if (count <= 0) return current;
  const step = key === "ArrowDown" ? 1 : -1;
  return (current + step + count) % count;
}

export const WorkspaceComposer = memo(function WorkspaceComposer({
  streaming,
  turnActive,
  queueAllowed = true,
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
  contextMeter = null,
}: WorkspaceComposerProps) {
  const [input, setInput] = useState("");
  const [menuDismissed, setMenuDismissed] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const composingRef = useRef(false);
  const parkedRestoreRef = useRef<{ text: string; focus: boolean } | null>(null);
  const pendingPrefixRef = useRef<string | null>(null);
  const menuId = useId();

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

  const sendInput = useCallback(() => {
    const text = input.trim();
    if (!text || disabled) return;
    onSend(text);
    setInput("");
  }, [disabled, input, onSend, setInput]);

  const queueInput = useCallback(() => {
    const text = input.trim();
    if (!text || disabled || onQueue === undefined) return;
    onQueue(text);
    setInput("");
  }, [disabled, input, onQueue, setInput]);

  const queueAvailable = turnActive && queueAllowed && !disabled && onQueue !== undefined;
  const defaultActionQueues = enterQueues && queueAvailable;

  const runDefaultAction = useCallback(() => {
    if (defaultActionQueues) queueInput();
    else sendInput();
  }, [defaultActionQueues, queueInput, sendInput]);

  // Paseo's `runAlternateSendAction`
  // (`packages/app/src/composer/input/state.ts`): with the queue default the
  // alternate key sends; with the steer default it queues onto a running
  // turn, and does nothing when there is no turn to queue onto.
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

  // Paseo resets the highlight on a query change and clamps a row that fell
  // out of range (`packages/app/src/hooks/use-autocomplete.ts`); the
  // Escape's dismissal rides the same line.
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
      if (event.key !== "Enter" || event.shiftKey) return;
      event.preventDefault();
      if (event.ctrlKey || event.metaKey) runAlternateAction();
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

  // Paseo's submit-button words
  // (`packages/app/src/i18n/resources/en.ts`) on the button that does what
  // Enter does.
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
      <div className="workspace-composer">
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
            {controls}
            {disabled && disabledReason !== null ? (
              <span className="workspace-composer-hint">{disabledReason}</span>
            ) : null}
          </div>
          {contextMeter}
          {queueAvailable ? (
            <button
              type="button"
              className="workspace-queue-action"
              data-testid="composer-queue-action"
              title={actionLabel}
              aria-label={actionLabel}
              onClick={runDefaultAction}
              disabled={disabled || !input.trim()}
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
                strokeWidth={2}
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
                strokeWidth={2}
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
              title="Send · Enter (Shift+Enter for a new line)"
              aria-label="Send"
              onClick={sendInput}
              disabled={disabled || !input.trim()}
            >
              <svg
                width={14}
                height={14}
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2}
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
