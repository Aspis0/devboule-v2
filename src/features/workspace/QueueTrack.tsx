import { memo, useEffect, useRef, type DragEvent as ReactDragEvent } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import type { QueuedMessage } from "./messageQueue";
import "./QueueTrack.css";

/**
 * The queued follow-up rows above the composer (Paseo's place): the text on
 * at most two lines, and per row Steer (send now — it interrupts the turn),
 * Edit (Paseo's: the row leaves the queue and its content goes back into the
 * composer), Delete, and reorder — by dragging the row, or Alt+Arrow on a
 * focused row. A send the daemon refused leaves its reason on the row.
 * Nothing renders while the queue is empty.
 */

const REORDER_HINT_ID = "workspace-queue-reorder-hint";

/** The drag marker only this component writes and accepts. A `text/plain`
 * drag — a selection dragged from anywhere — is somebody else's drag and
 * must never reorder the queue. */
export const QUEUE_ROW_MIME = "application/x-devboule-queue-row";

interface QueueTrackProps {
  items: readonly QueuedMessage[];
  onSteer: (id: string) => void;
  onEdit: (id: string) => void;
  onDelete: (id: string) => void;
  onMove: (id: string, index: number) => void;
  /** The row taken out was the last one: the composer takes the focus back. */
  onEmptied?: () => void;
}

/** The row is the draggable — never its buttons — so the drag carries the
 * row's id without a button starting a drag of its own. */
function rowDragStart(item: QueuedMessage, event: ReactDragEvent<HTMLDivElement>): void {
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData(QUEUE_ROW_MIME, item.id);
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
  useEffect(() => {
    emptiedRef.current = onEmptied;
  });

  // A row removed under the user's focus hands that focus to its neighbour —
  // the next row, else the previous one — and an emptied track reports, so
  // the composer can take the focus back. The holder is remembered from the
  // row's own focus event: React unmounts the removed row before this effect
  // can ask `document.activeElement` who it was.
  const currentIds = items.map((item) => item.id);
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

  if (items.length === 0) return null;

  function rowDrop(droppedOn: number, event: ReactDragEvent<HTMLDivElement>): void {
    const id = event.dataTransfer.getData(QUEUE_ROW_MIME);
    if (!id) return;
    event.preventDefault();
    const from = items.findIndex((item) => item.id === id);
    // Dropping a row ON another lands it in front of that row, in both
    // directions: the row you released the mouse on ends up just below it.
    // `move` counts the destination in the list the dragged row has already
    // left, so moving down subtracts that step back — and a row dropped on the
    // one directly under it does not move at all (review F15).
    onMove(id, from >= 0 && from < droppedOn ? droppedOn - 1 : droppedOn);
  }

  function rowKeyDown(id: string, index: number, event: ReactKeyboardEvent<HTMLDivElement>): void {
    if (!event.altKey) return;
    if (event.key === "ArrowUp") {
      event.preventDefault();
      onMove(id, index - 1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      onMove(id, index + 1);
    }
  }

  return (
    <>
      {/* The reorder instruction is for the screen reader, not the eye: said
        once, outside the list, and named by every row's `aria-describedby`.
        Inside the list it was a non-`listitem` child of `role=list`, and the
        whole tutorial was read out attached to every row (review F14). */}
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
            key={item.id}
            role="listitem"
            className="workspace-queue-row"
            tabIndex={0}
            data-testid="queue-row"
            data-queue-id={item.id}
            aria-describedby={REORDER_HINT_ID}
            draggable
            onFocus={() => {
              heldIdRef.current = item.id;
            }}
            onBlur={() => {
              if (heldIdRef.current === item.id) heldIdRef.current = null;
            }}
            onDragStart={(event) => rowDragStart(item, event)}
            onDragOver={rowDragOver}
            onDrop={(event) => rowDrop(index, event)}
            onKeyDown={(event) => rowKeyDown(item.id, index, event)}
          >
            <span className="workspace-queue-text" title={item.text}>
              {item.text}
            </span>
            {item.error === undefined ? null : (
              <span className="workspace-queue-row-error" role="alert">
                {item.error}
              </span>
            )}
            <span className="workspace-queue-actions">
              <button
                type="button"
                className="workspace-queue-button"
                aria-label="Edit queued message"
                title="Edit queued message"
                data-testid="queue-edit"
                onClick={() => onEdit(item.id)}
              >
                ✎
              </button>
              <button
                type="button"
                className="workspace-queue-button workspace-queue-steer"
                aria-label="Send queued message now — interrupts the running turn"
                title="Send queued message now — interrupts the running turn"
                data-testid="queue-steer"
                onClick={() => onSteer(item.id)}
              >
                ↑
              </button>
              <button
                type="button"
                className="workspace-queue-button"
                aria-label="Delete queued message"
                title="Delete queued message"
                data-testid="queue-delete"
                onClick={() => onDelete(item.id)}
              >
                ×
              </button>
            </span>
          </div>
        ))}
      </div>
    </>
  );
});
