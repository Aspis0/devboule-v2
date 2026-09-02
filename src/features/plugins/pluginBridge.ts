import { oracleAsk, oracleStatus, sessionsList } from "../../lib/tauri";
import type { OracleIndexStatus, OracleSearchResponse, Session } from "../../types/ipc";

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
  /** Injectable for tests; production uses the Tauri oracle_ask command. */
  oracleSearch?: (query: string) => Promise<OracleSearchResponse>;
  /** Injectable for tests; production uses the Tauri oracle_status command. */
  oracleIndexState?: () => Promise<OracleIndexStatus>;
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

export const HOST_SERVED_CAPABILITIES = ["sessions.watch", "oracle.search"] as const;
const SESSION_WATCH_INTERVAL_MS = 5000;
const MAX_PLUGIN_MESSAGE_BYTES = 1024 * 1024;
export const ORACLE_SOURCE_TIMEOUT_MS = 20_000;
const ORACLE_INDEX_STATE_TIMEOUT_MS = 3000;
const ORACLE_MAX_QUERY_CHARS = 4096;

/**
 * Provisional provider inference. Session truth has no provider field yet, so
 * title matching is deliberately small and explicit. Longer names win before
 * `pi`, whose short substring must not steal a Copilot title.
 */
const PROVIDER_MATCH_ORDER = ["opencode", "claude", "copilot", "codex", "grok", "pi"] as const;

