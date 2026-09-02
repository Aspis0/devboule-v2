import { sessionsList } from "../../lib/tauri";
import type { Session } from "../../types/ipc";

/**
 * The only channel a plugin frame gets to ask the host for work. The frame is
 * checked by both origin and identity because either check alone can admit a
 * message from a different document.
 */
export interface PluginBridgeOptions {
  iframe: HTMLIFrameElement;
  pluginId: string;
  pluginOrigin: string;
  capabilities: readonly string[];
  timeoutMs?: number;
  /** Forward a granted method to `plugin_invoke`. Absent: the host cannot route yet. */
  route?: (method: string, payload: unknown) => Promise<unknown>;
  /** Host-side allowlist: manifest request is necessary but not sufficient. */
  servedCapabilities?: readonly string[];
  /** Injectable for tests; production uses the Tauri sessions_list command. */
  sessionList?: () => Promise<Session[]>;
}

export interface PluginBridge {
  /** Send a request to the frame and reject if it never replies. */
  invoke(method: string, payload?: unknown): Promise<unknown>;
  dispose(): void;
}

interface InvokeMessage {
  v: 1;
  id: string;
  kind: "invoke";
  method: string;
  payload?: unknown;
}

type ReplyMessage =
  | { v: 1; id: string; kind: "result"; value: unknown }
  | { v: 1; id: string; kind: "error"; message: string; code?: string };

export interface PluginSession {
  id: string;
  provider: string | null;
  state: "working" | "finished";
  title: string;
}

export interface SessionFeed {
  sessions: PluginSession[];
}

type SessionEventMessage = {
  v: 1;
  id: string;
  kind: "event";
  event: "sessions.update";
  value: SessionFeed;
};

export const HOST_SERVED_CAPABILITIES = ["sessions.watch"] as const;
const SESSION_WATCH_INTERVAL_MS = 5000;
const MAX_PLUGIN_MESSAGE_BYTES = 1024 * 1024;

/**
 * Provisional provider inference. Session truth has no provider field yet, so
 * title matching is deliberately small and explicit. Longer names win before
 * `pi`, whose short substring must not steal a Copilot title.
 */
const PROVIDER_MATCH_ORDER = ["claude", "codex", "opencode", "copilot", "grok", "pi"] as const;

export function deriveSessionProvider(title: string): string | null {
  const normalized = title.toLowerCase();
  return PROVIDER_MATCH_ORDER.find((provider) => normalized.includes(provider)) ?? null;
}

/**
 * Host Session -> plugin agent mapping:
 * - live is working: it is the only state that proves a process is alive.
 * - ended is finished.
 * - recovered is finished, with a roster title marker because its transcript
 *   is recovered text, not evidence of a living process.
 * - silent has no host source in this slice and is never fabricated here.
 */
export function sessionToPluginSession(session: Session): PluginSession {
  const recovered = session.state.type === "recovered";
  return {
    id: session.id,
    provider: deriveSessionProvider(session.title),
    state: session.state.type === "live" ? "working" : "finished",
    title: recovered ? `${session.title} [recovered transcript]` : session.title,
  };
}

export function sessionsToFeed(sessions: readonly Session[]): SessionFeed {
  return { sessions: sessions.map(sessionToPluginSession) };
}

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

const DEFAULT_TIMEOUT_MS = 10_000;

interface SessionWatchState {
  pluginId: string;
  source: () => Promise<Session[]>;
  post: ((message: SessionEventMessage) => void) | null;
  subscriptionId: string;
  feed: SessionFeed | null;
  initial: Promise<SessionFeed> | null;
  timer: ReturnType<typeof setInterval> | null;
  detachTimer: ReturnType<typeof setTimeout> | null;
  refreshing: boolean;
  stopped: boolean;
}

const sessionWatches = new Map<string, SessionWatchState>();

function rebindSessionWatch(pluginId: string, post: (message: SessionEventMessage) => void): void {
  const state = sessionWatches.get(pluginId);
  if (state === undefined) return;
  state.stopped = false;
  state.post = post;
  if (state.detachTimer !== null) {
    clearTimeout(state.detachTimer);
    state.detachTimer = null;
  }
}

function detachSessionWatch(pluginId: string, post: (message: SessionEventMessage) => void): void {
  const state = sessionWatches.get(pluginId);
  if (state === undefined || state.post !== post) return;
  state.post = null;
  state.stopped = true;
  state.detachTimer = setTimeout(() => {
    if (state.post !== null) return;
    if (state.timer !== null) clearInterval(state.timer);
    state.timer = null;
    sessionWatches.delete(pluginId);
  }, 0);
}

