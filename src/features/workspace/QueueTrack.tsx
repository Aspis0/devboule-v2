import {
  memo,
  useEffect,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import type { QueuedMessage } from "../../types/ipc";
import "./QueueTrack.css";

/**
 * The queued follow-up rows above the composer: the text on at most two lines,
 * and per row Steer (send now — the daemon interrupts the turn itself), Edit
 * (the row's own text, replaced where it stands), Delete, and reorder — by
 * dragging the row, or Alt+Arrow on a focused row. A send the daemon refused
 * leaves its reason on the row, nothing renders while the queue is empty, and
 * an emptied track under the user's focus hands that focus to the composer.
 *
 * The rows are the daemon's, whole and unchanged: this component renders what
 * the last snapshot said and asks for the changes the user made.
 */

const REORDER_HINT_ID = "workspace-queue-reorder-hint";

/** The drag marker only this component writes and accepts. A `text/plain`
 * drag — a selection dragged from anywhere — is somebody else's drag and
 * must never reorder the queue. */
export const QUEUE_ROW_MIME = "application/x-devboule-queue-row";

interface QueueTrackProps {
  items: readonly QueuedMessage[];
  onSteer: (itemId: string) => void;
  onEdit: (itemId: string, text: string) => void;
  onDelete: (itemId: string) => void;
  onMove: (itemId: string, index: number) => void;
  /** The row taken out was the last one: the composer takes the focus back. */
  onEmptied?: () => void;
}

/** The row is the draggable — never its buttons, never its editor — so the drag
 * carries the row's id without a button starting a drag of its own. */
function rowDragStart(item: QueuedMessage, event: ReactDragEvent<HTMLDivElement>): void {
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData(QUEUE_ROW_MIME, item.itemId);
}

function rowDragOver(event: ReactDragEvent<HTMLDivElement>): void {
  if (event.dataTransfer.types.includes(QUEUE_ROW_MIME)) event.preventDefault();
}

export const QueueTrack = memo(function QueueTrack({
  items,
  onSteer,
  onEdit,
  onDelete,
  onMove,
  onEmptied,
}: QueueTrackProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const previousIdsRef = useRef<string[] | null>(null);
  const heldIdRef = useRef<string | null>(null);
  const emptiedRef = useRef(onEmptied);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  useEffect(() => {
    emptiedRef.current = onEmptied;
  });

  // A row removed under the user's focus hands that focus to its neighbour —
  // the next row, else the previous one — and an emptied track reports, so
  // the composer can take the focus back. The holder is remembered from the
  // row's own focus event: React unmounts the removed row before this effect
  // can ask `document.activeElement` who it was.
  const currentIds = items.map((item) => item.itemId);
  useEffect(() => {
    const previous = previousIdsRef.current;
    previousIdsRef.current = currentIds;
    if (previous === null) return;
    const removedIndex = previous.findIndex((id) => !currentIds.includes(id));
    if (removedIndex === -1) return;
    if (heldIdRef.current !== previous[removedIndex]) return;
    heldIdRef.current = null;
    const rows = containerRef.current?.querySelectorAll<HTMLDivElement>("[data-queue-id]");
    if (rows === undefined || rows.length === 0) {
      emptiedRef.current?.();
      return;
    }
    rows[Math.min(removedIndex, rows.length - 1)].focus();
  }, [currentIds]);

  // A row that stopped being there — sent, dropped, removed on another device —
  // takes its editor with it. Derived, not stored: the row is gone, so there is
  // nothing to reset.
  const editing = editingId !== null && currentIds.includes(editingId) ? editingId : null;

  if (items.length === 0) return null;

  function beginEdit(item: QueuedMessage): void {
    setEditingId(item.itemId);
    setDraft(item.text);
  }

  function saveEdit(itemId: string): void {
    setEditingId(null);
    if (draft.trim() !== "") onEdit(itemId, draft.trim());
  }

  function editKeyDown(event: ReactKeyboardEvent<HTMLInputElement>, itemId: string): void {
    if (event.key === "Enter") {
      event.preventDefault();
      saveEdit(itemId);
    } else if (event.key === "Escape") {
      event.preventDefault();
      setEditingId(null);
    }
  }

  function rowDrop(droppedOn: number, event: ReactDragEvent<HTMLDivElement>): void {
    const id = event.dataTransfer.getData(QUEUE_ROW_MIME);
    if (!id) return;
    event.preventDefault();
    const from = items.findIndex((item) => item.itemId === id);
    // Dropping a row ON another lands it in front of that row, in both
    // directions: the row you released the mouse on ends up just below it.
    // The index is counted in the queue the dragged row has already left, so a
    // downward drop subtracts that step back — and a row dropped on the one
    // directly under it does not move at all.
    onMove(id, from >= 0 && from < droppedOn ? droppedOn - 1 : droppedOn);
  }

  function rowKeyDown(
    itemId: string,
    index: number,
    event: ReactKeyboardEvent<HTMLDivElement>,
  ): void {
    if (!event.altKey) return;
    if (event.key === "ArrowUp") {
      event.preventDefault();
      onMove(itemId, index - 1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      onMove(itemId, index + 1);
    }
  }

  return (
    <>
      {/* The reorder instruction is for the screen reader, not the eye: said
        once, outside the list, and named by every row's `aria-describedby`.
        Inside the list it would be a non-`listitem` child of `role=list`, and
        the whole tutorial would be read out attached to every row. */}
      <span id={REORDER_HINT_ID} className="workspace-queue-hint">
        Press Alt with ArrowUp or ArrowDown to move the focused queued message; a row can also be
        dragged onto another.
      </span>
      <div
        ref={containerRef}
        className="workspace-queue-track"
        role="list"
        aria-label="Queued messages"
        data-testid="queue-track"
      >
        {items.map((item, index) => (
          <div
            key={item.itemId}
            role="listitem"
            className="workspace-queue-row"
            tabIndex={0}
            data-testid="queue-row"
            data-queue-id={item.itemId}
            aria-describedby={REORDER_HINT_ID}
            draggable
            onFocus={() => {
              heldIdRef.current = item.itemId;
            }}
            onBlur={() => {
              if (heldIdRef.current === item.itemId) heldIdRef.current = null;
            }}
            onDragStart={(event) => rowDragStart(item, event)}
            onDragOver={rowDragOver}
            onDrop={(event) => rowDrop(index, event)}
            onKeyDown={(event) => rowKeyDown(item.itemId, index, event)}
          >
            {editing === item.itemId ? (
              <input
                className="workspace-queue-edit"
                aria-label="Edit queued message"
                data-testid="queue-edit-input"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                onKeyDown={(event) => editKeyDown(event, item.itemId)}
              />
            ) : (
              <span className="workspace-queue-text" title={item.text}>
                {item.text}
              </span>
            )}
            {item.error === undefined ? null : (
              <span className="workspace-queue-row-error" role="alert">
                {item.error}
              </span>
            )}
            <span className="workspace-queue-actions">
              {editing === item.itemId ? (
                <>
                  <button
                    type="button"
                    className="workspace-queue-button"
                    aria-label="Save the queued message"
                    data-testid="queue-edit-save"
                    onClick={() => saveEdit(item.itemId)}
                  >
                    ✓
                  </button>
                  <button
                    type="button"
                    className="workspace-queue-button"
                    aria-label="Leave the queued message as it was"
                    data-testid="queue-edit-cancel"
                    onClick={() => setEditingId(null)}
                  >
                    ×
                  </button>
                </>
              ) : (
                <>
                  <button
                    type="button"
                    className="workspace-queue-button"
                    aria-label="Edit queued message"
                    title="Edit queued message"
                    data-testid="queue-edit"
                    onClick={() => beginEdit(item)}
                  >
                    ✎
                  </button>
                  <button
                    type="button"
                    className="workspace-queue-button workspace-queue-steer"
                    aria-label="Send queued message now — interrupts the running turn"
                    title="Send queued message now — interrupts the running turn"
                    data-testid="queue-steer"
                    onClick={() => onSteer(item.itemId)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="workspace-queue-button"
                    aria-label="Delete queued message"
                    title="Delete queued message"
                    data-testid="queue-delete"
                    onClick={() => onDelete(item.itemId)}
                  >
                    ×
                  </button>
                </>
              )}
            </span>
          </div>
        ))}
      </div>
    </>
  );
});
