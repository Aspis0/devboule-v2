// The stateful half of pending session actions: the scheduler that arms
// timers, fires intents, and owns the settled/error snapshots the strip
// subscribes to. Pure policy (mootness, verification, pruning) lives in
// pendingSessionActions.ts beside the types.

import type { PendingSessionAction, SessionInstance } from "./pendingSessionActions";
import { isCommandError, reasonFromCause } from "../../lib/tauri";

const STORAGE_KEY = "devboule.pendingSessionActions.v1";

/** Crash-copy key. Exported so tests can seed a leftover without a crash. */
export const PENDING_STORAGE_KEY = STORAGE_KEY;

export type PendingFire = (action: PendingSessionAction) => Promise<void>;

interface SchedulerOptions {
  now?: () => number;
  storage?: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
}

function isRecord(value: unknown): value is PendingSessionAction {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.id === "string" &&
    typeof record.title === "string" &&
    (record.kind === "archive" || record.kind === "delete") &&
    typeof record.dueAt === "number" &&
    (record.createdAtMs === undefined || typeof record.createdAtMs === "number")
  );
}

function readPersisted(storage: SchedulerOptions["storage"]): PendingSessionAction[] {
  if (!storage) return [];
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isRecord);
  } catch {
    return [];
  }
}

export class PendingSessionScheduler {
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly actions = new Map<string, PendingSessionAction>();
  private readonly listeners = new Set<() => void>();
  private snapshot: PendingSessionAction[] = [];
  // Fired but unconfirmed: the tab stays hidden until the roster confirms.
  // Survives unmount with the timers — this is what keeps a surface switch
  // from resurrecting a tab whose act already left.
  private settled = new Map<string, SessionInstance>();
  private settledSnapshot: ReadonlyMap<string, SessionInstance> = new Map();
  private fireError: string | null = null;

  constructor(
    private fire: PendingFire,
    private readonly options: SchedulerOptions = {},
  ) {}

  /** Rebind the fire after a remount. The timers outlive any one component. */
  setFire(fire: PendingFire): void {
    this.fire = fire;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): PendingSessionAction[] => this.snapshot;

  getSettledSnapshot = (): ReadonlyMap<string, SessionInstance> => this.settledSnapshot;

  getErrorSnapshot = (): string | null => this.fireError;

  /** A fire the human must see even if no undo bar is mounted for it. */
  reportError(message: string | null): void {
    if (this.fireError === message) return;
    this.fireError = message;
    this.notify();
  }

  /** Writes back the roster-pruned settled map (see `pruneDismissed`). */
  replaceSettled(next: ReadonlyMap<string, SessionInstance>): void {
    this.settled = new Map(next);
    this.settledSnapshot = this.settled;
    this.notify();
  }

  pending(): PendingSessionAction[] {
    return this.snapshot;
  }

  has(id: string): boolean {
    return this.actions.has(id);
  }

  /** Records the intent and arms the window. A second swipe of the same tab
   * cannot exist — the tab hides on the first — so a duplicate keeps the
   * first timer instead of arming a second. */
  schedule(action: PendingSessionAction): "scheduled" | "duplicate" {
    if (this.actions.has(action.id)) return "duplicate";
    this.actions.set(action.id, action);
    this.timers.set(
      action.id,
      setTimeout(() => this.settle(action.id), Math.max(0, action.dueAt - this.now())),
    );
    this.publish();
    return "scheduled";
  }

  /** Cancels before expiry: the caller puts the tab back. */
  cancel(id: string): PendingSessionAction | null {
    const action = this.actions.get(id) ?? null;
    if (action === null) return null;
    clearTimeout(this.timers.get(id));
    this.timers.delete(id);
    this.actions.delete(id);
    this.publish();
    return action;
  }

  /** Fires everything now: app close, where a pending delete evaporating is
   * worse than firing without waiting out the window. Best effort — the
   * persisted copy plus the startup check cover a close that wins the race. */
  flushAll(): void {
    const due = [...this.actions.values()];
    for (const action of due) {
      clearTimeout(this.timers.get(action.id));
    }
    this.timers.clear();
    this.actions.clear();
    this.publish();
    for (const action of due) {
      void this.fire(action).catch(() => undefined);
    }
  }

