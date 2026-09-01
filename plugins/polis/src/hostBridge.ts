import type { City } from "./model";

/**
 * The only way this document talks to the host. A raw Tauri invoke from
 * inside the frame hangs forever; everything goes through postMessage.
 */

const DEFAULT_TIMEOUT_MS = 4000;
export const MAX_HOST_RESPONSE_BYTES = 1024 * 1024;
export const CITY_FETCH_TIMEOUT_MS = 30_000;

export type CityLoadState =
  | { status: "pending"; city: null }
  | { status: "host"; city: City }
  | { status: "fixture"; city: City; error: unknown };

type HostInvoker = (
  method: string,
  payload?: unknown,
  timeoutMs?: number,
) => Promise<unknown>;

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
      reject(new Error(`Host request "${method}" timed out after ${timeoutMs / 1000} seconds`));
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
          const error = new Error(
            "plugin response is too large (maximum 1 MiB)",
          ) as Error & { code?: string };
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
    if (!isCity(value)) throw new Error("city.get returned an invalid city payload");
    return {
      status: "host",
      city: { ...value, agents: [], findings: [], dataSource: "host" },
    };
  } catch (error) {
    return { status: "fixture", city: fallback, error };
  }
}

export function cityHudLabel(city: City): "Host city" | "Fixture city" {
  return city.dataSource === "host" ? "Host city" : "Fixture city";
}

export function formatCityFetchReadout(
  state: Exclude<CityLoadState, { status: "pending" }>,
): string {
  if (state.status === "host") {
    return `City: host · ${state.city.files.length} files · ${state.city.imports.length} directed roads`;
  }
  return `City: fixture fallback — host city fetch refused — ${errorMessage(state.error)}`;
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCity(value: unknown): value is City {
  if (!isRecord(value) || !Array.isArray(value.files) || !Array.isArray(value.imports)) {
    return false;
  }
  return (
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
