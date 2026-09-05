import { useCallback, useEffect, useState } from "react";
import {
  createSessionStateChannel,
  sessionCreate,
  sessionsList,
  sessionsUnwatch,
  sessionsWatch,
} from "../../lib/tauri";
import type { ProviderInfo, Session, SessionKind, SessionStateSnapshot } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";

export interface WorkspaceSessionSource {
  list: () => Promise<Session[]>;
  create: (kind?: SessionKind, provider?: string | null) => Promise<Session>;
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
  create: (kind?: SessionKind, provider?: string | null) => Promise<Session | null>;
  select: (sessionId: string) => void;
  open: (session: Session) => void;
  watch: () => () => void;
}

const DEFAULT_SOURCE: WorkspaceSessionSource = {
  list: sessionsList,
  create: (kind = "acp", provider = null) =>
    provider == null ? sessionCreate(null, kind) : sessionCreate(null, kind, provider),
  watch: async (listener) => {
    const channel = createSessionStateChannel(listener);
    await sessionsWatch(channel);
    return () => {
      void sessionsUnwatch();
    };
  },
};

const LIST_ERROR = "Could not load sessions. The daemon is unreachable.";
const CREATE_ERROR = "Could not create an agent session. The daemon is unreachable.";

export function workspaceSessions(sessions: readonly Session[]): Session[] {
  return [...sessions];
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
  if (type === "live") return "live";
  if (type === "ended" || type === "recovered") {
    if ("integrity" in state && typeof state.integrity === "object" && state.integrity !== null) {
      const integrity = state.integrity;
      if (
        "kind" in integrity &&
        (integrity.kind === "truncated" || integrity.kind === "unverifiable")
      ) {
        return `${type} · ${integrity.kind}`;
      }
    }
    return type;
  }
  return "unknown";
}

export function sessionDotTone(state: unknown): "green" | "terracotta" | "border" {
  const label = sessionStateLabel(state);
  if (label === "live") return "green";
  if (label.startsWith("silent")) return "border";
  if (label.startsWith("recovered") || label.startsWith("ended ·")) return "border";
  return "terracotta";
}

export function sessionTitle(session: Pick<Session, "id" | "title" | "kind">): string {
  const title = session.title.trim();
  if (title) return title;
  return `${isAgentKind(session.kind) ? "Agent" : "Terminal"} ${session.id.slice(0, 8)}`;
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
      const listed = workspaceSessions(await source.list());
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

  const create = async (
    kind: SessionKind = "acp",
    provider: string | null = null,
  ): Promise<Session | null> => {
    if (state.creating) return null;
    ++refreshGeneration;
    publish({ ...state, creating: true, error: null });
    try {
      const session = await source.create(kind, provider);
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
    open: (session) => {
      ++refreshGeneration;
      publish({
        ...state,
        sessions: [...state.sessions.filter((current) => current.id !== session.id), session],
        selectedSessionId: session.id,
        error: null,
      });
    },
  };
}

export function chatCapableProviders(providers: ProviderInfo[]): ProviderInfo[] {
  return providers.filter(
    (provider) =>
      provider.pickable !== false &&
      (provider.protocol === "acp" || provider.protocol === "stream-json"),
  );
}

/** True when the provider spawns via npx and downloads third-party code on first run. */
export function requiresConsent(provider: ProviderInfo): boolean {
  return provider.origin === "npx-wrapper";
}

export function sessionCreateFromProvider(provider: ProviderInfo | undefined): {
  kind: SessionKind;
  provider: string | null;
} {
  if (provider === undefined) return { kind: "acp", provider: null };
  if (provider.protocol === "stream-json") return { kind: "claude", provider: null };
  if (provider.protocol === "acp") return { kind: "acp", provider: provider.id };
  return { kind: "acp", provider: null };
}

export function useWorkspaceSessions(): WorkspaceSessionState & {
  refresh: () => Promise<void>;
  create: (kind?: SessionKind, provider?: string | null) => Promise<Session | null>;
  select: (sessionId: string) => void;
  open: (session: Session) => void;
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
  const create = useCallback(
    (kind?: SessionKind, provider?: string | null) => controller.create(kind, provider),
    [controller],
  );
  const select = useCallback((sessionId: string) => controller.select(sessionId), [controller]);
  const open = useCallback((session: Session) => controller.open(session), [controller]);

  return { ...state, refresh, create, select, open };
}
