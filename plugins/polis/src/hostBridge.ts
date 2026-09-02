import type { City, CityAgent } from "./model";

/**
 * The only way this document talks to the host. A raw Tauri invoke from
 * inside the frame hangs forever; everything goes through postMessage.
 */

const DEFAULT_TIMEOUT_MS = 4000;
export const MAX_HOST_RESPONSE_BYTES = 1024 * 1024;
export const CITY_FETCH_TIMEOUT_MS = 30_000;
export const SESSIONS_WATCH_TIMEOUT_MS = 10_000;

export type CityLoadState =
  | { status: "pending"; city: null }
  | { status: "host"; city: City }
  | {
      status: "fixture";
      city: City;
      error: unknown;
      failure: "timeout" | "refusal" | "malformed";
    };

export type HostInvoker = (
  method: string,
  payload?: unknown,
  timeoutMs?: number,
) => Promise<unknown>;

export interface PluginSession {
  id: string;
  provider: string | null;
  state: "working" | "finished";
  title: string;
}

export interface SessionFeed {
  sessions: PluginSession[];
}

export interface SessionSubscription {
  close(): void;
}

export function invokeHost(
  method: string,
  payload?: unknown,
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<unknown> {
  const id = `polis-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = window.setTimeout(() => {
      if (settled) return;
      settled = true;
      window.removeEventListener("message", onMessage);
      const error = new Error(
        `Host request "${method}" timed out after ${timeoutMs / 1000} seconds`,
      ) as Error & { code?: string };
      error.code = "timeout";
      reject(error);
    }, timeoutMs);

    function onMessage(event: MessageEvent<unknown>): void {
      if (settled || event.source !== window.parent) return;
      const message = event.data;
      if (!isReply(message) || message.id !== id) return;
      settled = true;
      window.clearTimeout(timer);
      window.removeEventListener("message", onMessage);
      if (message.kind === "result") {
        if (!hostResponseWithinLimit(message.value)) {
          const error = new Error("plugin response is too large (maximum 1 MiB)") as Error & {
            code?: string;
          };
          error.code = "response_too_large";
          reject(error);
        } else {
          resolve(message.value);
        }
      } else {
        const error = new Error(message.message) as Error & { code?: string };
        if (message.code !== undefined) error.code = message.code;
        reject(error);
      }
    }

    window.addEventListener("message", onMessage);
    const request: { v: 1; id: string; kind: "invoke"; method: string; payload?: unknown } = {
      v: 1,
      id,
      kind: "invoke",
      method,
    };
    if (payload !== undefined) request.payload = payload;
    window.parent.postMessage(request, "*");
  });
}

/** Keep the returned Value bounded before it is retained by the plugin frame. */
export function hostResponseWithinLimit(value: unknown): boolean {
  try {
    const serialized = JSON.stringify(value);
    if (serialized === undefined) return true;
    return new TextEncoder().encode(serialized).byteLength <= MAX_HOST_RESPONSE_BYTES;
  } catch {
    return false;
  }
}

export function pendingCityState(): CityLoadState {
  return { status: "pending", city: null };
}

export async function loadCity(
  invoke: HostInvoker,
  fallback: City,
  timeoutMs = CITY_FETCH_TIMEOUT_MS,
): Promise<Exclude<CityLoadState, { status: "pending" }>> {
  try {
    const value = await invoke("city.get", undefined, timeoutMs);
    if (!isCity(value)) {
      const error = new Error("city.get returned an invalid city payload") as Error & {
        code?: string;
      };
      error.code = "malformed_city";
      throw error;
    }
    return {
      status: "host",
      city: { ...value, findings: [], dataSource: "host" },
    };
  } catch (error) {
    return { status: "fixture", city: fallback, error, failure: cityFetchFailure(error) };
  }
}

/** Validate the event value before it becomes a renderer-facing agent list. */
export function isSessionFeed(value: unknown): value is SessionFeed {
  if (!isRecord(value) || !Array.isArray(value.sessions)) return false;
  if (Object.keys(value).some((key) => key !== "sessions")) return false;
  return value.sessions.every((session) => {
    if (!isRecord(session)) return false;
    const keys = Object.keys(session);
    if (keys.some((key) => !["id", "provider", "state", "title"].includes(key))) return false;
    return (
      typeof session.id === "string" &&
      session.id !== "" &&
      (typeof session.provider === "string" || session.provider === null) &&
      (session.state === "working" || session.state === "finished") &&
      typeof session.title === "string"
    );
  });
}

export function sessionFeedToAgents(feed: SessionFeed): CityAgent[] {
  return feed.sessions.map((session) => ({
    id: session.id,
    provider: session.provider,
    state: session.state,
    fileId: null,
    title: session.title,
  }));
}

/**
 * Session events use the same v1 source check as replies, but are kept on one
 * persistent listener so a subscription survives ordinary request settlement.
 */
export async function subscribeSessions(
  invoke: HostInvoker,
  onUpdate: (agents: CityAgent[]) => void,
  timeoutMs = SESSIONS_WATCH_TIMEOUT_MS,
): Promise<SessionSubscription> {
  const subscriptionId = `polis-sessions-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  let closed = false;

  const notifyUnwatch = (): void => {
    void invoke("sessions.watch", { subscriptionId, action: "unsubscribe" }, timeoutMs).catch(
      () => undefined,
    );
  };

  const close = (): void => {
    if (closed) return;
    closed = true;
    window.removeEventListener("message", onMessage);
    window.removeEventListener("pagehide", onPageHide);
    notifyUnwatch();
  };

  function onPageHide(): void {
    close();
  }

  function onMessage(event: MessageEvent<unknown>): void {
    if (closed || event.source !== window.parent) return;
    const message = event.data;
    if (!isSessionEvent(message) || message.id !== subscriptionId) return;
    if (!hostResponseWithinLimit(message.value) || !isSessionFeed(message.value)) return;
    onUpdate(sessionFeedToAgents(message.value));
  }

  window.addEventListener("message", onMessage);
  window.addEventListener("pagehide", onPageHide);
  try {
    const value = await invoke("sessions.watch", { subscriptionId }, timeoutMs);
    if (!isSessionFeed(value)) {
      const error = new Error("sessions.watch returned an invalid session feed") as Error & {
        code?: string;
      };
      error.code = "malformed_sessions";
      throw error;
    }
    onUpdate(sessionFeedToAgents(value));
    return { close };
  } catch (error) {
    close();
    throw error;
  }
}

export function cityHudLabel(city: City): "Host city" | "Fixture city" {
  return city.dataSource === "host" ? "Host city" : "Fixture city";
}

export function formatCityFetchReadout(
  state: Exclude<CityLoadState, { status: "pending" }>,
): string {
  if (state.status === "host") {
    return `City: host · ${state.city.files.length} files${cityDegradationSuffix(state.city)} · ${state.city.imports.length} directed roads`;
  }
  const label =
    state.failure === "refusal"
      ? "fetch refused"
      : state.failure === "malformed"
        ? "malformed"
        : "timeout";
  return `City: fixture fallback — host city ${label} — ${errorMessage(state.error)}`;
}

export function cityDegradationSuffix(city: City): string {
  const notices: string[] = [];
  if (city.truncatedFiles !== undefined && city.truncatedFiles > 0) {
    notices.push(`at least ${city.truncatedFiles} beyond the file cap`);
  }
  if (city.skippedFiles !== undefined && city.skippedFiles > 0) {
    notices.push(`${city.skippedFiles} skipped`);
  }
  return notices.length === 0 ? "" : ` (${notices.join(", ")})`;
}

export function formatWorkspaceRootReadout(value: unknown): string {
  if (!isRecord(value)) return `Bridge reply: result — ${safeJson(value)}`;
  const root = typeof value.root === "string" ? value.root : "";
  const status = typeof value.status === "string" ? value.status : "unknown";
  if (root === "") {
    return `Bridge reply: workspace.root status ${status} — host did not grant a root`;
  }
  return `Bridge reply: workspace.root ${status} — ${root}`;
}

export function formatHandshakeReadout(value: unknown): string {
  if (!isRecord(value)) return "Backend: no handshake payload";
  const handshake = isRecord(value.handshake) ? value.handshake : value;
  const protocolVersion = handshake.protocolVersion;
  const instanceId = handshake.instanceId;
  const pid = handshake.pid;
  const capabilities = Array.isArray(handshake.capabilities)
    ? handshake.capabilities.filter((item): item is string => typeof item === "string")
    : [];
  if (protocolVersion === undefined && instanceId === undefined && pid === undefined) {
    return "Backend: handshake missing from the workspace.root reply";
  }
  return `Backend: handshake ok · protocol ${String(protocolVersion)} · pid ${String(pid)} · ${capabilities.join(", ") || "no capabilities"}`;
}

export function formatBackendFailureReadout(error: unknown): string {
  const code = errorCode(error);
  const message = errorMessage(error);
  const lower = message.toLowerCase();
  let state: string;
  if (code === "workspace_unavailable") {
    state = "no project open";
  } else if (code === "workspace_confinement_refused") {
    state = "workspace root refused";
  } else if (lower.includes("did not declare a backend") || lower.includes("no backend")) {
    state = "no backend declared";
  } else if (lower.includes("timed out") || lower.includes("timeout")) {
    state = "timeout";
  } else if (lower.includes("capability") || lower.includes("not in the granted")) {
    state = "capability refused";
  } else if (
    lower.includes("handshake") ||
    lower.includes("peer pid") ||
    lower.includes("protocol version") ||
    lower.includes("hello")
  ) {
    state = "handshake refused";
  } else {
    state = "spawn failed";
  }
  return `Backend: ${state} — ${message}`;
}

function isReply(
  value: unknown,
): value is
  | { v: 1; id: string; kind: "result"; value: unknown }
  | { v: 1; id: string; kind: "error"; message: string; code?: string } {
  if (!isRecord(value) || value.v !== 1 || typeof value.id !== "string") return false;
  if (value.kind === "result" && "value" in value) return true;
  if (
    value.kind === "error" &&
    typeof value.message === "string" &&
    (value.code === undefined || typeof value.code === "string")
  ) {
    return true;
  }
  return false;
}

function isSessionEvent(
  value: unknown,
): value is { v: 1; id: string; kind: "event"; event: "sessions.update"; value: unknown } {
  return (
    isRecord(value) &&
    value.v === 1 &&
    typeof value.id === "string" &&
    value.id !== "" &&
    value.kind === "event" &&
    value.event === "sessions.update" &&
    "value" in value
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCity(value: unknown): value is City {
  if (
    !isRecord(value) ||
    !Array.isArray(value.files) ||
    !Array.isArray(value.imports) ||
    !Array.isArray(value.agents) ||
    !Array.isArray(value.findings)
  ) {
    return false;
  }
  return (
    optionalCounterIsValid(value.truncatedFiles) &&
    optionalCounterIsValid(value.skippedFiles) &&
    value.files.every(
      (file) =>
        isRecord(file) &&
        typeof file.id === "string" &&
        typeof file.path === "string" &&
        typeof file.lines === "number" &&
        typeof file.district === "string",
    ) &&
    value.imports.every(
      (edge) =>
        isRecord(edge) &&
        typeof edge.from === "string" &&
        typeof edge.to === "string" &&
        typeof edge.weight === "number",
    )
  );
}

function optionalCounterIsValid(value: unknown): value is number | undefined {
  return (
    value === undefined ||
    (typeof value === "number" && Number.isInteger(value) && Number.isFinite(value) && value >= 0)
  );
}

function cityFetchFailure(error: unknown): "timeout" | "refusal" | "malformed" {
  const code = errorCode(error);
  if (code === "timeout") return "timeout";
  if (code === "malformed_city") return "malformed";
  const message = errorMessage(error).toLowerCase();
  if (message.includes("timed out") || message.includes("timeout")) return "timeout";
  return "refusal";
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function errorMessage(error: unknown): string {
  if (isRecord(error) && typeof error.message === "string" && error.message) {
    return error.message;
  }
  return error instanceof Error ? error.message : "unknown error";
}

function errorCode(error: unknown): string | undefined {
  if (isRecord(error) && typeof error.code === "string" && error.code) {
    return error.code;
  }
  if (error instanceof Error) {
    const code = (error as Error & { code?: unknown }).code;
    return typeof code === "string" && code ? code : undefined;
  }
  return undefined;
}