function ensureSessionWatch(
  pluginId: string,
  source: () => Promise<Session[]>,
  subscriptionId: string,
  post: (message: SessionEventMessage) => void,
): SessionWatchState {
  const existing = sessionWatches.get(pluginId);
  if (existing !== undefined) {
    existing.source = source;
    existing.stopped = false;
    existing.subscriptionId = subscriptionId;
    existing.post = post;
    if (existing.detachTimer !== null) {
      clearTimeout(existing.detachTimer);
      existing.detachTimer = null;
    }
    return existing;
  }

  const state: SessionWatchState = {
    pluginId,
    source,
    post,
    subscriptionId,
    feed: null,
    initial: null,
    timer: null,
    detachTimer: null,
    refreshing: false,
    stopped: false,
  };
  sessionWatches.set(pluginId, state);
  return state;
}

function sessionWatchInitial(state: SessionWatchState): Promise<SessionFeed> {
  if (state.feed !== null) return Promise.resolve(state.feed);
  if (state.initial !== null) return state.initial;

  state.initial = state
    .source()
    .then((sessions) => {
      const feed = sessionsToFeed(sessions);
      if (state.stopped) return feed;
      if (!messageValueWithinLimit(feed)) throw oversizedMessageError();
      state.feed = feed;
      state.timer = setInterval(() => {
        void refreshSessionWatch(state);
      }, SESSION_WATCH_INTERVAL_MS);
      return feed;
    })
    .catch((error: unknown) => {
      state.initial = null;
      throw error;
    });
  return state.initial;
}

function oversizedMessageError(): Error & { code: string } {
  return Object.assign(new Error("plugin session feed is too large (maximum 1 MiB)"), {
    code: "response_too_large",
  });
}

async function refreshSessionWatch(state: SessionWatchState): Promise<void> {
  if (state.stopped || state.post === null || state.refreshing) return;
  state.refreshing = true;
  try {
    const next = sessionsToFeed(await state.source());
    const serialized = JSON.stringify(next);
    if (serialized === JSON.stringify(state.feed)) return;
    state.feed = next;
    if (!messageValueWithinLimit(next)) return;
    state.post?.({
      v: 1,
      id: state.subscriptionId,
      kind: "event",
      event: "sessions.update",
      value: next,
    });
  } catch {
    // A transient list failure leaves the last truthful snapshot in place.
    // The next 5-second refresh retries while the subscribed bridge remains.
  } finally {
    state.refreshing = false;
  }
}

function messageValueWithinLimit(value: unknown): boolean {
  try {
    const serialized = JSON.stringify(value);
    return serialized === undefined
      ? true
      : new TextEncoder().encode(serialized).byteLength <= MAX_PLUGIN_MESSAGE_BYTES;
  } catch {
    return false;
  }
}

