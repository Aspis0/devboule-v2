import {
  memo,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { composerActionLabel } from "../../lib/sendBehavior";
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
  /** Rows above the composer: the session's queued follow-ups. */
  queuedTrack?: ReactNode;
  /** Draft handed back by the queue, applied once per nonce. */
  restoreDraft?: RestoredDraft | null;
  /** Handed the textarea element so the parent can put the focus back here. */
  captureTextarea?: (element: HTMLTextAreaElement | null) => void;
  /** Pickers rendered on the left of the control bar, below the textarea. */
  controls?: ReactNode;
  /** Context ring for the control bar, between the pickers and the actions —
      where the composer redesign's attach button will sit beside it. */
  contextMeter?: ReactNode;
}

function commandQuery(input: string): string | null {
  const trimmed = input.trimStart();
  if (!trimmed.startsWith("/")) return null;
  const query = trimmed.slice(1);
  if (/\s/.test(query)) return null;
  return query.toLowerCase();
}

/** One step from the highlighted row for an arrow key, wrapping at both ends
 * (Paseo's getNextActiveIndex); with no rows there is no row to move to. */
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
  const menuId = useId();

  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea === null) return;
    textarea.style.height = "auto";
    const overflowing = textarea.scrollHeight > TEXTAREA_MAX_HEIGHT_PX;
    textarea.style.height = `${Math.min(textarea.scrollHeight, TEXTAREA_MAX_HEIGHT_PX)}px`;
    textarea.style.overflowY = overflowing ? "auto" : "hidden";
  }, [input]);

  // A queue hand-back (an Edit's text, a refused steer's text) is applied
  // once, keyed by its nonce; only a refused steer also takes the focus,
  // because an Edit's focus follows the row rule in the track.
  useEffect(() => {
    if (restoreDraft === null) return;
    setInput(restoreDraft.text);
    if (restoreDraft.focus) textareaRef.current?.focus();
  }, [restoreDraft]);

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

  // Paseo's runAlternateSendAction: with the queue default the alternate key
  // sends; with the steer default it queues onto a running turn, and does
  // nothing when there is no turn to queue onto.
  const runAlternateAction = useCallback(() => {
    if (enterQueues) {
      sendInput();
      return;
    }
    if (queueAvailable) queueInput();
  }, [enterQueues, queueAvailable, queueInput, sendInput]);

  const query = commandQuery(input);
  const commandMatches = useMemo(
    () =>
      query === null
        ? []
        : availableCommands.filter((command) => command.name.toLowerCase().includes(query)),
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
  // out of range (use-autocomplete); the Escape's dismissal rides the same line.
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
      // Enter belongs to an IME while a composition is open: the keystroke
      // commits the composition, and sending here would submit the text before
      // the candidate is chosen. `isComposing` is the standard signal; the
      // legacy `keyCode === 229` covers engines that report the composition
      // commit without setting it.
      if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
      // Paseo's menu keys, with Shift kept for text editing, and Paseo's rule
      // that a menu with no rows takes none of them (so Enter still sends).
      if (commandMenuVisible && !event.shiftKey) {
        if (event.key === "Escape") {
          event.preventDefault();
          setMenuDismissed(true);
          return;
        }
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

  // Paseo's submit-button words on the button that does what Enter does.
  const actionLabel = composerActionLabel(defaultActionQueues);

  return (
    <div className="workspace-composer-wrap">
      {queuedTrack}
      {commandMenuVisible ? (
        <WorkspaceCommandMenu
          commands={commandMatches}
          activeIndex={activeRow}
          activeOptionId={activeOptionId}
          onSelect={selectCommand}
        />
      ) : null}
      <div className="workspace-composer">
        <textarea
          ref={(element) => {
            textareaRef.current = element;
            captureTextarea?.(element);
          }}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleComposerKeyDown}
          placeholder={COMPOSER_PLACEHOLDER}
          rows={1}
          aria-label="Message the agent"
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
              className="workspace-secondary-action workspace-queue-action"
              data-testid="composer-queue-action"
              title={actionLabel}
              onClick={runDefaultAction}
              disabled={disabled || !input.trim()}
            >
              {actionLabel}
            </button>
          ) : null}
          {streaming && onStop ? (
            <button
              type="button"
              className="workspace-secondary-action workspace-send-action"
              aria-label="Stop the current turn"
              onClick={onStop}
              disabled={disabled}
            >
              Stop
            </button>
          ) : (
            <button
              type="button"
              className="workspace-primary-action workspace-send-action"
              title="Send · Enter (Shift+Enter for a new line)"
              onClick={sendInput}
              disabled={disabled || !input.trim()}
            >
              Send
            </button>
          )}
        </div>
      </div>
    </div>
  );
});
