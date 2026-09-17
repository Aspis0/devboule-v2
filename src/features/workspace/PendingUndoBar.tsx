// The undo window made visible: one bar per scheduled action, in future
// tense — the daemon call has not fired yet — naming what will happen and
// offering the cancel. Delete reads heavier than archive, as it must.

import { useState } from "react";
import type { PendingSessionAction } from "./pendingSessionActions";

interface PendingUndoBarProps {
  pending: PendingSessionAction;
  onUndo: (id: string) => void;
}

export function PendingUndoBar({ pending, onUndo }: PendingUndoBarProps) {
  const deleting = pending.kind === "delete";
  // Remaining time fixed at mount, not a ticking clock: the bar states the
  // window the scheduler enforces, and a remount after a surface switch
  // shows what is left rather than a stale full window.
  const [seconds] = useState(() => Math.max(1, Math.ceil((pending.dueAt - Date.now()) / 1000)));
  return (
    <div className={`pending-undo-bar${deleting ? " pending-undo-bar-delete" : ""}`} role="status">
      <span className="pending-undo-text">
        {deleting ? (
          <>
            &ldquo;{pending.title}&rdquo; will be deleted in {seconds} seconds — the session is
            destroyed and its process stops.
          </>
        ) : (
          <>
            &ldquo;{pending.title}&rdquo; will be archived in {seconds} seconds — the process stops,
            every message stays.
          </>
        )}
      </span>
      <button
        type="button"
        className="pending-undo-action"
        onClick={() => onUndo(pending.id)}
        aria-label={
          deleting ? `Undo delete of ${pending.title}` : `Undo archive of ${pending.title}`
        }
      >
        Undo
      </button>
    </div>
  );
}
