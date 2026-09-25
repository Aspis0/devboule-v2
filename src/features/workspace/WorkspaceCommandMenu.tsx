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
  /** The listbox's own id: the composer's combobox points aria-controls at it. */
  listId: string;
  /** The filter's matches, in the order the daemon gave them. */
  commands: readonly WorkspaceCommand[];
  /** The row the keys are on; -1 while there is no row to be on. */
  activeIndex: number;
  /** The active row's element id: the composer points aria-activedescendant at it. */
  activeOptionId: string | null;
  onSelect: (command: WorkspaceCommand) => void;
}

export function WorkspaceCommandMenu({
  listId,
  commands,
  activeIndex,
  activeOptionId,
  onSelect,
}: WorkspaceCommandMenuProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const activeRowRef = useRef<HTMLButtonElement>(null);

  // Paseo works this offset out inside its own list; a bare scrollIntoView
  // would walk every scrollable ancestor — transcript and page included.
  useEffect(() => {
    const row = activeRowRef.current;
    const list = listRef.current;
    if (row === null || list === null) return;
    const rowTop = row.offsetTop;
    const rowBottom = rowTop + row.offsetHeight;
    const viewTop = list.scrollTop;
    const viewBottom = viewTop + list.clientHeight;
    if (rowTop < viewTop) list.scrollTop = rowTop;
    else if (rowBottom > viewBottom) list.scrollTop = rowBottom - viewBottom;
  }, [activeIndex, commands.length]);

  return (
    <div
      className="workspace-command-menu"
      id={listId}
      ref={listRef}
      role="listbox"
      aria-label="Available commands"
    >
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
