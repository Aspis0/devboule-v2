import { useRef, useState, type RefObject } from "react";
import { useConfirmAsk } from "../../../components/ConfirmHost";
import { useMenuOpen } from "../../../lib/menuOpen";
import { AnchoredPopover } from "../popoverPlace";
import type { ArchiveTarget } from "../subagentArchive";

// Every clause is a checked effect of the close, as on the list's archive ask.
const ARCHIVE_MESSAGE =
  "It leaves this list and its tab closes. Its transcript stays in History; its attached files are removed.";

export interface AgentRowMenuProps {
  /** The agent as the row showed it when the menu opened. */
  target: ArchiveTarget;
  title: string;
  anchor: RefObject<HTMLElement | null>;
  /** Archives the agent; answers with the refusal sentence, or null. */
  onArchive: (target: ArchiveTarget) => Promise<string | null>;
  /** Runs once the archive went through; the row may be gone by then. */
  onArchived: () => void;
  onDismiss: () => void;
}

/** The menu a finished agent's row opens: Archive, asked first, and a refusal said in place. */
export function AgentRowMenu({
  target,
  title,
  anchor,
  onArchive,
  onArchived,
  onDismiss,
}: AgentRowMenuProps) {
  const askConfirm = useConfirmAsk();
  const containerRef = useRef<HTMLDivElement>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  // Mounted only while it is up, so the shell may dismiss it at any time.
  useMenuOpen(true, onDismiss);

  const archive = async () => {
    const confirmed = await askConfirm({
      title: `Archive ${title}?`,
      message: ARCHIVE_MESSAGE,
      confirmLabel: "Archive",
      cancelLabel: "Keep it",
    });
    if (!confirmed) return;
    const sentence = await onArchive(target);
    if (sentence === null) onArchived();
    else setRefusal(sentence);
  };

  return (
    <AnchoredPopover
      anchorRef={anchor}
      containerRef={containerRef}
      onDismiss={onDismiss}
      className="agent-row-menu"
    >
      <div role="menu">
        <button
          type="button"
          role="menuitem"
          className="agent-row-menu-item"
          onClick={() => void archive()}
        >
          Archive
        </button>
        {refusal === null ? null : (
          <p role="alert" className="agent-row-refusal">
            {refusal}
          </p>
        )}
      </div>
    </AnchoredPopover>
  );
}