export function deriveSessionProvider(title: string): string | null {
  const normalized = title.toLowerCase();
  return (
    PROVIDER_MATCH_ORDER.find((provider) =>
      new RegExp(`(?:^|[^a-z0-9])${provider}(?=$|[^a-z0-9])`, "i").test(normalized),
    ) ?? null
  );
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

interface PendingOracleSearch {
  id: string;
  query: string;
}

const DEFAULT_TIMEOUT_MS = 10_000;
const SESSION_SOURCE_TIMEOUT_MS = 10_000;

type SessionSubscriberPost = (message: ReplyMessage | SessionEventMessage) => void;

interface SessionSubscriber {
  post: SessionSubscriberPost;
}

interface SessionWatchState {
  source: () => Promise<Session[]>;
  subscribers: Map<string, SessionSubscriber>;
  feed: SessionFeed | null;
  initial: Promise<SessionFeed> | null;
  initialGeneration: number | null;
  timer: ReturnType<typeof setInterval> | null;
  detachTimer: ReturnType<typeof setTimeout> | null;
  refreshing: boolean;
  stopped: boolean;
  generation: number;
}

const sessionWatches = new Map<string, SessionWatchState>();

function rebindSessionWatch(pluginId: string): void {
  const state = sessionWatches.get(pluginId);
  if (state === undefined) return;
  state.stopped = false;
  if (state.detachTimer !== null) {
    clearTimeout(state.detachTimer);
    state.detachTimer = null;
  }
}

function detachSessionWatch(pluginId: string, post: SessionSubscriberPost): void {
  const state = sessionWatches.get(pluginId);
  if (state === undefined) return;
  let detached = false;
  for (const [subscriptionId, subscriber] of state.subscribers) {
    if (subscriber.post !== post) continue;
    state.subscribers.delete(subscriptionId);
    detached = true;
  }
  if (!detached || state.subscribers.size > 0) return;
  state.stopped = true;
  state.generation += 1;
  state.initial = null;
  state.initialGeneration = null;
  if (state.timer !== null) clearInterval(state.timer);
  state.timer = null;
  state.detachTimer = setTimeout(() => {
    if (!state.stopped) return;
    sessionWatches.delete(pluginId);
  }, 0);
}

function ensureSessionWatch(
  pluginId: string,
  source: () => Promise<Session[]>,
  subscriptionId: string,
  post: SessionSubscriberPost,
): SessionWatchState {
  const existing = sessionWatches.get(pluginId);
  if (existing !== undefined) {
    existing.source = source;
    existing.stopped = false;
    existing.subscribers.set(subscriptionId, { post });
    if (existing.detachTimer !== null) {
      clearTimeout(existing.detachTimer);
      existing.detachTimer = null;
    }
    return existing;
  }

  const state: SessionWatchState = {
    source,
    subscribers: new Map([[subscriptionId, { post }]]),
    feed: null,
    initial: null,
    initialGeneration: null,
    timer: null,
    detachTimer: null,
    refreshing: false,
    stopped: false,
    generation: 0,
  };
  sessionWatches.set(pluginId, state);
  return state;
}

function startSessionWatch(state: SessionWatchState): void {
  if (state.stopped || state.timer !== null || state.subscribers.size === 0) return;
  if (state.feed === null) return;
  state.timer = setInterval(() => {
    void refreshSessionWatch(state);
  }, SESSION_WATCH_INTERVAL_MS);
}

function stopSessionWatch(pluginId: string, state: SessionWatchState): void {
  state.stopped = true;
  state.generation += 1;
  state.initial = null;
  state.initialGeneration = null;
  if (state.timer !== null) clearInterval(state.timer);
  state.timer = null;
  sessionWatches.delete(pluginId);
}

function sessionWatchInitial(state: SessionWatchState): Promise<SessionFeed> {
  if (state.feed !== null) return Promise.resolve(state.feed);
  if (state.initial !== null) return state.initial;

  const generation = state.generation;
  const initial = withTimeout(
    Promise.resolve().then(() => state.source()),
    SESSION_SOURCE_TIMEOUT_MS,
    "sessions.watch",
  )
    .then((sessions) => {
      const feed = sessionsToFeed(sessions);
      if (state.stopped || state.generation !== generation) {
        if (state.initialGeneration === generation) {
          state.initial = null;
          state.initialGeneration = null;
        }
        return feed;
      }
      if (!messageValueWithinLimit(feed)) throw oversizedMessageError();
      return feed;
    })
    .catch((error: unknown) => {
      if (state.initialGeneration === generation) {
        state.initial = null;
        state.initialGeneration = null;
      }
      throw error;
    });
  state.initial = initial;
  state.initialGeneration = generation;
  return initial;
}

function oversizedMessageError(subject = "plugin session feed"): Error & { code: string } {
  return Object.assign(new Error(`${subject} is too large (maximum 1 MiB)`), {
    code: "response_too_large",
  });
}

function sessionWatchFailureReply(
  id: string,
  error: unknown,
): Extract<ReplyMessage, { kind: "error" }> {
  const reply: Extract<ReplyMessage, { kind: "error" }> = {
    v: 1,
    id,
    kind: "error",
    message: "sessions.watch failed",
  };
  const code = routeErrorCode(error);
  if (code !== undefined) reply.code = code;
  return reply;
}

function oracleSearchFailureReply(
  id: string,
  error: unknown,
): Extract<ReplyMessage, { kind: "error" }> {
  const reply: Extract<ReplyMessage, { kind: "error" }> = {
    v: 1,
    id,
    kind: "error",
    message: "oracle.search failed",
  };
  const code = routeErrorCode(error);
  if (code !== undefined) reply.code = code;
  return reply;
}

async function refreshSessionWatch(state: SessionWatchState): Promise<void> {
  if (state.stopped || state.subscribers.size === 0 || state.refreshing) return;
  state.refreshing = true;
  try {
    const next = sessionsToFeed(
      await withTimeout(
        Promise.resolve().then(() => state.source()),
        SESSION_SOURCE_TIMEOUT_MS,
        "sessions.watch",
      ),
    );
    const serialized = JSON.stringify(next);
    if (serialized === JSON.stringify(state.feed)) return;
    if (!messageValueWithinLimit(next)) return;
    for (const [subscriptionId, subscriber] of state.subscribers) {
      subscriber.post({
        v: 1,
        id: subscriptionId,
        kind: "event",
        event: "sessions.update",
        value: next,
      });
    }
    state.feed = next;
  } catch {
    // A transient list failure leaves the last truthful snapshot in place.
    // The next 5-second refresh retries while the subscribed bridge remains.
  } finally {
    state.refreshing = false;
  }
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(Object.assign(new Error(`${label} timed out`), { code: "timeout" }));
    }, timeoutMs);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
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

type OraclePluginMatchType =
  | "lexical"
  | "dense"
  | "dense+lexical"
  | "dense+reranked";
type OraclePluginIndexState = "idle" | "indexing" | "ready" | "incomplete" | "stale" | "error";

interface OraclePluginResult {
  path: string;
  startLine: number;
  endLine: number;
  focusStartLine?: number;
  focusEndLine?: number;
  symbol?: string;
  match?: OraclePluginMatchType;
}

interface OraclePluginSearchResponse {
  query: string;
  results: OraclePluginResult[];
  index?: {
    state: OraclePluginIndexState;
    indexedFiles: number;
  };
}

