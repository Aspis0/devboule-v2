import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { composerActionLabel } from "../../lib/sendBehavior";

export interface WorkspaceCommand {
  name: string;
  description: string;
  hint?: string;
}

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
  const textareaRef = useRef<HTMLTextAreaElement>(null);

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

  const handleComposerKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
      // Enter belongs to an IME while a composition is open: the keystroke
      // commits the composition, and sending here would submit the text before
      // the candidate is chosen. `isComposing` is the standard signal; the
      // legacy `keyCode === 229` covers engines that report the composition
      // commit without setting it.
      if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
      if (event.key !== "Enter" || event.shiftKey) return;
      event.preventDefault();
      if (event.ctrlKey || event.metaKey) runAlternateAction();
      else runDefaultAction();
    },
    [runAlternateAction, runDefaultAction],
  );

  const query = commandQuery(input);
  const commandMatches =
    query === null
      ? []
      : availableCommands.filter((command) => command.name.toLowerCase().includes(query));
  const commandMenuVisible = !disabled && query !== null && availableCommands.length > 0;
  // Paseo's submit-button words on the button that does what Enter does.
  const actionLabel = composerActionLabel(defaultActionQueues);

  const selectCommand = useCallback(
    (command: WorkspaceCommand) => {
      setInput(`/${command.name} `);
      textareaRef.current?.focus();
    },
    [setInput],
  );

  return (
    <div className="workspace-composer-wrap">
      {queuedTrack}
      {commandMenuVisible ? (
        <div className="workspace-command-menu" role="listbox" aria-label="Available commands">
          <div className="workspace-command-menu-heading">Agent commands</div>
          {commandMatches.length > 0 ? (
            commandMatches.map((command) => (
              <button
                type="button"
                role="option"
                className="workspace-command-option"
                key={command.name}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => selectCommand(command)}
              >
                <span className="workspace-command-name">/{command.name}</span>
                <span className="workspace-command-description">
                  {command.description}
                  {command.hint ? ` · ${command.hint}` : ""}
                </span>
              </button>
            ))
          ) : (
            <div className="workspace-command-empty">No matching commands.</div>
          )}
        </div>
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
