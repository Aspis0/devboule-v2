import { useEffect, useRef } from "react";

/** A command the daemon published: the name that goes into the text, and the
 * words that sell it. */
export interface WorkspaceCommand {
  name: string;
  description: string;
  hint?: string;
}

/** Paseo's `agentAutocomplete.noCommands`: its list renders this line whenever
 * it has no option to draw, so the menu is never an empty box. */
const NO_COMMANDS_TEXT = "No commands found";

interface WorkspaceCommandMenuProps {
  /** The filter's matches, in the order the daemon gave them. */
  commands: readonly WorkspaceCommand[];
  /** The row the keys are on; -1 while there is no row to be on. */
  activeIndex: number;
  /** The active row's element id: the composer points aria-activedescendant at it. */
  activeOptionId: string | null;
  onSelect: (command: WorkspaceCommand) => void;
}

export function WorkspaceCommandMenu({
  commands,
  activeIndex,
  activeOptionId,
  onSelect,
}: WorkspaceCommandMenuProps) {
  const activeRowRef = useRef<HTMLButtonElement>(null);

  // Paseo keeps the highlighted row in view with its own offset math; here one
  // nearest-block call follows the keys, and the list too when it changes.
  useEffect(() => {
    activeRowRef.current?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, commands.length]);

  return (
    <div className="workspace-command-menu" role="listbox" aria-label="Available commands">
      <div className="workspace-command-menu-heading">Agent commands</div>
      {commands.length > 0 ? (
        commands.map((command, index) => {
          const active = index === activeIndex;
          return (
            <button
              type="button"
              role="option"
              className="workspace-command-option"
              key={command.name}
              id={active ? (activeOptionId ?? undefined) : undefined}
              aria-selected={active}
              ref={active ? activeRowRef : undefined}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onSelect(command)}
            >
              <span className="workspace-command-name">/{command.name}</span>
              <span className="workspace-command-description">
                {command.description}
                {command.hint ? ` · ${command.hint}` : ""}
              </span>
            </button>
          );
        })
      ) : (
        <div className="workspace-command-empty">{NO_COMMANDS_TEXT}</div>
      )}
    </div>
  );
}