function oracleSearchQuery(payload: unknown): string | null {
  if (!isRecord(payload) || typeof payload.query !== "string") return null;
  const query = payload.query.trim();
  // The user-facing limit applies to the query Oracle actually receives, after trimming.
  return query === "" ? null : query;
}

function projectOracleSearchResponse(
  response: unknown,
  query: string,
): OraclePluginSearchResponse {
  if (!isOracleSearchResponse(response) || response.query !== query) {
    throw invalidOracleResponseError();
  }
  return {
    query,
    results: response.results.slice(0, 10).map((result) => {
      const projected: OraclePluginResult = {
        path: result.path,
        startLine: result.line_start,
        endLine: result.line_end,
      };
      if (
        typeof result.focus_line_start === "number" &&
        typeof result.focus_line_end === "number"
      ) {
        projected.focusStartLine = result.focus_line_start;
        projected.focusEndLine = result.focus_line_end;
      }
      if (typeof result.symbol_name === "string") projected.symbol = result.symbol_name;
      if (isOracleMatchType(result.match_type)) projected.match = result.match_type;
      return projected;
    }),
  };
}

function isOracleSearchResponse(value: unknown): value is OracleSearchResponse {
  if (!isRecord(value) || typeof value.query !== "string" || !Array.isArray(value.results)) {
    return false;
  }
  return value.results.every(isOracleResult);
}

function isOracleResult(value: unknown): value is OracleSearchResponse["results"][number] {
  if (
    !isRecord(value) ||
    typeof value.path !== "string" ||
    value.path.trim() === "" ||
    !isNonNegativeInteger(value.line_start) ||
    !isNonNegativeInteger(value.line_end) ||
    value.line_end < value.line_start ||
    typeof value.snippet !== "string" ||
    typeof value.score !== "number" ||
    !Number.isFinite(value.score)
  ) {
    return false;
  }

  const hasFocusStart = Object.prototype.hasOwnProperty.call(value, "focus_line_start");
  const hasFocusEnd = Object.prototype.hasOwnProperty.call(value, "focus_line_end");
  if (hasFocusStart !== hasFocusEnd) return false;
  if (
    hasFocusStart &&
    (!isNonNegativeInteger(value.focus_line_start) ||
      !isNonNegativeInteger(value.focus_line_end) ||
      value.focus_line_end < value.focus_line_start)
  ) {
    return false;
  }

  if (
    Object.prototype.hasOwnProperty.call(value, "symbol_name") &&
    (typeof value.symbol_name !== "string" || value.symbol_name === "")
  ) {
    return false;
  }
  if (
    Object.prototype.hasOwnProperty.call(value, "match_type") &&
    !isOracleMatchType(value.match_type)
  ) {
    return false;
  }
  return true;
}

function isNonNegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

function isOracleMatchType(value: unknown): value is OraclePluginMatchType {
  return (
    value === "lexical" ||
    value === "dense" ||
    value === "dense+lexical" ||
    value === "dense+reranked"
  );
}

function isOracleIndexState(value: unknown): value is OraclePluginIndexState {
  return (
    value === "idle" ||
    value === "indexing" ||
    value === "ready" ||
    value === "incomplete" ||
    value === "stale" ||
    value === "error"
  );
}

function invalidOracleResponseError(): Error & { code: string } {
  return Object.assign(new Error("oracle.search returned an invalid response"), {
    code: "invalid_response",
  });
}

export function resetSessionWatchesForTests(): void {
  for (const state of sessionWatches.values()) {
    if (state.timer !== null) clearInterval(state.timer);
    if (state.detachTimer !== null) clearTimeout(state.detachTimer);
  }
  sessionWatches.clear();
}

