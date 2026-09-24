// Why: closing fires at once (the owner's decision — the undo window is
// gone). This store hides a fired row until the roster confirms it, owns
// each failure by the ACT that produced it — keyed by id AND generation, so
// a late result for an older generation can never unhide, fail or clear a
// newer close — and reports a target that went stale between the ask and
// the click.

import type { Session } from "../../types/ipc";
import { isCommandError, reasonFromCause } from "../../lib/tauri";
import type { CloseIntent } from "./closePolicy";

/** The key an OLDER build persisted its undo-window intents under. The new
 * build never reads it: startup drops it unread, so a record no living UI
 * confirmed can never fire. */
export const OLDER_BUILD_PENDING_KEY = "devboule.pendingSessionActions.v1";

export interface CloseFailure {
  readonly id: string;
  readonly message: string;
}

/** The row a close act was resolved against: the act names this instance. */
export interface CloseTarget {
  readonly id: string;
  readonly title: string;
  readonly generation: number;
}

interface CloseActionsOptions {
  archive: (id: string) => Promise<void>;
  destroy: (id: string) => Promise<void>;
}

export class CloseActionStore {
  private readonly listeners = new Set<() => void>();
  // Fired and awaiting the roster: id → the generation the act named. The
  // row stays hidden while the mark stands; `pruneConfirmed` drops it.
  private closing = new Map<string, number>();
  private closingSnapshot: readonly string[] = [];
  // id → the failed act's generation and sentence: one line per session,
  // replaceable only by an act of the SAME session, clearable only by an
  // act of the SAME generation.
  private failures = new Map<string, { generation: number; message: string }>();
  private failuresSnapshot: ReadonlyArray<CloseFailure> = [];

  constructor(private readonly acts: CloseActionsOptions) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getClosingSnapshot = (): readonly string[] => this.closingSnapshot;

  getFailuresSnapshot = (): ReadonlyArray<CloseFailure> => this.failuresSnapshot;

  /** Fires the act now. The row hides immediately; a failure brings it back
   * with the reason, named. A second act for a row already leaving is
   * ignored — the row is hidden, so no control can reach it. */
  act(kind: CloseIntent, target: CloseTarget, onFailed?: () => void): void {
    if (this.closing.has(target.id)) return;
    this.markClosing(target.id, target.generation);
    const run = kind === "archive" ? this.acts.archive : this.acts.destroy;
    void run(target.id).then(
      () => this.clearFailure(target.id, target.generation),
      (cause: unknown) => {
        // A late result for an older generation settles nothing: the mark
        // and the visible row it would name belong to a newer act now.
        if (this.closing.get(target.id) !== target.generation) return;
        // Already gone by another hand: the postcondition holds, the row
        // stays hidden for the roster to confirm, and this act's earlier
        // failure is cleared — it did not go through, but it is moot now.
        if (isCommandError(cause) && cause.code === "session_not_found") {
          this.clearFailure(target.id, target.generation);
          return;
        }
        this.unmark(target.id);
        const verb = kind === "archive" ? "Archive" : "Delete";
        this.recordFailure(
          target.id,
          target.generation,
          `${verb} of “${target.title}” failed: ${reasonFromCause(cause)}`,
        );
        onFailed?.();
      },
    );
  }

  /** A target that vanished or changed generation between the ask and the
   * click: never acted on, and said so where failures are read. */
  skipped(kind: CloseIntent, target: CloseTarget): void {
    this.unmark(target.id);
    const verb = kind === "archive" ? "Archive" : "Delete";
    this.recordFailure(
      target.id,
      target.generation,
      `${verb} of “${target.title}” skipped — the session changed after it was confirmed.`,
    );
  }

  /** Drops marks the roster has answered: the row is gone, or came back
   * under a new generation, or A PROCESS IS RUNNING AGAIN — none of those is
   * this act's to keep hiding. A same-generation `ended` row is the close's
   * outcome, and a same-generation `recovered` row is untouched by the act:
   * both stay hidden until something really changes them. */
  pruneConfirmed(sessions: readonly Session[]): void {
    for (const [id, generation] of this.closing) {
      const row = sessions.find((session) => session.id === id);
      if (row === undefined || row.state.generation !== generation) {
        this.unmark(id);
        continue;
      }
      if (row.state.type === "live" || row.state.type === "silent") {
        this.unmark(id);
      }
    }
  }

  clearFailures(): void {
    if (this.failures.size === 0) return;
    this.failures = new Map();
    this.failuresSnapshot = [];
    this.notify();
  }

  private markClosing(id: string, generation: number): void {
    this.closing = new Map(this.closing).set(id, generation);
    this.closingSnapshot = [...this.closing.keys()];
    this.notify();
  }

  private unmark(id: string): void {
    if (!this.closing.has(id)) return;
    const next = new Map(this.closing);
    next.delete(id);
    this.closing = next;
    this.closingSnapshot = [...this.closing.keys()];
    this.notify();
  }

  private recordFailure(id: string, generation: number, message: string): void {
    this.failures = new Map(this.failures).set(id, { generation, message });
    this.publishFailures();
  }

  private clearFailure(id: string, generation: number): void {
    const recorded = this.failures.get(id);
    if (recorded === undefined || recorded.generation !== generation) return;
    this.failures = new Map(this.failures);
    this.failures.delete(id);
    this.publishFailures();
  }

  private publishFailures(): void {
    this.failuresSnapshot = [...this.failures.entries()].map(([failureId, failure]) => ({
      id: failureId,
      message: failure.message,
    }));
    this.notify();
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

// The acts outlive a single mount (a fire can still be in the air when the
// user switches to Settings and back), so the store — like the marks and
// failures it holds — is the app's, not the surface's.
let sharedStore: CloseActionStore | null = null;

export function sharedCloseActions(acts: CloseActionsOptions): CloseActionStore {
  if (sharedStore === null) sharedStore = new CloseActionStore(acts);
  return sharedStore;
}

/** Startup: an older build's undo-window records are dropped unread and the
 * key removed, so nothing a living UI never confirmed can fire. The storage
 * ACCESS is inside the try: a restricted WebView may throw on the getter
 * itself, and a startup nicety must not take the Workspace down. */
export function discardPersistedPendingCloses(
  getStorage: () => Pick<Storage, "getItem" | "removeItem"> | null,
): void {
  try {
    getStorage()?.removeItem(OLDER_BUILD_PENDING_KEY);
  } catch {
    // Private-mode tolerance: nothing persisted, nothing to drop.
  }
}

/** Test seam: drops the shared instance. */
export function resetSharedCloseActionsForTests(): void {
  sharedStore = null;
}
