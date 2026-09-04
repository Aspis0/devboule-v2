import { memo, useCallback, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

export interface WorkspaceCommand {
  name: string;
  description: string;
  hint?: string;
}

interface WorkspaceComposerProps {
  streaming: boolean;
  disabled?: boolean;
  disabledReason?: string;
  availableCommands?: readonly WorkspaceCommand[];
  onSend: (text: string) => void;
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
  disabled = false,
  disabledReason = "This session is no longer available.",
  availableCommands = [],
  onSend,
}: WorkspaceComposerProps) {
  const [input, setInput] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const sendInput = useCallback(() => {
    const text = input.trim();
    if (!text || disabled) return;
    onSend(text);
    setInput("");
  }, [disabled, input, onSend, setInput]);

  const handleComposerKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key !== "Enter" || event.shiftKey) return;
      event.preventDefault();
      sendInput();
    },
    [sendInput],
  );

  const query = commandQuery(input);
  const commandMatches =
    query === null
      ? []
      : availableCommands.filter((command) => command.name.toLowerCase().includes(query));
  const commandMenuVisible = !disabled && query !== null && availableCommands.length > 0;

  const selectCommand = useCallback(
    (command: WorkspaceCommand) => {
      setInput(`/${command.name} `);
      textareaRef.current?.focus();
    },
    [setInput],
  );

  return (
    <div className="workspace-composer-wrap">
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
          ref={textareaRef}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleComposerKeyDown}
          placeholder={streaming ? "Steer the running turn…" : "Message the agent…"}
          rows={3}
          aria-label="Message the agent"
          disabled={disabled}
        />
        <div className="workspace-composer-footer">
          <span className="workspace-composer-hint">
            {disabled
              ? disabledReason
              : availableCommands.length > 0
                ? "Type / for commands · Enter to send · Shift+Enter for a new line"
                : "Enter to send · Shift+Enter for a new line"}
          </span>
          <button
            type="button"
            className="workspace-primary-action workspace-send-action"
            onClick={sendInput}
            disabled={disabled || !input.trim()}
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
});
