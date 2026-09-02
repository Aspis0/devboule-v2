import { useCallback, useEffect, useState } from "react";
import {
  createSessionStateChannel,
  sessionCreate,
  sessionsList,
  sessionsUnwatch,
  sessionsWatch,
} from "../../lib/tauri";
import type { Session, SessionStateSnapshot } from "../../types/ipc";

export interface WorkspaceSessionSource {
  list: () => Promise<Session[]>;
  create: () => Promise<Session>;
  watch?: (listener: (snapshots: SessionStateSnapshot[]) => void) => Promise<() => void>;
}

export interface WorkspaceSessionState {
  sessions: Session[];
  selectedSessionId: string | null;
  loading: boolean;
  creating: boolean;
  error: string | null;
}

export interface WorkspaceSessionController {
  getState: () => WorkspaceSessionState;
  subscribe: (listener: () => void) => () => void;
  refresh: () => Promise<void>;
  create: () => Promise<Session | null>;
  select: (sessionId: string) => void;
  watch: () => () => void;
}

const DEFAULT_SOURCE: WorkspaceSessionSource = {
  list: sessionsList,
  create: () => sessionCreate(null, "terminal"),
  watch: async (listener) => {
    const channel = createSessionStateChannel(listener);
    await sessionsWatch(channel);
    return () => {
      void sessionsUnwatch();
    };
  },
};

const LIST_ERROR = "Could not load terminal sessions. The daemon is unreachable.";
const CREATE_ERROR = "Could not create a terminal session. The daemon is unreachable.";

export function terminalSessions(sessions: readonly Session[]): Session[] {
  return sessions.filter((session) => session.kind === "terminal");
}

function formatElapsed(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const seconds = Math.floor(elapsedMs / 1_000);
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

export function sessionStateLabel(state: unknown, elapsedMs?: number | null): string {
  if (typeof state !== "object" || state === null || !("type" in state)) return "unknown";
  const type = state.type;
  if (type === "silent") {
    return typeof elapsedMs === "number"
      ? `silent · ${formatElapsed(elapsedMs)}`
      : "silent · duration unknown";
  }
  return type === "live" || type === "ended" || type === "recovered" ? type : "unknown";
}

export function sessionDotTone(state: unknown): "green" | "terracotta" | "border" {
  const label = sessionStateLabel(state);
  if (label === "live") return "green";
  if (label.startsWith("silent")) return "border";
  if (label === "recovered") return "border";
  return "terracotta";
}

export function sessionTitle(session: Pick<Session, "id" | "title">): string {
  const title = session.title.trim();
  return title || `Terminal ${session.id.slice(0, 8)}`;
}

export function createWorkspaceSessionController(
  source: WorkspaceSessionSource = DEFAULT_SOURCE,
): WorkspaceSessionController {
  let state: WorkspaceSessionState = {
    sessions: [],
    selectedSessionId: null,
    loading: true,
    creating: false,
    error: null,
  };
  let refreshGeneration = 0;
  const listeners = new Set<() => void>();
  let watchLeases = 0;
  let watchPromise: Promise<() => void> | null = null;
  let watchStop: (() => void) | null = null;

  const publish = (next: WorkspaceSessionState): void => {
    state = next;
    for (const listener of listeners) listener();
  };

  const refresh = async (): Promise<void> => {
    const generation = ++refreshGeneration;
    publish({ ...state, loading: true, error: null });
    try {
      const listed = terminalSessions(await source.list());
      if (generation !== refreshGeneration) return;
      const selected =
        state.selectedSessionId !== null &&
        listed.some((session) => session.id === state.selectedSessionId)
          ? state.selectedSessionId
          : (listed[0]?.id ?? null);
      publish({
        ...state,
        sessions: listed,
        selectedSessionId: selected,
        loading: false,
        error: null,
      });
    } catch {
      if (generation !== refreshGeneration) return;
      publish({ ...state, loading: false, error: LIST_ERROR });
    }
  };

  const applySnapshot = (snapshots: SessionStateSnapshot[]): void => {
    // A pushed roster is authoritative. Cancel an older list response so a
    // slow initial request cannot put the tab strip back behind the daemon.
    ++refreshGeneration;
    const known = new Map(state.sessions.map((session) => [session.id, session]));
    const sessions = snapshots.map((snapshot): Session => {
      const previous = known.get(snapshot.id);
      return previous
        ? {
            ...previous,
            title: snapshot.title,
            state: snapshot.state,
            elapsedMs: snapshot.elapsedMs,
          }
        : {
            id: snapshot.id,
            workspaceId: null,
            kind: "terminal",
            title: snapshot.title,
            state: snapshot.state,
            elapsedMs: snapshot.elapsedMs,
          };
    });
    const selected =
      state.selectedSessionId !== null &&
      sessions.some((session) => session.id === state.selectedSessionId)
        ? state.selectedSessionId
        : (sessions[0]?.id ?? null);
    publish({
      ...state,
      sessions,
      selectedSessionId: selected,
      loading: false,
      error: null,
    });
  };

  const create = async (): Promise<Session | null> => {
    if (state.creating) return null;
    ++refreshGeneration;
    publish({ ...state, creating: true, error: null });
    try {
      const session = await source.create();
      if (session.kind !== "terminal") {
        publish({ ...state, creating: false, error: CREATE_ERROR });
        return null;
      }
      const sessions = [...state.sessions.filter((current) => current.id !== session.id), session];
      publish({
        ...state,
        sessions,
        selectedSessionId: session.id,
        creating: false,
        error: null,
      });
      return session;
    } catch {
      publish({ ...state, creating: false, error: CREATE_ERROR });
      return null;
    }
  };

  const watch = (): (() => void) => {
    watchLeases += 1;
    let released = false;
    if (source.watch && watchPromise === null) {
      watchPromise = source
        .watch(applySnapshot)
        .then((stop) => {
          watchStop = stop;
          if (watchLeases === 0) {
            stop();
            watchStop = null;
            watchPromise = null;
          }
          return stop;
        })
        .catch(() => {
          watchPromise = null;
          if (watchLeases > 0) {
            publish({ ...state, error: LIST_ERROR });
          }
          return () => undefined;
        });
    }
    return () => {
      if (released) return;
      released = true;
      watchLeases = Math.max(0, watchLeases - 1);
      if (watchLeases === 0 && watchStop !== null) {
        watchStop();
        watchStop = null;
        watchPromise = null;
      }
    };
  };

  return {
    getState: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    refresh,
    create,
    watch,
    select: (sessionId) => {
      if (state.sessions.some((session) => session.id === sessionId)) {
        publish({ ...state, selectedSessionId: sessionId });
      }
    },
  };
}

export function useWorkspaceSessions(): WorkspaceSessionState & {
  refresh: () => Promise<void>;
  create: () => Promise<Session | null>;
  select: (sessionId: string) => void;
} {
  const [controller] = useState<WorkspaceSessionController>(() =>
    createWorkspaceSessionController(),
  );
  const [state, setState] = useState(controller.getState);

  useEffect(() => {
    const unsubscribe = controller.subscribe(() => setState(controller.getState()));
    const releaseWatch = controller.watch();
    void controller.refresh();
    return () => {
      releaseWatch();
      unsubscribe();
    };
  }, [controller]);

  const refresh = useCallback(() => controller.refresh(), [controller]);
  const create = useCallback(() => controller.create(), [controller]);
  const select = useCallback((sessionId: string) => controller.select(sessionId), [controller]);

  return { ...state, refresh, create, select };
}