/** Attach the host side of the frame's versioned, capability-limited channel. */
export function createPluginBridge(options: PluginBridgeOptions): PluginBridge {
  const pluginOrigin = options.pluginOrigin.replace(/\/+$/, "");
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const pending = new Map<string, PendingRequest>();
  const servedCapabilities = options.servedCapabilities ?? HOST_SERVED_CAPABILITIES;
  const sessionList = options.sessionList ?? sessionsList;
  const oracleSearch = options.oracleSearch ?? oracleAsk;
  const oracleIndexState = options.oracleIndexState ?? oracleStatus;
  let nextId = 0;
  let disposed = false;
  let oracleSearchInFlight = false;
  let pendingOracleSearch: PendingOracleSearch | null = null;

  function post(reply: ReplyMessage | SessionEventMessage): void {
    options.iframe.contentWindow?.postMessage(reply, pluginOrigin);
  }

  async function runOracleSearch(request: PendingOracleSearch): Promise<void> {
    try {
      const response = await withTimeout(
        Promise.resolve().then(() => oracleSearch(request.query)),
        ORACLE_SOURCE_TIMEOUT_MS,
        "oracle.search",
      );
      const value = projectOracleSearchResponse(response, request.query);
      if (value.results.length === 0) {
        try {
          const status = await withTimeout(
            Promise.resolve().then(() => oracleIndexState()),
            ORACLE_INDEX_STATE_TIMEOUT_MS,
            "oracle.search index status",
          );
          if (
            isRecord(status) &&
            isOracleIndexState(status.state) &&
            isNonNegativeInteger(status.indexed_files)
          ) {
            value.index = {
              state: status.state,
              indexedFiles: status.indexed_files,
            };
          }
        } catch {
          // Index status is explanatory only; a status outage must not hide a valid empty search.
        }
      }
      if (disposed) return;
      if (!messageValueWithinLimit(value)) {
        const error = oversizedMessageError("oracle.search response");
        post({
          v: 1,
          id: request.id,
          kind: "error",
          code: error.code,
          message: error.message,
        });
        return;
      }
      post({ v: 1, id: request.id, kind: "result", value });
    } catch (error: unknown) {
      if (disposed) return;
      console.error("oracle.search failed", error);
      post(oracleSearchFailureReply(request.id, error));
    } finally {
      oracleSearchInFlight = false;
      if (disposed) {
        pendingOracleSearch = null;
        return;
      }
      const pending = pendingOracleSearch;
      pendingOracleSearch = null;
      if (pending !== null) {
        oracleSearchInFlight = true;
        void runOracleSearch(pending);
      }
    }
  }

  const postSessionMessage: SessionSubscriberPost = (message) => post(message);
  rebindSessionWatch(options.pluginId);

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
        if (payload?.action === "unsubscribe") {
          const state = sessionWatches.get(options.pluginId);
          state?.subscribers.delete(subscriptionId);
          if (state !== undefined && state.subscribers.size === 0) {
            stopSessionWatch(options.pluginId, state);
          }
          post({
            v: 1,
            id: message.id,
            kind: "result",
            value: { unsubscribed: true },
          });
          return;
        }
        const state = ensureSessionWatch(
          options.pluginId,
          sessionList,
          subscriptionId,
          postSessionMessage,
        );
        const generation = state.generation;
        void sessionWatchInitial(state).then(
          (value) => {
            const subscriber = state.subscribers.get(subscriptionId);
            if (state.stopped || state.generation !== generation || subscriber === undefined)
              return;
            subscriber.post({ v: 1, id: message.id, kind: "result", value });
            state.feed = value;
            startSessionWatch(state);
          },
          (error: unknown) => {
            const subscriber = state.subscribers.get(subscriptionId);
            if (state.stopped || subscriber === undefined) return;
            console.error("sessions.watch failed", error);
            subscriber.post(sessionWatchFailureReply(message.id, error));
          },
        );
        return;
      }

      if (capability === "oracle.search") {
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

        const query = oracleSearchQuery(message.payload);
        if (query === null) {
          post({
            v: 1,
            id: message.id,
            kind: "error",
            code: "invalid_request",
            message: "oracle.search requires a non-empty query string",
          });
          return;
        }
        if (query.length > ORACLE_MAX_QUERY_CHARS) {
          post({
            v: 1,
            id: message.id,
            kind: "error",
            code: "invalid_request",
            message: "oracle.search query is too long (maximum 4096 characters)",
          });
          return;
        }
        if (oracleSearchInFlight) {
          if (pendingOracleSearch !== null) {
            post({
              v: 1,
              id: pendingOracleSearch.id,
              kind: "error",
              code: "busy",
              message: "oracle.search is already running for this plugin",
            });
          }
          pendingOracleSearch = { id: message.id, query };
          return;
        }

        oracleSearchInFlight = true;
        void runOracleSearch({ id: message.id, query });
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
      pendingOracleSearch = null;
      detachSessionWatch(options.pluginId, postSessionMessage);
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
