import { useCallback, useEffect, useState } from "react";
import {
  createSessionStateChannel,
  sessionCreate,
  sessionsList,
  sessionsUnwatch,
  sessionsWatch,
} from "../../lib/tauri";
import type {
  AttentionReason,
  PeerRow,
  ProviderInfo,
  Session,
  SessionKind,
  SessionStateSnapshot,
} from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";

export interface WorkspaceSessionSource {
  list: () => Promise<Session[]>;
  create: (
    workspaceId: string | null,
    kind?: SessionKind,
    provider?: string | null,
  ) => Promise<Session>;
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
  create: (
    kind?: SessionKind,
    provider?: string | null,
    workspaceId?: string | null,
  ) => Promise<Session | null>;
  select: (sessionId: string) => void;
  open: (session: Session) => void;
  watch: () => () => void;
  reconnect: () => Promise<void>;
  dismissError: () => void;
}

const DEFAULT_SOURCE: WorkspaceSessionSource = {
  list: sessionsList,
  create: (workspaceId, kind = "acp", provider = null) =>
    provider == null
      ? sessionCreate(workspaceId, kind)
      : sessionCreate(workspaceId, kind, provider),
  watch: async (listener) => {
    const channel = createSessionStateChannel(listener);
    await sessionsWatch(channel);
    return () => {
      void sessionsUnwatch();
    };
  },
};

const LIST_ERROR = "Could not load sessions. The daemon is unreachable.";
const CREATE_FALLBACK_ERROR = "Could not create the agent session.";

/**
 * A session with a running process belongs in the tab strip. Journal-only
 * records (recovered or ended) stay out of it — they remain reachable from
 * History, and join the strip only once the user opens them there.
 */
function sessionHasProcess(session: Session): boolean {
  return session.state.type === "live" || session.state.type === "silent";
}

/**
 * Message from a rejected invoke. Tauri rejections are not always `Error`s:
 * the daemon's serialized WireError arrives as a plain
 * `{ code, message }` object, which `String(cause)` would render as
 * "[object Object]".
 */
function rejectionMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "object" && cause !== null && "message" in cause) {
    const message = (cause as { message: unknown }).message;
    if (typeof message === "string") return message;
  }
  return String(cause);
}

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

/**
 * Human words for why a session wants attention. Rendered inside the tab
 * button so the reason is part of the tab's accessible name, not only its
 * colour.
 */
export function sessionAttentionLabel(reason: AttentionReason): string {
  if (reason === "permission") return "needs approval";
  return reason;
}

export function sessionTitle(session: Pick<Session, "id" | "title" | "kind">): string {
  const title = session.title.trim();
  if (title) return title;
  return `${isAgentKind(session.kind) ? "Agent" : "Terminal"} ${session.id.slice(0, 8)}`;
}

/**
 * The display name for each paired device, keyed by device id. This is the
 * `DevicesList` map the session badge resolves against; revoked rows are kept,
 * because a session started by a device that has since been revoked still
 * belongs to it.
 */
export function peerDeviceNames(peers: readonly PeerRow[]): Map<string, string> {
  return new Map(peers.map((peer) => [peer.deviceId, peer.displayName]));
}

/**
 * The tab badge for a session of remote origin, or null for a local one.
 *
 * The device is named, never guessed: the daemon stamps only the device id on
 * the origin, and the name comes from the devices list the workspace holds. An
 * id the list does not know yet falls back to the id itself, which is still a
 * true statement about where the session came from; an origin that names no
 * device at all yields no badge rather than an invented one.
 */
