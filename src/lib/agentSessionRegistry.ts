import { AgentSession, type AgentSessionDeps } from "./agentSession";
import { taskTransitions } from "./backgroundTasks";
import {
  lastSeenTaskState,
  rememberTaskState,
  resetTaskStateMemoryForTests,
} from "../features/workspace/taskStateMemory";
import type { PermissionRequest, PermissionResolved } from "../types/ipc";
import {
  EMPTY_QUEUE_GATE,
  nextQueueGate,
  type QueueSnapshotEvent,
} from "../features/workspace/queueSnapshot";

export interface RegistryEntry {
  readonly session: AgentSession;
  readonly generation: number | null;
  mounts: number;
  lastViewedAt: number;
  tasksNews: boolean;
  queueSnapshot: QueueSnapshotEvent | null;
}

export interface PendingPermission {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
}

interface PermissionObserver {
  onRequest: (pending: PendingPermission) => void;
  onResolved: (sessionId: string, resolution: PermissionResolved) => void;
}

interface Entry extends RegistryEntry {
  callbacks: AgentSessionDeps | null;
  pendingPermissions: Map<string, PendingPermission>;
  tasksVisible: boolean;
  liveGoal: string | null | undefined;
  listeners: Set<() => void>;
  unsubscribe: () => void;
}

const entries = new Map<string, Entry>();
const bySession = new WeakMap<AgentSession, Entry>();
const permissionObservers = new Set<PermissionObserver>();
const CAP = 8;
let sweepScheduled = false;
let viewOrder = 0;

function notify(entry: Entry): void {
  for (const listener of entry.listeners) listener();
}

function remove(id: string, entry: Entry): void {
  entries.delete(id);
  bySession.delete(entry.session);
  entry.unsubscribe();
  entry.callbacks = null;
  entry.session.dispose();
}

function scheduleSweep(): void {
  if (sweepScheduled) return;
  sweepScheduled = true;
  queueMicrotask(() => {
    sweepScheduled = false;
    sweep();
  });
}

function stateIdle(session: AgentSession): boolean {
  const state = session.getState();
  return state.status !== "initializing" && state.status !== "running" && !state.streaming;
}

function sweep(): void {
  if (entries.size <= CAP) return;
  const idle = [...entries]
    .filter(([, entry]) => {
      return (
        entry.mounts === 0 &&
        stateIdle(entry.session) &&
        !entry.tasksNews &&
        entry.pendingPermissions.size === 0 &&
        (entry.queueSnapshot?.items.length ?? 0) === 0
      );
    })
    .sort((a, b) => a[1].lastViewedAt - b[1].lastViewedAt);
  for (const [id, entry] of idle) {
    if (entries.size <= CAP) break;
    remove(id, entry);
  }
}