  /** Clears everything without firing: test reset only. A real caller that
   * drops intents without firing them breaks the undo promise. */
  abandonAllForTests(): void {
    for (const id of this.timers.keys()) clearTimeout(this.timers.get(id));
    this.timers.clear();
    this.actions.clear();
    this.settled = new Map();
    this.settledSnapshot = this.settled;
    this.fireError = null;
    this.publish();
  }

  /** Intents left by a close that won the race. Returned unfired: the caller
   * verifies each against the roster before re-arming or dropping it. */
  loadPersisted(): PendingSessionAction[] {
    return readPersisted(this.options.storage ?? null);
  }

  /** Drops the crash copy: verified records re-arm below and repersist on
   * schedule, so keeping it would refire them. Call before re-arming. */
  clearPersisted(): void {
    try {
      this.options.storage?.removeItem(STORAGE_KEY);
    } catch {
      // Same private-mode tolerance as the write path.
    }
  }

  private settle(id: string): void {
    const action = this.actions.get(id) ?? null;
    if (action === null) return;
    this.timers.delete(id);
    this.actions.delete(id);
    // Copy-on-write throughout: the snapshots handed to useSyncExternalStore
    // compare by reference, so an in-place mutation would hide the tab (or
    // show it) without ever re-rendering.
    this.settled = new Map(this.settled).set(id, {
      createdAtMs: action.createdAtMs,
      generation: action.generation,
    });
    this.settledSnapshot = this.settled;
    this.publish();
    void this.fire(action).then(undefined, (cause: unknown) => {
      // Gone by another hand (closed elsewhere, raced exit): the call
      // answers session_not_found, the postcondition holds, and the tab
      // stays hidden with nothing said.
      if (isCommandError(cause) && cause.code === "session_not_found") return;
      const restore = () => {
        const next = new Map(this.settled);
        next.delete(action.id);
        this.replaceSettled(next);
      };
      // Residue of a resume that landed between the last roster read and
      // the fire: the daemon detached the old instance's observers, so the
      // stop names a subscription that is gone
      // (`SessionRuntime::is_observer`, InvalidRequest). The new process is
      // safe — the call killed nothing — but the intent is void and the tab
      // must come back with a sentence the human can act on, not the
      // protocol's. Matched narrowly (code plus the daemon's own words);
      // if the daemon ever renames it this falls through to the generic
      // error below, which still restores the tab.
      if (
        action.kind === "archive" &&
        isCommandError(cause) &&
        cause.code === "invalid_request" &&
        cause.message.includes("not attached")
      ) {
        restore();
        this.reportError(
          `Archive of “${action.title}” didn't go through — the session restarted. Archive it again if you still want to.`,
        );
        return;
      }
      // Anything else restores the tab and reports the reason — including
      // a daemon that is simply unreachable, which must never read as done.
      restore();
      this.reportError(reasonFromCause(cause));
    });
  }

  private now(): number {
    return this.options.now?.() ?? Date.now();
  }

  private publish(): void {
    this.snapshot = [...this.actions.values()];
    try {
      this.options.storage?.setItem(STORAGE_KEY, JSON.stringify(this.snapshot));
    } catch {
      // Private mode and quota failures leave memory as the store.
    }
    this.notify();
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

// The intent belongs to the app's lifetime, not to a component that comes
// and goes with a shell surface: browser timers survive unmount, so a
// per-mount scheduler would either double-fire (old timer plus a re-armed
// copy) or need the remount to tell "live intent" from "crash leftover"
// apart. One scheduler per app lifetime has neither problem.
let sharedScheduler: PendingSessionScheduler | null = null;
let startupRecoveryClaimed = false;

export function sharedPendingScheduler(
  fire: PendingFire,
  storage: SchedulerOptions["storage"] = null,
): PendingSessionScheduler {
  if (!sharedScheduler) {
    sharedScheduler = new PendingSessionScheduler(fire, { storage });
  } else {
    sharedScheduler.setFire(fire);
  }
  return sharedScheduler;
}

/** True exactly once per app lifetime: guards the crash-leftover recovery. */
export function claimStartupRecovery(): boolean {
  if (startupRecoveryClaimed) return false;
  startupRecoveryClaimed = true;
  return true;
}

/** Test seam: drops the shared instance and the recovery claim. */
export function resetSharedPendingSchedulerForTests(): void {
  sharedScheduler?.abandonAllForTests();
  sharedScheduler = null;
  startupRecoveryClaimed = false;
}
