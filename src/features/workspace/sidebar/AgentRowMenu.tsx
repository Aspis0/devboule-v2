import { useRef, useState, type RefObject } from "react";
import { useConfirmAsk } from "../../../components/ConfirmHost";
import { useMenuOpen } from "../../../lib/menuOpen";
import { AnchoredPopover } from "../popoverPlace";

// Every clause is a checked effect of the close, as on the list's archive ask.
const ARCHIVE_MESSAGE =
  "It leaves this list and its tab closes. Its transcript stays in History; its attached files are removed.";

export interface AgentRowMenuProps {
  sessionId: string;
  title: string;
  anchor: RefObject<HTMLElement | null>;
  /** Archives the agent; answers with the refusal sentence, or null. */
  onArchive: (sessionId: string) => Promise<string | null>;
  onDismiss: () => void;
}

/** The menu a finished agent's row opens: Archive, asked first, and a refusal said in place. */
export function AgentRowMenu({
  sessionId,
  title,
  anchor,
  onArchive,
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
    const sentence = await onArchive(sessionId);
    if (sentence === null) onDismiss();
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