export function sessionOriginBadge(
  session: Pick<Session, "origin">,
  deviceNames: ReadonlyMap<string, string>,
): string | null {
  const origin = session.origin;
  if (origin?.kind !== "peer" || origin.deviceId === undefined) return null;
  return `from ${deviceNames.get(origin.deviceId) ?? origin.deviceId}`;
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
  // Ids the user opened explicitly (from History) in this app run. They keep
  // their tab even when the daemon reports no running process.
  const openedIds = new Set<string>();
  let watchLeases = 0;
  let watchPromise: Promise<() => void> | null = null;
  let watchStop: (() => void) | null = null;

  const publish = (next: WorkspaceSessionState): void => {
    state = next;
    for (const listener of listeners) listener();
  };

  const stripSessions = (candidates: readonly Session[]): Session[] =>
    candidates.filter((session) => sessionHasProcess(session) || openedIds.has(session.id));

  const chooseSelected = (
    candidates: readonly Session[],
    preferred: string | null,
  ): string | null =>
    preferred !== null && candidates.some((session) => session.id === preferred)
      ? preferred
      : (candidates[0]?.id ?? null);

  const refresh = async (): Promise<void> => {
    const generation = ++refreshGeneration;
    publish({ ...state, loading: true, error: null });
    try {
      const listed = stripSessions(workspaceSessions(await source.list()));
      if (generation !== refreshGeneration) return;
      publish({
        ...state,
        sessions: listed,
        selectedSessionId: chooseSelected(listed, state.selectedSessionId),
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
      const carried = {
        title: snapshot.title,
        state: snapshot.state,
        elapsedMs: snapshot.elapsedMs,
        // Attention comes and goes with each roster push; assigning it
        // (even undefined) keeps a stale badge from surviving a cleared one.
        attention: snapshot.attention,
        // Origin is session identity, not roster state: a push that stops
        // carrying it (or never did) must not erase what the list already
        // said about a row the app is holding, so the previous value stands in.
        origin: snapshot.origin ?? previous?.origin,
      };
      return previous
        ? { ...previous, ...carried }
        : {
            id: snapshot.id,
            workspaceId: snapshot.workspaceId,
            kind: snapshot.kind,
            ...carried,
          };
    });
    const visible = stripSessions(sessions);
    // The roster is authoritative for opened ids too: a session the daemon no
    // longer reports (deleted from the journal) must not keep a History tab.
    const rosterIds = new Set(sessions.map((session) => session.id));
    for (const id of openedIds) if (!rosterIds.has(id)) openedIds.delete(id);
    publish({
      ...state,
      sessions: visible,
      selectedSessionId: chooseSelected(visible, state.selectedSessionId),
      loading: false,
      error: null,
    });
  };

  const create = async (
    kind: SessionKind = "acp",
    provider: string | null = null,
    workspaceId: string | null = null,
  ): Promise<Session | null> => {
    if (state.creating) return null;
    ++refreshGeneration;
    publish({ ...state, creating: true, error: null });
    try {
      const session = await source.create(workspaceId, kind, provider);
      const listed = stripSessions([
        ...state.sessions.filter((current) => current.id !== session.id),
        session,
      ]);
      publish({
        ...state,
        sessions: listed,
        selectedSessionId: chooseSelected(listed, session.id),
        creating: false,
        error: null,
      });
      return session;
    } catch (cause) {
      // The daemon answered and rejected the start; surface its reason instead
      // of a generic claim about reachability.
      const message = rejectionMessage(cause);
      publish({
        ...state,
        creating: false,
        error: message.trim().length > 0 ? message : CREATE_FALLBACK_ERROR,
      });
      return null;
    }
  };

  const startWatch = (): void => {
    if (!source.watch || watchPromise !== null) return;
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
  };

  const watch = (): (() => void) => {
    watchLeases += 1;
    let released = false;
    startWatch();
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

  /** Reloads after the daemon (re)connected and revives a watch that never came up.
   * Do NOT tear down a live watch here: the Rust bridge owns the roster
   * subscription and rebinds it across daemon recovery (RosterSubscription,
   * src-tauri/src/client/mod.rs), so a started watch survives a restart. Only
   * a failed initial watch needs retrying, and startWatch()'s catch resets
   * watchPromise for exactly that case. */
  const reconnect = async (): Promise<void> => {
    if (watchLeases > 0) startWatch();
    await refresh();
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
    reconnect,
    select: (sessionId) => {
      if (state.sessions.some((session) => session.id === sessionId)) {
        publish({ ...state, selectedSessionId: sessionId });
      }
    },
    open: (session) => {
      ++refreshGeneration;
      openedIds.add(session.id);
      publish({
        ...state,
        sessions: [...state.sessions.filter((current) => current.id !== session.id), session],
        selectedSessionId: session.id,
        error: null,
      });
    },
    dismissError: () => {
      if (state.error === null) return;
      publish({ ...state, error: null });
    },
  };
}

export function chatCapableProviders(providers: ProviderInfo[]): ProviderInfo[] {
  return providers.filter(
    (provider) =>
      provider.pickable !== false &&
      (provider.protocol === "acp" ||
        provider.protocol === "stream-json" ||
        provider.protocol === "pi-rpc" ||
        provider.protocol === "codex-app-server"),
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
  if (provider.protocol === "pi-rpc") return { kind: "pi", provider: null };
  if (provider.protocol === "codex-app-server") return { kind: "codex", provider: null };
  if (provider.protocol === "acp") return { kind: "acp", provider: provider.id };
  return { kind: "acp", provider: null };
}

export function useWorkspaceSessions(workspaceId: string | null = null): WorkspaceSessionState & {
  refresh: () => Promise<void>;
  reconnect: () => Promise<void>;
  create: (
    kind?: SessionKind,
    provider?: string | null,
    workspaceId?: string | null,
  ) => Promise<Session | null>;
  select: (sessionId: string) => void;
  open: (session: Session) => void;
  dismissError: () => void;
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
  const reconnect = useCallback(() => controller.reconnect(), [controller]);
  const create = useCallback(
    (kind?: SessionKind, provider?: string | null, requestedWorkspaceId?: string | null) =>
      controller.create(
        kind,
        provider,
        requestedWorkspaceId === undefined ? workspaceId : requestedWorkspaceId,
      ),
    [controller, workspaceId],
  );
  const select = useCallback((sessionId: string) => controller.select(sessionId), [controller]);
  const open = useCallback((session: Session) => controller.open(session), [controller]);
  const dismissError = useCallback(() => controller.dismissError(), [controller]);

  return { ...state, refresh, reconnect, create, select, open, dismissError };
}
