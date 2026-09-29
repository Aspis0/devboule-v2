import { useEffect, useRef } from "react";
import { useMenuOpen } from "../../lib/menuOpen";
import { scrollRowIntoView } from "../../lib/scrollRowIntoView";

/** A command the daemon published: the name that goes into the text, and the
 * words that sell it. */
export interface WorkspaceCommand {
  name: string;
  description: string;
  hint?: string;
}

/** The command menu draws this line whenever it has no option to render,
 * so it is never an empty box. */
const NO_COMMANDS_TEXT = "No commands found";

interface WorkspaceCommandMenuProps {
  /** The menu is up; the owner owns the open state and says so. */
  open: boolean;
  /** Dismiss the menu — the band opening is the outside press. */
  onClose: () => void;
  /** The listbox's own id: the composer's combobox points aria-controls at it. */
  listId: string;
  /** The ranked matches, best first; the daemon's order for a bare "/". */
  commands: readonly WorkspaceCommand[];
  /** The row the keys are on; -1 while there is no row to be on. */
  activeIndex: number;
  /** The active row's element id: the composer points aria-activedescendant at it. */
  activeOptionId: string | null;
  onSelect: (command: WorkspaceCommand) => void;
}

export function WorkspaceCommandMenu({
  open,
  onClose,
  listId,
  commands,
  activeIndex,
  activeOptionId,
  onSelect,
}: WorkspaceCommandMenuProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const activeRowRef = useRef<HTMLButtonElement>(null);
  useMenuOpen(open, onClose);

  // Scrolling only this list's own box: a bare scrollIntoView would walk
  // every scrollable ancestor — transcript and page included.
  useEffect(() => {
    if (!open) return;
    const row = activeRowRef.current;
    const list = listRef.current;
    if (row === null || list === null) return;
    scrollRowIntoView(list, row);
  }, [activeIndex, commands.length, open]);

  if (!open) return null;

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