export function acquire(deps: AgentSessionDeps, generation: number | null): RegistryEntry {
  let entry = entries.get(deps.sessionId);
  const liveGoal = entry?.liveGoal;
  const initialGoal = liveGoal !== undefined ? liveGoal : deps.initialGoal;
  if (
    entry &&
    (entry.generation !== generation ||
      entry.session.isDisposed() ||
      (entry.session.getState().status === "error" && entry.session.getSubscriptionId() === null))
  ) {
    remove(deps.sessionId, entry);
    entry = undefined;
  }
  if (entry) {
    entry.callbacks = deps;
    entry.mounts++;
    entry.lastViewedAt = ++viewOrder;
    sweep();
    return entry;
  }
  const sessionId = deps.sessionId;
  const created: Entry = {
    session: new AgentSession({
      sessionId: deps.sessionId,
      invoke: deps.invoke,
      createChannel: deps.createChannel,
      initialGoal,
      // Null while no view is mounted: an unknown epoch is a baseline, never news.
      daemonEpoch: () => created.callbacks?.daemonEpoch?.() ?? null,
      onTurnStarted: () => created.callbacks?.onTurnStarted?.(),
      onTurnFinished: () => {
        if (created.mounts > 0) created.callbacks?.onTurnFinished?.();
      },
      onGoalChanged: (goal) => {
        created.liveGoal = goal;
        if (created.mounts > 0) created.callbacks?.onGoalChanged?.(goal);
      },
      onQueueSnapshot: (snapshot) => {
        if (nextQueueGate(created.queueSnapshot ?? EMPTY_QUEUE_GATE, snapshot) === null) return;
        const hadQueue = (created.queueSnapshot?.items.length ?? 0) > 0;
        created.queueSnapshot = snapshot;
        if (created.mounts > 0) created.callbacks?.onQueueSnapshot?.(snapshot);
        if (hadQueue && snapshot.items.length === 0) sweep();
      },
      onPermissionRequest: (request, subscriptionId) => {
        const pending = { sessionId, subscriptionId, request };
        created.pendingPermissions.set(request.toolCallId, pending);
        for (const observer of permissionObservers) observer.onRequest(pending);
        created.callbacks?.onPermissionRequest?.(request, subscriptionId);
      },
      onPermissionResolved: (resolution) => {
        const removed = created.pendingPermissions.delete(resolution.toolCallId);
        for (const observer of permissionObservers) observer.onResolved(sessionId, resolution);
        created.callbacks?.onPermissionResolved?.(resolution);
        if (removed) sweep();
      },
    }),
    generation,
    mounts: 1,
    lastViewedAt: ++viewOrder,
    tasksNews: false,
    queueSnapshot: null,
    callbacks: deps,
    pendingPermissions: new Map(),
    tasksVisible: false,
    liveGoal,
    listeners: new Set(),
    unsubscribe: () => {},
  };
  entries.set(deps.sessionId, created);
  bySession.set(created.session, created);
  // Seeded from the remembered list: a finish while no controller existed is still news.
  let previous = lastSeenTaskState(sessionId);
  const unsubscribeTasks = created.session.subscribeTasks(() => {
    const next = created.session.getTaskState();
    if (next === null) return;
    rememberTaskState(sessionId, next);
    const settled = taskTransitions(previous, next).some(
      (task) => task.state === "finished" || task.state === "failed",
    );
    previous = next;
    if (settled && !created.tasksVisible && !created.tasksNews) {
      created.tasksNews = true;
      notify(created);
    }
  });
  let wasIdle = stateIdle(created.session);
  const unsubscribeState = created.session.subscribe(() => {
    const idle = stateIdle(created.session);
    const becameIdle = idle && !wasIdle;
    wasIdle = idle;
    if (becameIdle) sweep();
  });
  created.unsubscribe = () => {
    unsubscribeTasks();
    unsubscribeState();
  };
  void created.session.start();
  sweep();
  return created;
}

export function release(entry: RegistryEntry): void {
  entry.mounts = Math.max(0, entry.mounts - 1);
  if (entry.mounts === 0) {
    const retained = bySession.get(entry.session);
    if (retained) {
      retained.tasksVisible = false;
      retained.callbacks = null;
      scheduleSweep();
    }
  }
  // Defer eviction until StrictMode has synchronously reacquired its entry.
}

export function syncOpenTabs(ids: Iterable<string>): void {
  const open = new Set(ids);
  for (const [id, entry] of entries) {
    if (!open.has(id)) remove(id, entry);
  }
  sweep();
}

export function peek(sessionId: string, generation: number | null): RegistryEntry | undefined {
  const entry = entries.get(sessionId);
  return entry?.generation === generation && !entry.session.isDisposed() ? entry : undefined;
}

export function registeredSession(source: unknown): boolean {
  return source instanceof AgentSession && bySession.has(source);
}

export function tasksNews(source: unknown): boolean {
  return source instanceof AgentSession ? (bySession.get(source)?.tasksNews ?? false) : false;
}

export function subscribeTasksNews(source: unknown, listener: () => void): () => void {
  const entry = source instanceof AgentSession ? bySession.get(source) : undefined;
  entry?.listeners.add(listener);
  return () => {
    entry?.listeners.delete(listener);
  };
}

export function setTasksVisible(source: unknown, visible: boolean): void {
  const entry = source instanceof AgentSession ? bySession.get(source) : undefined;
  if (!entry) return;
  entry.tasksVisible = visible;
  if (visible && entry.tasksNews) {
    entry.tasksNews = false;
    notify(entry);
    sweep();
  }
}

export function pendingPermissionRequests(): PendingPermission[] {
  return [...entries.values()].flatMap((entry) => [...entry.pendingPermissions.values()]);
}

// A cleared or stale card may never get its permission_resolved; keeping the
// record would re-seed a dead card on remount and pin the entry against eviction.
export function dropPermissionRequest(sessionId: string, toolCallId: string): void {
  if (entries.get(sessionId)?.pendingPermissions.delete(toolCallId)) sweep();
}

export function subscribePermissions(
  onRequest: PermissionObserver["onRequest"],
  onResolved: PermissionObserver["onResolved"],
): () => void {
  const observer = { onRequest, onResolved };
  permissionObservers.add(observer);
  return () => {
    permissionObservers.delete(observer);
  };
}

export function resetRegistry(): void {
  for (const [id, entry] of entries) remove(id, entry);
  viewOrder = 0;
  permissionObservers.clear();
  resetTaskStateMemoryForTests();
}