/** Attach the host side of the frame's versioned, capability-limited channel. */
export function createPluginBridge(options: PluginBridgeOptions): PluginBridge {
  const pluginOrigin = options.pluginOrigin.replace(/\/+$/, "");
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const pending = new Map<string, PendingRequest>();
  const servedCapabilities = options.servedCapabilities ?? HOST_SERVED_CAPABILITIES;
  const sessionList = options.sessionList ?? sessionsList;
  let nextId = 0;
  let disposed = false;

  function post(reply: ReplyMessage | SessionEventMessage): void {
    options.iframe.contentWindow?.postMessage(reply, pluginOrigin);
  }

  const postSessionEvent = (message: SessionEventMessage): void => post(message);
  rebindSessionWatch(options.pluginId, postSessionEvent);

  function handleMessage(event: MessageEvent<unknown>): void {
    // Both checks are required: an attacker can send from a trusted origin in
    // another window, and a frame can navigate to a different origin.
    if (event.origin !== pluginOrigin || event.source !== options.iframe.contentWindow) return;

    const message = parseMessage(event.data);
    if (message === null) return;

    if (message.kind === "invoke") {
      const capability = message.method;
      if (!options.capabilities.includes(capability)) {
        post({
          v: 1,
          id: message.id,
          kind: "error",
          message: `Plugin capability "${capability}" was not requested in the manifest`,
        });
        return;
      }

      if (capability === "sessions.watch") {
        if (!servedCapabilities.includes(capability)) {
          post({
            v: 1,
            id: message.id,
            kind: "error",
            code: "capability_not_supported",
            message: `The host does not serve plugin capability "${capability}"`,
          });
          return;
        }
        const payload = isRecord(message.payload) ? message.payload : undefined;
        const subscriptionId =
          typeof payload?.subscriptionId === "string" && payload.subscriptionId !== ""
            ? payload.subscriptionId
            : `${options.pluginId}-sessions`;
        const state = ensureSessionWatch(
          options.pluginId,
          sessionList,
          subscriptionId,
          postSessionEvent,
        );
        void sessionWatchInitial(state).then(
          (value) => {
            if (!disposed) post({ v: 1, id: message.id, kind: "result", value });
          },
          (error: unknown) => {
            if (disposed) return;
            const reply: Extract<ReplyMessage, { kind: "error" }> = {
              v: 1,
              id: message.id,
              kind: "error",
              message: routeErrorMessage(error),
            };
            const code = routeErrorCode(error);
            if (code !== undefined) reply.code = code;
            post(reply);
          },
        );
        return;
      }

      if (options.route === undefined) {
        post({
          v: 1,
          id: message.id,
          kind: "error",
          message: `The host cannot route plugin method "${message.method}" yet`,
        });
        return;
      }

      const requestId = message.id;
      void options.route(message.method, message.payload).then(
        (value) => {
          if (disposed) return;
          post({ v: 1, id: requestId, kind: "result", value });
        },
        (error: unknown) => {
          if (disposed) return;
          const reply: Extract<ReplyMessage, { kind: "error" }> = {
            v: 1,
            id: requestId,
            kind: "error",
            message: routeErrorMessage(error),
          };
          const code = routeErrorCode(error);
          if (code !== undefined) reply.code = code;
          post(reply);
        },
      );
      return;
    }

    const request = pending.get(message.id);
    if (request === undefined) return;
    pending.delete(message.id);
    clearTimeout(request.timer);
    if (message.kind === "result") request.resolve(message.value);
    else request.reject(new Error(message.message));
  }

  window.addEventListener("message", handleMessage);

  return {
    invoke(method, payload) {
      if (disposed) return Promise.reject(new Error("Plugin bridge is disposed"));
      const frame = options.iframe.contentWindow;
      if (frame === null) return Promise.reject(new Error("Plugin frame is unavailable"));

      const id = `${options.pluginId}-${++nextId}`;
      const message: InvokeMessage = { v: 1, id, kind: "invoke", method };
      if (payload !== undefined) message.payload = payload;

      return new Promise<unknown>((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`Plugin request "${method}" timed out`));
        }, timeoutMs);
        pending.set(id, { resolve, reject, timer });
        try {
          frame.postMessage(message, pluginOrigin);
        } catch (error) {
          clearTimeout(timer);
          pending.delete(id);
          reject(error instanceof Error ? error : new Error("Plugin request could not be sent"));
        }
      });
    },

    dispose() {
      if (disposed) return;
      disposed = true;
      window.removeEventListener("message", handleMessage);
      for (const [id, request] of pending) {
        clearTimeout(request.timer);
        request.reject(new Error(`Plugin request "${id}" was cancelled`));
      }
      pending.clear();
      detachSessionWatch(options.pluginId, postSessionEvent);
    },
  };
}

function parseMessage(value: unknown): InvokeMessage | ReplyMessage | null {
  if (!isRecord(value) || value.v !== 1 || typeof value.id !== "string" || value.id === "") {
    return null;
  }

  if (value.kind === "invoke" && typeof value.method === "string" && value.method !== "") {
    return value as unknown as InvokeMessage;
  }
  if (value.kind === "result" && "value" in value) {
    return value as unknown as ReplyMessage;
  }
  if (
    value.kind === "error" &&
    typeof value.message === "string" &&
    (value.code === undefined || typeof value.code === "string")
  ) {
    return value as unknown as ReplyMessage;
  }
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Tauri rejects with `{ code, message }`, not always an Error. */
function routeErrorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message: unknown }).message;
    if (typeof message === "string" && message !== "") return message;
  }
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return "plugin invoke failed";
}

function routeErrorCode(error: unknown): string | undefined {
  if (isRecord(error) && typeof error.code === "string" && error.code !== "") {
    return error.code;
  }
  if (error instanceof Error) {
    const code = (error as Error & { code?: unknown }).code;
    return typeof code === "string" && code !== "" ? code : undefined;
  }
  return undefined;
}
